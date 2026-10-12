//! Atomic application of the object merge (design §5) plus the surrounding
//! pipeline: preconditions, old-client detection (§6), fast-forward guard,
//! crash-safe ref choreography and the startup recovery protocol.

use anyhow::{bail, Context, Result};
use git2::{Oid, Repository, Sort};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::decision::{self, DecisionInput, MergePlan, NewConflict, PlannedFile, Side, Touch};
use super::pending::{self, conflict_ref, TrailerState, REF_APPLYING, REF_PRE_MERGE};
use super::protocol::{self, ProtocolFile};
use super::snapshot::{self, skill_identical, FileEntry, Snapshot, METADATA_DIR};
use super::treebuild::{apply_tree_edits, TreeEdit};
use super::validate::{self, validate_merged_tree};
use crate::core::skill_store::{PendingConflictRow, SkillStore};
use crate::core::sync_metadata;
use crate::core::{git_backup, repo_lock::RepoLock};

/// Human-readable outcome of a sync merge (§4.5/§8). Returned to the
/// frontend by the pull command; commit messages only carry the count line.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct MergeSummary {
    /// "object" or "system" (legacy fallback keeps the old line-merge path).
    pub engine: String,
    pub up_to_date: bool,
    pub fast_forward: bool,
    /// Skills this sync added, changed or removed in the local library.
    pub updated: Vec<UpdatedSkill>,
    /// Presets this sync added, changed or removed in the local library.
    pub presets: Vec<UpdatedPreset>,
    /// Skill ids whose local version was kept over a conflicting remote one.
    pub kept_local: Vec<String>,
    /// Skill ids newly declared "needs attention" by this merge.
    pub new_conflicts: Vec<String>,
    /// Total pendings (old + new) after the merge.
    pub pending_total: usize,
    /// Set when an old-client write was tolerated with a warning (§6).
    pub old_client_warning: Option<String>,
    pub legacy_fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Changed,
    Removed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UpdatedSkill {
    pub skill_id: String,
    pub path: String,
    /// Device (commit author) that last touched this skill on the remote.
    pub from_device: String,
    pub kind: ChangeKind,
}

/// A preset whose file or skill list the sync changed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UpdatedPreset {
    pub preset_id: String,
    pub name: String,
    /// Device (commit author) that last touched this preset on the remote.
    pub from_device: String,
    pub kind: ChangeKind,
}

struct TouchInfo {
    time: i64,
    commit: String,
    author: String,
}

/// Object-merge pull (§5 流程). Caller holds the repo lock and has applied
/// the device identity; this function performs P1/P2, fetch, merge and the
/// atomic apply. Returns the merge summary.
pub fn object_merge_pull_unlocked(store: &SkillStore, skills_dir: &Path) -> Result<MergeSummary> {
    crate::core::resource_store::check_library_transactions()?;
    git_backup::ensure_no_interrupted_git_operation(skills_dir)?;
    crate::core::resource_sync::check_current_writer(&Repository::open(skills_dir)?)?;

    // P1: project the DB to metadata files, then commit anything dirty.
    sync_metadata::write_all_from_db_unlocked(store)?;
    if git_backup::has_uncommitted_changes(skills_dir)? {
        git_backup::commit_all_unlocked(skills_dir, "backup")?;
    }
    // P2: the tree must now be clean (gitignored files aside — porcelain
    // does not list them).
    if git_backup::has_uncommitted_changes(skills_dir)? {
        bail!("working tree still dirty after pre-merge commit; aborting sync");
    }

    let branch = git_backup::current_branch(skills_dir);
    git_backup::fetch_branch(skills_dir, &branch)?;

    let repo = Repository::open(skills_dir).context("failed to open skills repository")?;
    let ours = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .context("repository has no HEAD commit")?;
    let theirs = repo
        .refname_to_id(&format!("refs/remotes/origin/{branch}"))
        .with_context(|| format!("origin/{branch} not found after fetch"))?;

    crate::core::resource_sync::drop_legacy_records(&repo, store)?;

    if theirs == ours {
        return Ok(up_to_date_summary(&repo, store));
    }
    protocol::ensure_supported_format(&repo, &repo.find_commit(theirs)?)?;
    let base = repo
        .merge_base(ours, theirs)
        .context("no common history with the remote (unrelated histories)")?;
    if base == theirs {
        return Ok(up_to_date_summary(&repo, store));
    }

    // §6 legacy: a remote written only by pre-protocol clients keeps the
    // current line-merge behavior.
    let theirs_tree = repo.find_commit(theirs)?.tree()?;
    crate::core::resource_sync::validate_tree(&repo, &theirs_tree)?;
    crate::core::resource_sync::check_writers(&repo, base, ours)?;
    crate::core::resource_sync::check_writers(&repo, base, theirs)?;
    if theirs_tree
        .get_path(Path::new(protocol::PROTOCOL_FILE_REL))
        .is_err()
    {
        if repo
            .find_commit(ours)?
            .tree()?
            .get_name(".agents-manager")
            .is_some()
            || theirs_tree.get_name(".agents-manager").is_some()
        {
            bail!("Expanded resource libraries require the compatible object merge engine; upgrade the other backup clients first");
        }
        log::info!("object merge: remote is pre-protocol (legacy), falling back to git merge");
        git_backup::merge_branch_system(skills_dir, &branch)?;
        let pending_total = rebuild_pending_projection(&repo, store).unwrap_or(0);
        return Ok(MergeSummary {
            engine: "system".to_string(),
            legacy_fallback: true,
            pending_total,
            ..Default::default()
        });
    }

    // §6 mixed-write detection on both sides.
    let old_client_warning = check_old_client_writes(&repo, base, ours, theirs)?;

    let base_tree = repo.find_commit(base)?.tree()?;
    let ours_tree = repo.find_commit(ours)?.tree()?;
    crate::core::resource_sync::validate_tree(&repo, &ours_tree)?;
    let base_snap =
        snapshot::read_snapshot(&repo, &base_tree).context("failed to read base snapshot")?;
    let ours_snap =
        snapshot::read_snapshot(&repo, &ours_tree).context("failed to read local snapshot")?;
    let theirs_snap =
        snapshot::read_snapshot(&repo, &theirs_tree).context("failed to read remote snapshot")?;
    let resources = crate::core::resource_sync::merge(&base_snap, &ours_snap, &theirs_snap);

    // §4/§11-4: declared-pending set from trailers.
    let pending::PendingPins { pinned, reflagged } =
        pending::effective_pending(&repo, base, ours, theirs, &ours_snap, &theirs_snap)?;

    // ff guard (§4): fast-forward unless theirs touches a locally-declared
    // pending skill.
    if base == ours {
        let ff_blocked = pinned
            .iter()
            .any(|(id, side)| *side == Side::Ours && skill_differs(&ours_snap, &theirs_snap, id));
        if !ff_blocked {
            return apply_fast_forward(
                store,
                &repo,
                &branch,
                ours,
                theirs,
                &ours_snap,
                &theirs_snap,
                old_client_warning,
            );
        }
        log::info!("object merge: ff blocked (remote touches pending skills), full merge");
    }

    // §3 decision.
    let ours_touch = touch_info_map(&repo, ours, base)?;
    let theirs_touch = touch_info_map(&repo, theirs, base)?;
    let ours_touch_simple: BTreeMap<String, Touch> = ours_touch
        .iter()
        .map(|(p, t)| (p.clone(), (t.time, t.commit.clone())))
        .collect();
    let theirs_touch_simple: BTreeMap<String, Touch> = theirs_touch
        .iter()
        .map(|(p, t)| (p.clone(), (t.time, t.commit.clone())))
        .collect();
    let mut plan = decision::decide(&DecisionInput {
        base: &base_snap,
        ours: &ours_snap,
        theirs: &theirs_snap,
        pinned: &pinned,
        ours_touch: &ours_touch_simple,
        theirs_touch: &theirs_touch_simple,
    })?;
    // Replace residual newest-wins decisions for whole instruction bundles/MCPs.
    plan.residual.retain(|path, _| {
        !path.starts_with(".agents-manager/instructions/")
            && !path.starts_with(".agents-manager/mcps/")
    });
    plan.residual.extend(resources.files);
    // SID-104: presets renamed apart from a same-name preset wait for review.
    if !plan.same_name.is_empty() {
        let path = format!("{METADATA_DIR}/{}", sync_metadata::PRESET_REVIEW_FILE);
        let mut review: sync_metadata::PresetReviewFile = plan
            .residual
            .get(&path)
            .and_then(|entry| repo.find_blob(entry.oid).ok())
            .and_then(|blob| serde_json::from_slice(blob.content()).ok())
            .unwrap_or_default();
        review.schema_version = 1;
        review.same_name.extend(plan.same_name.clone());
        let oid = repo.blob(&sync_metadata::canonical_json_bytes(&review)?)?;
        plan.residual.insert(
            path,
            FileEntry {
                oid,
                mode: 0o100644,
            },
        );
    }
    // SID-108: a skill both sides flagged with different versions keeps ours
    // and is flagged again against the newest remote version.
    for id in reflagged {
        if !plan.new_conflicts.iter().any(|c| c.skill_id == id) {
            plan.kept_local.push(id.clone());
            plan.new_conflicts.push(NewConflict {
                theirs_path: theirs_snap.skills.get(&id).map(|s| s.meta.path.clone()),
                skill_id: id,
            });
        }
    }

    // §5 steps 4–5: build + validate the merged tree. Skill dirs that were
    // already unclaimed in either input tip (legacy dirt from old versions)
    // are grandfathered — the validator only rejects orphans the merge
    // itself would introduce.
    let edits = plan_to_edits(&repo, &ours_snap, &theirs_snap, &plan)?;
    let merged_tree_oid = apply_tree_edits(&repo, Some(&ours_tree), &edits)?;
    let merged_tree = repo.find_tree(merged_tree_oid)?;
    let mut tolerated = validate::unclaimed_skill_dirs(&repo, &ours_tree)?;
    tolerated.extend(validate::unclaimed_skill_dirs(&repo, &theirs_tree)?);
    validate_merged_tree(&repo, &merged_tree, &tolerated)
        .context("object merge aborted (zero changes)")?;
    crate::core::resource_sync::validate_tree(&repo, &merged_tree)?;
    let merged_snap =
        snapshot::read_snapshot(&repo, &merged_tree).context("failed to read merged snapshot")?;
    let (updated, presets) = library_changes(&repo, &ours_snap, &merged_snap, &theirs_touch);

    // §5 忽略文件注: paths the checkout would create must not be shadowed by
    // untracked/ignored files on disk — explicit error, never a silent FORCE.
    let blockers = blocking_workdir_paths(&repo, skills_dir, &ours_tree, &merged_tree)?;
    if !blockers.is_empty() {
        bail!(
            "sync blocked: untracked or ignored files are in the way of incoming skills: {} — move or delete them and sync again",
            blockers.join(", ")
        );
    }

    // Trailer cap (§4): overflow blocks the automatic path.
    let conflict_ids: Vec<String> = plan
        .new_conflicts
        .iter()
        .map(|c| c.skill_id.clone())
        .collect();
    let trailer = protocol::format_conflicts_trailer(&conflict_ids);
    if let Some((_, overflowed)) = &trailer {
        if *overflowed {
            bail!(
                "SYNC_CONFLICT: {} conflicting skills in one merge exceeds the automatic limit; resolve some conflicts manually first",
                conflict_ids.len()
            );
        }
    }

    // §5 steps 6–8: safety refs, staging refs, merge commit.
    pending::write_ref(&repo, REF_PRE_MERGE, ours, "pre-merge safety point")?;
    let attempt_id = uuid::Uuid::now_v7().to_string();
    for conflict in &plan.new_conflicts {
        pending::write_ref(
            &repo,
            &pending::staging_ref(&attempt_id, &conflict.skill_id),
            theirs,
            "conflict staging",
        )?;
    }

    let mut message = format!(
        "sync: merge remote skill changes ({} updated, {} kept local, {} conflicts)",
        plan.updated_from_theirs.len(),
        plan.kept_local.len(),
        plan.new_conflicts.len()
    );
    // SID-104: what the newer side replaced, so the history shows it.
    if !plan.replaced.is_empty() {
        message.push_str(&format!(
            " · newer change kept: {}",
            plan.replaced.join(", ")
        ));
    }
    message = protocol::app_commit_message(&message);
    if let Some((line, _)) = &trailer {
        message.push('\n');
        message.push_str(line);
    }
    if let Some(line) = crate::core::resource_sync::conflicts_trailer(&resources.conflicts) {
        message.push('\n');
        message.push_str(&line);
    }
    let sig = repo
        .signature()
        .or_else(|_| git2::Signature::now("Skills Manager", "skills-manager@local"))?;
    let ours_commit = repo.find_commit(ours)?;
    let theirs_commit = repo.find_commit(theirs)?;
    let merge_commit = repo.commit(
        None,
        &sig,
        &sig,
        &message,
        &merged_tree,
        &[&ours_commit, &theirs_commit],
    )?;

    // §5 steps 7'–11: applying marker, branch CAS, checkout, promote.
    finish_apply(
        &repo,
        &branch,
        ours,
        merge_commit,
        &merged_tree,
        &attempt_id,
    )?;

    // §4 钉住: a still-active pending's conflict ref offers the version the
    // tree did not keep. The pinned side holds the kept version, so the ref
    // moves to the other side's tip only when that side has a different one.
    // A host that adopted the kept version must not replace the offered one.
    for id in &plan.still_pending {
        let (other, kept_snap, other_snap) = match pinned.get(id) {
            Some(Side::Theirs) => (ours, &theirs_snap, &ours_snap),
            _ => (theirs, &ours_snap, &theirs_snap),
        };
        if skill_differs(kept_snap, other_snap, id) {
            pending::write_ref(&repo, &conflict_ref(id), other, "advance conflict ref")?;
        }
    }

    let pending_total = rebuild_pending_projection(&repo, store)?;
    let summary = MergeSummary {
        engine: "object".to_string(),
        updated,
        presets,
        kept_local: plan.kept_local.clone(),
        new_conflicts: plan
            .new_conflicts
            .iter()
            .map(|c| c.skill_id.clone())
            .collect(),
        pending_total,
        old_client_warning,
        ..Default::default()
    };
    log::info!(
        "object merge: done ({} updated, {} kept local, {} new conflicts, {} pending)",
        summary.updated.len(),
        summary.kept_local.len(),
        summary.new_conflicts.len(),
        summary.pending_total
    );
    Ok(summary)
}

/// Shared tail of merge and fast-forward (§5 steps 7'–11): write the
/// applying marker, move the branch ref (CAS against the expected old
/// head), force the working tree onto the new commit (pre-checked — clean
/// tree, no blockers), then promote staging refs and clear the marker.
fn finish_apply(
    repo: &Repository,
    branch: &str,
    old_head: Oid,
    new_head: Oid,
    new_tree: &git2::Tree,
    attempt_id: &str,
) -> Result<()> {
    pending::write_ref(repo, REF_APPLYING, new_head, "object merge applying")?;

    let branch_ref = format!("refs/heads/{branch}");
    repo.reference_matching(
        &branch_ref,
        new_head,
        true,
        old_head,
        "skills-manager: object merge",
    )
    .context("branch moved while merging; aborting with no changes")?;

    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    if let Err(e) = repo.checkout_tree(new_tree.as_object(), Some(&mut checkout)) {
        // Roll the ref back and restore the (possibly partially checked-out)
        // working tree. The applying marker is only cleared once the
        // rollback checkout succeeded — otherwise it stays so the startup
        // recovery settles the working tree (§5 恢复协议).
        let _ = repo.reference(&branch_ref, old_head, true, "object merge rollback");
        let rolled_back = repo
            .find_commit(old_head)
            .and_then(|c| c.tree())
            .and_then(|old_tree| {
                let mut co = git2::build::CheckoutBuilder::new();
                co.force();
                repo.checkout_tree(old_tree.as_object(), Some(&mut co))
            });
        match rolled_back {
            Ok(()) => pending::delete_ref(repo, REF_APPLYING),
            Err(rollback_err) => log::error!(
                "object merge: rollback checkout also failed, leaving applying marker for startup recovery: {rollback_err}"
            ),
        }
        return Err(anyhow::Error::from(e).context("checkout of merged tree failed; rolled back"));
    }

    pending::promote_staging(repo, attempt_id)?;
    pending::delete_ref(repo, REF_APPLYING);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_fast_forward(
    store: &SkillStore,
    repo: &Repository,
    branch: &str,
    ours: Oid,
    theirs: Oid,
    ours_snap: &Snapshot,
    theirs_snap: &Snapshot,
    old_client_warning: Option<String>,
) -> Result<MergeSummary> {
    let skills_dir = repo
        .workdir()
        .context("skills repository has no working directory")?;
    let ours_tree = repo.find_commit(ours)?.tree()?;
    let theirs_tree = repo.find_commit(theirs)?.tree()?;

    let blockers = blocking_workdir_paths(repo, skills_dir, &ours_tree, &theirs_tree)?;
    if !blockers.is_empty() {
        bail!(
            "sync blocked: untracked or ignored files are in the way of incoming skills: {} — move or delete them and sync again",
            blockers.join(", ")
        );
    }

    pending::write_ref(repo, REF_PRE_MERGE, ours, "pre-merge safety point")?;
    let attempt_id = uuid::Uuid::now_v7().to_string(); // no staging refs; promote is a no-op
    finish_apply(repo, branch, ours, theirs, &theirs_tree, &attempt_id)?;

    // The remote history may itself declare or resolve conflicts — make the
    // durable refs match HEAD's trailer state.
    pending::heal_conflict_refs(repo, theirs)?;
    let pending_total = rebuild_pending_projection(repo, store)?;

    let theirs_touch = touch_info_map(repo, theirs, ours)?;
    let (updated, presets) = library_changes(repo, ours_snap, theirs_snap, &theirs_touch);

    Ok(MergeSummary {
        engine: "object".to_string(),
        fast_forward: true,
        updated,
        presets,
        pending_total,
        old_client_warning,
        ..Default::default()
    })
}

fn up_to_date_summary(repo: &Repository, store: &SkillStore) -> MergeSummary {
    let pending_total = rebuild_pending_projection(repo, store).unwrap_or(0);
    MergeSummary {
        engine: "object".to_string(),
        up_to_date: true,
        pending_total,
        ..Default::default()
    }
}

// ── §6 old-client detection ──

struct Violation {
    sha: String,
    author: String,
    time: String,
    double_parent: bool,
}

fn check_old_client_writes(
    repo: &Repository,
    base: Oid,
    ours: Oid,
    theirs: Oid,
) -> Result<Option<String>> {
    let ours_violations = scan_old_client(repo, base, ours)?;
    let theirs_violations = scan_old_client(repo, base, theirs)?;
    if ours_violations.is_empty() && theirs_violations.is_empty() {
        return Ok(None);
    }

    let describe = |v: &Violation| format!("{} ({}, {})", v.sha, v.author, v.time);
    let doubles: Vec<String> = ours_violations
        .iter()
        .chain(&theirs_violations)
        .filter(|v| v.double_parent)
        .map(describe)
        .collect();
    if !doubles.is_empty() {
        bail!(
            "sync blocked: an old skills-manager version performed a line-level merge on another device — commit(s) {}. Upgrade that device and retry, or use the recovery flow (a safety snapshot is taken first)",
            doubles.join("; ")
        );
    }

    // Single-parent old-client writes: allow if the affected side's tip still
    // passes structural validation and no SKILL.md carries conflict markers.
    for (label, tip, violated) in [
        ("local", ours, !ours_violations.is_empty()),
        ("remote", theirs, !theirs_violations.is_empty()),
    ] {
        if !violated {
            continue;
        }
        let tree = repo.find_commit(tip)?.tree()?;
        // Legacy unclaimed dirs and committed metadata junk in the tip are
        // tolerated here for the same reason as in the merge itself; the
        // structural checks still apply.
        let tolerated = validate::unclaimed_skill_dirs(repo, &tree)?;
        validate::validate_input_tip(repo, &tree, &tolerated).with_context(|| {
            format!(
                "sync blocked: an old skills-manager version wrote to the {label} library and left it inconsistent — upgrade that device and repair via the recovery flow"
            )
        })?;
        let snap = snapshot::read_snapshot(repo, &tree)?;
        let markers = scan_conflict_markers(repo, &snap)?;
        if !markers.is_empty() {
            bail!(
                "sync blocked: conflict markers found in {} (written by an old version): {}",
                label,
                markers.join(", ")
            );
        }
    }

    let listed: Vec<String> = ours_violations
        .iter()
        .chain(&theirs_violations)
        .take(3)
        .map(describe)
        .collect();
    Ok(Some(format!(
        "old skills-manager version wrote commit(s) {} — please upgrade that device",
        listed.join("; ")
    )))
}

fn scan_old_client(repo: &Repository, base: Oid, tip: Oid) -> Result<Vec<Violation>> {
    let mut walk = repo.revwalk()?;
    walk.push(tip)?;
    walk.hide(base)?;
    let mut out = Vec::new();
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        let tree = commit.tree()?;
        let has_marker = tree
            .get_path(Path::new(protocol::PROTOCOL_FILE_REL))
            .is_ok();
        let has_trailer = protocol::has_protocol_trailer(commit.message().unwrap_or_default());
        if has_marker && !has_trailer {
            out.push(Violation {
                sha: commit.id().to_string()[..7].to_string(),
                author: commit.author().name().unwrap_or("unknown").to_string(),
                time: chrono::DateTime::from_timestamp(commit.time().seconds(), 0)
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default(),
                double_parent: commit.parent_count() > 1,
            });
        }
    }
    Ok(out)
}

fn scan_conflict_markers(repo: &Repository, snap: &Snapshot) -> Result<Vec<String>> {
    let mut hits = Vec::new();
    for skill in snap.skills.values() {
        let Some(content) = skill.content else {
            continue;
        };
        let tree = repo.find_tree(content)?;
        for marker in snapshot::SKILL_DIR_MARKERS {
            if let Some(entry) = tree.get_name(marker) {
                if let Ok(blob) = repo.find_blob(entry.id()) {
                    let content = blob.content();
                    if content.starts_with(b"<<<<<<< ")
                        || content.windows(9).any(|w| w == b"\n<<<<<<< ")
                    {
                        hits.push(format!("{}/{}", skill.meta.path, marker));
                    }
                }
            }
        }
    }
    Ok(hits)
}

/// Whether two snapshots hold different versions of a skill, counting a
/// deletion as a version.
fn skill_differs(a: &Snapshot, b: &Snapshot, id: &str) -> bool {
    match (a.skills.get(id), b.skills.get(id)) {
        (Some(a), Some(b)) => !skill_identical(a, b),
        (None, None) => false,
        _ => true,
    }
}

// ── last-touch attribution ──

/// Newest-first per-path info over `hide..tip`: which commit last touched a
/// path, when, and by whom. Merge commits are diffed against their first
/// parent (attribution approximation).
fn touch_info_map(repo: &Repository, tip: Oid, hide: Oid) -> Result<BTreeMap<String, TouchInfo>> {
    let mut walk = repo.revwalk()?;
    walk.push(tip)?;
    walk.hide(hide)?;
    walk.set_sorting(Sort::TOPOLOGICAL)?;
    let mut out: BTreeMap<String, TouchInfo> = BTreeMap::new();
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        let tree = commit.tree()?;
        let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
        let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
        for delta in diff.deltas() {
            for file in [delta.new_file(), delta.old_file()] {
                let Some(path) = file.path().and_then(|p| p.to_str()) else {
                    continue;
                };
                out.entry(path.to_string()).or_insert_with(|| TouchInfo {
                    time: commit.time().seconds(),
                    commit: commit.id().to_string(),
                    author: commit.author().name().unwrap_or("").to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// Author of the newest remote commit that touched `file`, or else anything
/// under `dir`.
fn device_for(touch: &BTreeMap<String, TouchInfo>, file: &str, dir: &str) -> String {
    let prefix = format!("{dir}/");
    touch
        .get(file)
        .or_else(|| {
            touch
                .iter()
                .find(|(p, _)| p.as_str() == dir || p.starts_with(&prefix))
                .map(|(_, info)| info)
        })
        .map(|info| info.author.clone())
        .unwrap_or_default()
}

/// What a sync changed in the local library: every skill and preset that
/// differs between the tree before and after it (SID-99).
fn library_changes(
    repo: &Repository,
    before: &Snapshot,
    after: &Snapshot,
    touch: &BTreeMap<String, TouchInfo>,
) -> (Vec<UpdatedSkill>, Vec<UpdatedPreset>) {
    let skill_ids: BTreeSet<&String> = before.skills.keys().chain(after.skills.keys()).collect();
    let skills = skill_ids
        .into_iter()
        .filter_map(|id| {
            let (kind, path) = match (before.skills.get(id), after.skills.get(id)) {
                (None, Some(a)) => (ChangeKind::Added, &a.meta.path),
                (Some(b), Some(a)) if !skill_identical(b, a) => (ChangeKind::Changed, &a.meta.path),
                (Some(b), None) => (ChangeKind::Removed, &b.meta.path),
                _ => return None,
            };
            Some(UpdatedSkill {
                skill_id: id.clone(),
                path: path.clone(),
                from_device: device_for(touch, &skill_meta_path(id), path),
                kind,
            })
        })
        .collect();

    let members = |snap: &Snapshot, preset: &str| -> Vec<(String, FileEntry)> {
        snap.memberships
            .iter()
            .filter(|((sid, _), _)| sid == preset)
            .map(|((_, skill), entry)| (skill.clone(), *entry))
            .collect()
    };
    let preset_ids: BTreeSet<&String> = before
        .scenarios
        .keys()
        .chain(after.scenarios.keys())
        .collect();
    let presets = preset_ids
        .into_iter()
        .filter_map(|id| {
            let (kind, entry) = match (before.scenarios.get(id), after.scenarios.get(id)) {
                (None, Some(a)) => (ChangeKind::Added, a),
                (Some(b), Some(a)) if b != a || members(before, id) != members(after, id) => {
                    (ChangeKind::Changed, a)
                }
                (Some(b), None) => (ChangeKind::Removed, b),
                _ => return None,
            };
            let name = repo
                .find_blob(entry.oid)
                .ok()
                .and_then(|blob| {
                    serde_json::from_slice::<crate::core::sync_metadata::ScenarioMetaFile>(
                        blob.content(),
                    )
                    .ok()
                })
                .map(|meta| meta.name)
                .unwrap_or_default();
            Some(UpdatedPreset {
                preset_id: id.clone(),
                name,
                from_device: device_for(
                    touch,
                    &format!("{METADATA_DIR}/scenarios/{id}.json"),
                    &format!("{METADATA_DIR}/scenario-skills/{id}"),
                ),
                kind,
            })
        })
        .collect();
    (skills, presets)
}

// ── plan → tree edits ──

fn plan_to_edits(
    repo: &Repository,
    ours: &Snapshot,
    theirs: &Snapshot,
    plan: &MergePlan,
) -> Result<BTreeMap<String, TreeEdit>> {
    let mut edits: BTreeMap<String, TreeEdit> = BTreeMap::new();

    // Removals first; puts inserted afterwards overwrite same-path removes.
    for (id, ours_skill) in &ours.skills {
        let keep = plan.skills.get(id);
        let path_kept = keep
            .map(|p| p.meta.path == ours_skill.meta.path)
            .unwrap_or(false);
        if !path_kept {
            edits.insert(ours_skill.meta.path.clone(), TreeEdit::Remove);
        }
        if keep.is_none() {
            edits.insert(skill_meta_path(id), TreeEdit::Remove);
        }
    }
    for (id, planned) in &plan.skills {
        let content = planned
            .content
            .with_context(|| format!("skill {id} has no content directory to merge"))?;
        let ours_skill = ours.skills.get(id);
        let same_content_in_place = ours_skill
            .map(|o| o.meta.path == planned.meta.path && o.content == Some(content))
            .unwrap_or(false);
        if !same_content_in_place {
            edits.insert(
                planned.meta.path.clone(),
                TreeEdit::PutTree { oid: content },
            );
        }
        let bytes = sync_metadata::canonical_json_bytes(&planned.meta)?;
        let blob = repo.blob(&bytes)?;
        let unchanged = ours_skill
            .map(|o| o.meta_entry.oid == blob)
            .unwrap_or(false);
        if !unchanged {
            edits.insert(
                skill_meta_path(id),
                TreeEdit::PutBlob {
                    oid: blob,
                    mode: 0o100644,
                },
            );
        }
    }

    let scenarios = write_planned::<_, sync_metadata::ScenarioMetaFile>(repo, &plan.scenarios)?;
    let memberships =
        write_planned::<_, sync_metadata::ScenarioSkillMetaFile>(repo, &plan.memberships)?;
    let agents = write_planned::<_, sync_metadata::AgentMetaFile>(repo, &plan.agents)?;
    diff_file_maps(&mut edits, &ours.agents, &agents, |key| {
        format!("{METADATA_DIR}/agents/{key}.json")
    });
    diff_file_maps(&mut edits, &ours.scenarios, &scenarios, |id| {
        format!("{METADATA_DIR}/scenarios/{id}.json")
    });
    diff_file_maps(
        &mut edits,
        &ours.memberships,
        &memberships,
        |(sid, skid)| format!("{METADATA_DIR}/scenario-skills/{sid}/{skid}.json"),
    );
    diff_file_maps(&mut edits, &ours.residual, &plan.residual, |p| p.clone());

    // protocol.json: larger merge_protocol wins; ties resolve to the smaller
    // blob OID so both devices pick the same bytes (§3).
    let protocol_pick = pick_versioned(
        ours.protocol
            .as_ref()
            .map(|(e, p)| (*e, u64::from(p.merge_protocol))),
        theirs
            .protocol
            .as_ref()
            .map(|(e, p)| (*e, u64::from(p.merge_protocol))),
    );
    match protocol_pick {
        Some(entry) => {
            if ours.protocol.as_ref().map(|(e, _)| *e) != Some(entry) {
                edits.insert(
                    protocol::PROTOCOL_FILE_REL.to_string(),
                    TreeEdit::PutBlob {
                        oid: entry.oid,
                        mode: entry.mode,
                    },
                );
            }
        }
        None => {
            let bytes = protocol::protocol_file_bytes(&ProtocolFile::default())?;
            let blob = repo.blob(&bytes)?;
            edits.insert(
                protocol::PROTOCOL_FILE_REL.to_string(),
                TreeEdit::PutBlob {
                    oid: blob,
                    mode: 0o100644,
                },
            );
        }
    }
    // schema.json: larger schema_version wins, same tie-break.
    if let Some(entry) = pick_versioned(ours.schema, theirs.schema) {
        if ours.schema.map(|(e, _)| e) != Some(entry) {
            edits.insert(
                format!("{METADATA_DIR}/schema.json"),
                TreeEdit::PutBlob {
                    oid: entry.oid,
                    mode: entry.mode,
                },
            );
        }
    }

    Ok(edits)
}

fn skill_meta_path(id: &str) -> String {
    format!("{METADATA_DIR}/skills/{id}.json")
}

/// Blobs for the planned preset and membership files. Merged ones are written
/// in the app's own format, so the next export leaves them as they are.
fn write_planned<K: Ord + Clone, T: serde::Serialize + serde::de::DeserializeOwned>(
    repo: &Repository,
    planned: &BTreeMap<K, PlannedFile>,
) -> Result<BTreeMap<K, FileEntry>> {
    planned
        .iter()
        .map(|(key, file)| {
            let entry = match file {
                PlannedFile::Blob(entry) => *entry,
                PlannedFile::Merged(value) => {
                    let typed: T = serde_json::from_value(value.clone())
                        .context("a merged preset file is not in the expected format")?;
                    let bytes = sync_metadata::canonical_json_bytes(&typed)?;
                    FileEntry {
                        oid: repo.blob(&bytes)?,
                        mode: 0o100644,
                    }
                }
            };
            Ok((key.clone(), entry))
        })
        .collect()
}

fn diff_file_maps<K: Ord>(
    edits: &mut BTreeMap<String, TreeEdit>,
    ours: &BTreeMap<K, FileEntry>,
    planned: &BTreeMap<K, FileEntry>,
    to_path: impl Fn(&K) -> String,
) {
    for key in ours.keys() {
        if !planned.contains_key(key) {
            edits.insert(to_path(key), TreeEdit::Remove);
        }
    }
    for (key, entry) in planned {
        if ours.get(key) != Some(entry) {
            edits.insert(
                to_path(key),
                TreeEdit::PutBlob {
                    oid: entry.oid,
                    mode: entry.mode,
                },
            );
        }
    }
}

fn pick_versioned(
    ours: Option<(FileEntry, u64)>,
    theirs: Option<(FileEntry, u64)>,
) -> Option<FileEntry> {
    match (ours, theirs) {
        (None, None) => None,
        (Some((e, _)), None) => Some(e),
        (None, Some((e, _))) => Some(e),
        (Some((oe, ov)), Some((te, tv))) => {
            if ov > tv {
                Some(oe)
            } else if tv > ov {
                Some(te)
            } else if oe == te || oe.oid.as_bytes() <= te.oid.as_bytes() {
                Some(oe)
            } else {
                Some(te)
            }
        }
    }
}

// ── checkout pre-check (§5 忽略文件注 / §11-6) ──

/// Paths the target tree adds relative to `ours` that already exist on disk
/// (necessarily untracked or ignored — tracked paths live in `ours`). A
/// FORCE checkout would silently overwrite them, so the merge refuses.
fn blocking_workdir_paths(
    repo: &Repository,
    skills_dir: &Path,
    ours_tree: &git2::Tree,
    target_tree: &git2::Tree,
) -> Result<Vec<String>> {
    let diff = repo.diff_tree_to_tree(Some(ours_tree), Some(target_tree), None)?;
    let mut blockers = Vec::new();
    for delta in diff.deltas() {
        if delta.status() != git2::Delta::Added {
            continue;
        }
        let Some(path) = delta.new_file().path() else {
            continue;
        };
        if skills_dir.join(path).exists() {
            blockers.push(path.to_string_lossy().to_string());
        }
    }
    Ok(blockers)
}

// ── pending projection (§4 SQLite 投影) ──

/// Rebuild the pending_conflicts table from HEAD's trailer state plus the
/// conflict refs. Returns the number of active pendings.
pub fn rebuild_pending_projection(repo: &Repository, store: &SkillStore) -> Result<usize> {
    let head = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .context("repository has no HEAD commit")?;
    let active: Vec<String> = pending::replay_trailers(repo, head, None)?
        .into_iter()
        .filter(|(_, s)| *s == TrailerState::Active)
        .map(|(id, _)| id)
        .collect();

    let existing: BTreeMap<String, PendingConflictRow> = store
        .list_pending_conflicts()?
        .into_iter()
        .map(|row| (row.skill_id.clone(), row))
        .collect();

    let mut rows = Vec::new();
    for id in active {
        let Some(theirs_commit) = pending::ref_target(repo, &conflict_ref(&id)) else {
            // No pinned object (e.g. ref lost and unreconstructable): the
            // trailer still marks it pending; surface without a commit.
            rows.push(PendingConflictRow {
                skill_id: id.clone(),
                theirs_commit: String::new(),
                theirs_path: existing.get(&id).and_then(|r| r.theirs_path.clone()),
                detected_at: existing
                    .get(&id)
                    .map(|r| r.detected_at)
                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
            });
            continue;
        };
        let theirs_path = theirs_skill_path(repo, theirs_commit, &id);
        rows.push(PendingConflictRow {
            skill_id: id.clone(),
            theirs_commit: theirs_commit.to_string(),
            theirs_path,
            detected_at: existing
                .get(&id)
                .map(|r| r.detected_at)
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
        });
    }
    let total = rows.len();
    store.replace_pending_conflicts(&rows)?;
    Ok(total)
}

/// The skill's path inside a pinned theirs commit, for display.
pub(crate) fn theirs_skill_path(repo: &Repository, commit: Oid, skill_id: &str) -> Option<String> {
    let tree = repo.find_commit(commit).ok()?.tree().ok()?;
    let entry = tree.get_path(Path::new(&skill_meta_path(skill_id))).ok()?;
    let blob = repo.find_blob(entry.id()).ok()?;
    let meta: crate::core::sync_metadata::SkillMetaFile =
        serde_json::from_slice(blob.content()).ok()?;
    Some(meta.path)
}

// ── startup recovery (§5 启动恢复协议) ──

/// Best-effort crash recovery, run once at startup before background work.
/// Never blocks startup: a busy lock or an error only logs.
pub fn recover_on_startup(store: &SkillStore, skills_dir: &Path) {
    if !skills_dir.join(".git").exists() {
        return;
    }
    let Ok(_lock) = RepoLock::acquire("merge recovery") else {
        return;
    };
    match recover_locked(store, skills_dir) {
        Ok(()) => {}
        Err(e) => log::warn!("merge recovery: {e:#}"),
    }
}

fn recover_locked(store: &SkillStore, skills_dir: &Path) -> Result<()> {
    let repo = Repository::open(skills_dir)?;
    let head = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .context("repository has no HEAD commit")?;

    if let Some(applying) = pending::ref_target(&repo, REF_APPLYING) {
        let old_head = pending::ref_target(&repo, REF_PRE_MERGE);
        match old_head {
            Some(old_head) if head == old_head => {
                // Crash between marker and branch move, or a crashed
                // rollback: the merge never took effect. The working tree
                // may still carry a partial checkout from the failed
                // attempt — settle it back onto HEAD (rescuing anything
                // that differs; debris and user edits made since the crash
                // cannot be told apart, so both are kept safe).
                log::info!("merge recovery: apply never landed, cleaning up");
                settle_worktree(&repo, old_head)?;
                pending::delete_ref(&repo, REF_APPLYING);
                pending::delete_ref(&repo, REF_PRE_MERGE);
                for (attempt, skill_id, _) in pending::list_staging_refs(&repo) {
                    pending::delete_ref(&repo, &pending::staging_ref(&attempt, &skill_id));
                }
            }
            Some(old_head) if head == applying => {
                // Crash between branch move and checkout/cleanup: replay.
                log::info!("merge recovery: replaying interrupted apply");
                replay_interrupted_apply(&repo, old_head, head)?;
            }
            _ => {
                let msg = "merge recovery: HEAD does not match either side of an interrupted sync; not touching anything — use the backup page recovery flow";
                log::error!("{msg}");
                let _ = store.set_setting(crate::core::auto_backup::SETTING_LAST_ERROR, msg);
                return Ok(());
            }
        }
    } else {
        // No interrupted apply: heal refs against HEAD trailers and clean
        // ghost staging refs; drop a stale pre-merge safety ref.
        pending::heal_conflict_refs(&repo, head)?;
        if let Some(pre) = pending::ref_target(&repo, REF_PRE_MERGE) {
            if let Ok(commit) = repo.find_commit(pre) {
                let age_days = (chrono::Utc::now().timestamp() - commit.time().seconds()) / 86_400;
                if age_days > pending::PRE_MERGE_RETENTION_DAYS {
                    pending::delete_ref(&repo, REF_PRE_MERGE);
                }
            }
        }
    }

    rebuild_pending_projection(&repo, store)?;
    Ok(())
}

/// HEAD already points at the merge commit but the working tree may still be
/// the old one (§5 v3-R3 finding 2/3): if the tree was touched since the
/// crash, snapshot it to a rescue commit + user-visible tag first; then
/// finish checkout and step-11 cleanup.
fn replay_interrupted_apply(repo: &Repository, old_head: Oid, head: Oid) -> Result<()> {
    settle_worktree_from(repo, old_head, head)?;
    pending::heal_conflict_refs(repo, head)?;
    pending::delete_ref(repo, REF_APPLYING);
    Ok(())
}

/// Settle the working tree onto `target` after a crash: an untouched tree
/// (still exactly `expected_clean`, ignoring unrelated untracked files) is
/// force-checked-out directly; anything else — user edits or debris from a
/// partial checkout — is preserved first in a rescue commit + user-visible
/// snapshot tag. Never silently overwrites (§5 恢复不吞用户数据).
fn settle_worktree_from(repo: &Repository, expected_clean: Oid, target: Oid) -> Result<()> {
    let expected_tree = repo.find_commit(expected_clean)?.tree()?;
    let target_commit = repo.find_commit(target)?;
    let target_tree = target_commit.tree()?;

    if !worktree_matches_tree(repo, &expected_tree, &target_tree)? {
        let sig = repo
            .signature()
            .or_else(|_| git2::Signature::now("Skills Manager", "skills-manager@local"))?;
        let mut index = repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        let rescue_tree = repo.find_tree(index.write_tree()?)?;
        let rescue = repo.commit(
            None,
            &sig,
            &sig,
            &protocol::app_commit_message("rescue: working tree changed during interrupted sync"),
            &rescue_tree,
            &[&target_commit],
        )?;
        let tag = format!(
            "sm-v-{}-{}",
            chrono::Utc::now().format("%Y%m%d-%H%M%S"),
            &rescue.to_string()[..7]
        );
        repo.tag_lightweight(&tag, &repo.find_object(rescue, None)?, true)?;
        log::warn!("merge recovery: working tree preserved in rescue snapshot {tag}");
    }

    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    repo.checkout_tree(target_tree.as_object(), Some(&mut checkout))?;
    Ok(())
}

fn settle_worktree(repo: &Repository, target: Oid) -> Result<()> {
    settle_worktree_from(repo, target, target)
}

/// Whether the working tree exactly matches `expected`, considering only
/// paths that the replayed checkout would touch (union of both trees, §11-6)
/// so unrelated untracked/ignored files don't trigger a rescue snapshot.
fn worktree_matches_tree(
    repo: &Repository,
    expected: &git2::Tree,
    target: &git2::Tree,
) -> Result<bool> {
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(true)
        .recurse_ignored_dirs(true);
    let diff = repo.diff_tree_to_workdir(Some(expected), Some(&mut opts))?;
    for delta in diff.deltas() {
        for file in [delta.new_file(), delta.old_file()] {
            let Some(path) = file.path() else { continue };
            if expected.get_path(path).is_ok() || target.get_path(path).is_ok() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
