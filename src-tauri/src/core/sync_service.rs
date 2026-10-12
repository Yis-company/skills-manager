//! Sync operations shared by the app, the CLI's `sync` group and host
//! dispatch, so each has one implementation (SID-110): sync now, status,
//! conflicts and their resolution, history, restore, pause and resume.

use std::path::Path;

use serde::Serialize;

use super::audit_log::AuditDraft;
use super::error::{AppError, ErrorKind};
use super::git_backup_store::{
    apply_device_identity, effective_device_name, reconcile_skills_index_unlocked,
};
use super::host::HostCtx;
use super::merge::{self, resolve::ResolveAction};
use super::scenario_service::{self, STATUS_EDITED};
use super::skill_store::SkillStore;
use super::{auto_backup, central_repo, git_backup, sync_apply, sync_metadata};

/// Commit message of a `sync now` that finds local changes.
pub const SYNC_COMMIT_MESSAGE: &str = "sync";

/// When this host last synced successfully (ms since the epoch).
pub const SETTING_LAST_SYNC_AT: &str = "backup_last_sync_at";

/// Sync now: finish an interrupted sync, then commit, merge, apply to the
/// agents and push. A failure is recorded for `sync status`; success clears
/// it and stamps the last sync time.
pub fn sync_now(ctx: &HostCtx, message: &str) -> Result<SyncOutcome, AppError> {
    let store = &ctx.store;
    let skills_dir = central_repo::skills_dir();
    merge::recover_on_startup(store, &skills_dir);
    let outcome = git_backup::with_repo_lock("sync now", || {
        run_sync_blocking(store, &skills_dir, message)
    })
    .map_err(|e| {
        let error = AppError::classify_git_error(format!("{e:#}"));
        let _ = store.set_setting(auto_backup::SETTING_LAST_ERROR, &error.message);
        error
    })?;
    record_success(store);
    if outcome.merge.as_ref().is_some_and(|m| !m.up_to_date) {
        sync_apply::reapply_resources(ctx);
    }
    Ok(outcome)
}

/// A host's Sync state (SID-102), as the host itself sees it. "Syncing" and
/// "Update pending" belong to the app's view of a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    NotSetUp,
    UpToDate,
    Paused,
    Offline,
    NeedsReview,
    UpdateRequired,
}

#[derive(Debug, Serialize)]
pub struct SyncStatus {
    pub state: SyncState,
    /// Why the host is stuck, or what waits for review.
    pub reason: Option<String>,
    pub host: String,
    pub remote: Option<String>,
    /// Last successful sync, ms since the epoch.
    pub last_sync: Option<i64>,
    pub conflicts: Vec<SyncConflict>,
    pub same_name_presets: Vec<SameNamePreset>,
    pub edited_here: Vec<EditedCopy>,
    pub version: String,
    /// The Sync format this build understands and writes.
    pub format: u32,
}

/// A skill changed on two hosts (SID-108). The choices are named by host.
#[derive(Debug, Clone, Serialize)]
pub struct SyncConflict {
    pub skill_id: String,
    pub name: String,
    /// The host whose version is in place: the one that recorded the conflict.
    pub this_host: String,
    /// The host whose version the conflict offers instead.
    pub other_host: String,
}

/// A preset renamed apart from another host's preset of the same name
/// (SID-104), waiting for a choice.
#[derive(Debug, Clone, Serialize)]
pub struct SameNamePreset {
    pub preset_id: String,
    pub name: String,
    /// The preset that kept the name.
    pub other_id: String,
    pub other_name: String,
}

/// How to settle a same-name pair: keep both, combine the renamed preset into
/// the other, or keep one preset by id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetChoice {
    KeepBoth,
    Combine,
    Keep(String),
}

/// An agent copy flagged "Edited here" (SID-99).
#[derive(Debug, Serialize)]
pub struct EditedCopy {
    pub skill_id: String,
    pub name: String,
    pub agent: String,
    /// The skill left this agent (rather than changed in Sync).
    pub left: bool,
}

pub fn sync_status(store: &SkillStore) -> SyncStatus {
    let git = git_backup::get_status(&central_repo::skills_dir()).ok();
    let remote = git.as_ref().and_then(|s| s.remote_url.clone());
    let conflicts = conflicts(store);
    let same_name_presets = same_name_presets(store);
    let edited_here = edited_copies(store);
    let last_error = store
        .get_setting(auto_backup::SETTING_LAST_ERROR)
        .ok()
        .flatten()
        .filter(|e| !e.trim().is_empty());
    let (state, reason) = if !git.is_some_and(|s| s.is_repo) || remote.is_none() {
        (SyncState::NotSetUp, None)
    } else if !auto_backup::is_enabled(store) {
        (SyncState::Paused, None)
    } else if let Some(error) = last_error {
        let state = if error.contains(merge::protocol::UPDATE_REQUIRED) {
            SyncState::UpdateRequired
        } else if matches!(
            AppError::classify_git_error(&error).kind,
            ErrorKind::Network
        ) {
            SyncState::Offline
        } else {
            SyncState::NeedsReview
        };
        (state, Some(error))
    } else if !conflicts.is_empty() || !same_name_presets.is_empty() || !edited_here.is_empty() {
        let reason = format!(
            "{} changed on two hosts, {} with the same name, {} edited here",
            plural(conflicts.len(), "skill"),
            plural(same_name_presets.len(), "preset"),
            plural(edited_here.len(), "agent copy"),
        );
        (SyncState::NeedsReview, Some(reason))
    } else {
        (SyncState::UpToDate, None)
    };
    SyncStatus {
        state,
        reason,
        host: effective_device_name(store),
        remote,
        last_sync: store
            .get_setting(SETTING_LAST_SYNC_AT)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok()),
        conflicts,
        same_name_presets,
        edited_here,
        version: env!("CARGO_PKG_VERSION").to_string(),
        format: merge::protocol::MERGE_PROTOCOL_VERSION,
    }
}

fn plural(n: usize, noun: &str) -> String {
    match (n, noun.strip_suffix('y')) {
        (1, _) => format!("1 {noun}"),
        (_, Some(stem)) => format!("{n} {stem}ies"),
        _ => format!("{n} {noun}s"),
    }
}

/// Pending skill conflicts, with the host on each side.
pub fn conflicts(store: &SkillStore) -> Vec<SyncConflict> {
    let repo = git2::Repository::open(central_repo::skills_dir()).ok();
    let head = repo.as_ref().and_then(|repo| repo.head().ok()?.target());
    let author = |commit: &str| -> Option<String> {
        let commit = repo
            .as_ref()?
            .find_commit(git2::Oid::from_str(commit).ok()?)
            .ok()?;
        let name = commit.author().name().map(str::to_owned);
        name
    };
    // A host that pulled the conflict holds the version of the host that
    // recorded it, so that host names the version in place.
    let recorder = |skill_id: &str| -> Option<String> {
        let commit = merge::pending::declaring_commit(repo.as_ref()?, head?, skill_id)?;
        let name = commit.author().name().map(str::to_owned);
        name
    };
    store
        .list_pending_conflicts()
        .unwrap_or_default()
        .into_iter()
        .map(|row| SyncConflict {
            name: store
                .get_skill_by_id(&row.skill_id)
                .ok()
                .flatten()
                .map(|s| s.name)
                .or(row.theirs_path)
                .unwrap_or_else(|| row.skill_id.clone()),
            other_host: author(&row.theirs_commit).unwrap_or_default(),
            this_host: recorder(&row.skill_id).unwrap_or_else(|| effective_device_name(store)),
            skill_id: row.skill_id,
        })
        .collect()
}

fn same_name_presets(store: &SkillStore) -> Vec<SameNamePreset> {
    let names: std::collections::HashMap<String, String> = store
        .get_all_scenarios()
        .unwrap_or_default()
        .into_iter()
        .map(|scenario| (scenario.id, scenario.name))
        .collect();
    sync_metadata::preset_reviews(store)
        .unwrap_or_default()
        .into_iter()
        .map(|(preset_id, other_id)| SameNamePreset {
            name: names.get(&preset_id).cloned().unwrap_or_default(),
            other_name: names.get(&other_id).cloned().unwrap_or_default(),
            preset_id,
            other_id,
        })
        .collect()
}

fn edited_copies(store: &SkillStore) -> Vec<EditedCopy> {
    store
        .get_all_targets()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.status.starts_with(STATUS_EDITED))
        .map(|t| EditedCopy {
            name: store
                .get_skill_by_id(&t.skill_id)
                .ok()
                .flatten()
                .map_or_else(|| t.skill_id.clone(), |s| s.name),
            left: t.status != STATUS_EDITED,
            skill_id: t.skill_id,
            agent: t.tool,
        })
        .collect()
}

/// The resolve action for `--keep <host>` or `--both` on the conflict a skill
/// id or name refers to. Returns the skill id with it.
pub fn resolve_choice(
    store: &SkillStore,
    skill: &str,
    keep: Option<&str>,
    both: bool,
) -> Result<(String, ResolveAction), AppError> {
    let conflict = conflicts(store)
        .into_iter()
        .find(|c| c.skill_id == skill || c.name == skill)
        .ok_or_else(|| AppError::not_found(format!("No conflict for {skill}")))?;
    let action = match keep {
        _ if both => ResolveAction::KeepBoth,
        Some(host) if host.eq_ignore_ascii_case(&conflict.this_host) => ResolveAction::KeepLocal,
        Some(host) if host.eq_ignore_ascii_case(&conflict.other_host) => ResolveAction::UseRemote,
        _ => {
            return Err(AppError::invalid_input(format!(
                "Choose --keep {} or --keep {}, or --both",
                conflict.this_host, conflict.other_host
            )))
        }
    };
    Ok((conflict.skill_id, action))
}

/// Resolve a skill conflict (SID-108) and apply the result.
pub fn resolve(ctx: &HostCtx, skill_id: &str, action: ResolveAction) -> Result<(), AppError> {
    let store = &ctx.store;
    let skills_dir = central_repo::skills_dir();
    git_backup::with_repo_lock("resolve conflict", || {
        apply_device_identity(store, &skills_dir);
        merge::resolve::resolve_conflict_unlocked(store, &skills_dir, skill_id, action)?;
        reconcile_skills_index_unlocked(store)?;
        store.log_audit(
            AuditDraft::new("resolve_conflict")
                .skill(skill_id, skill_id)
                .detail(format!("action={action:?}"))
                .ok(),
        );
        Ok(())
    })
    .map_err(|e| AppError::classify_git_error(format!("{e:#}")))
}

/// Settle a same-name preset pair (SID-104). The choice goes out with the
/// next sync, and the agents follow when the active preset is involved.
pub fn resolve_preset(
    store: &SkillStore,
    preset_id: &str,
    choice: PresetChoice,
) -> Result<(), AppError> {
    let mut reviews = sync_metadata::preset_reviews(store).map_err(AppError::db)?;
    let other = reviews
        .remove(preset_id)
        .ok_or_else(|| AppError::not_found(format!("No same-name review for {preset_id}")))?;
    let (keep, drop) = match &choice {
        PresetChoice::KeepBoth => (None, None),
        PresetChoice::Combine => (Some(other), Some(preset_id.to_string())),
        PresetChoice::Keep(id) if id == preset_id => (Some(id.clone()), Some(other)),
        PresetChoice::Keep(id) if *id == other => (Some(other), Some(preset_id.to_string())),
        PresetChoice::Keep(id) => {
            return Err(AppError::invalid_input(format!(
                "{id} isn't part of this review"
            )))
        }
    };
    let active = store.get_active_scenario_id().map_err(AppError::db)?;
    sync_metadata::with_repo_lock("resolve same-name presets", || {
        if let (Some(keep), Some(drop)) = (&keep, &drop) {
            if choice == PresetChoice::Combine {
                for skill_id in store.get_skill_ids_for_scenario(drop)? {
                    store.add_skill_to_scenario(keep, &skill_id)?;
                }
            }
            if active.as_ref() == Some(drop) {
                store.set_active_scenario(keep)?;
            }
            store.delete_scenario(drop)?;
        }
        sync_metadata::write_preset_reviews_unlocked(&reviews)?;
        sync_metadata::write_all_from_db_unlocked(store)
    })
    .map_err(AppError::db)?;
    match keep {
        Some(keep) if active.is_some() && (active == drop || active.as_ref() == Some(&keep)) => {
            scenario_service::apply_scenario_to_default(store, &keep)
                .and_then(scenario_service::refusals_to_error)
        }
        _ => Ok(()),
    }
}

pub fn history(limit: Option<usize>) -> Result<Vec<git_backup::SyncHistoryEntry>, AppError> {
    git_backup::sync_history(&central_repo::skills_dir(), limit).map_err(AppError::git)
}

/// Restore a commit from the Sync history and apply it to the agents.
pub fn restore(ctx: &HostCtx, point: &str) -> Result<(), AppError> {
    let store = &ctx.store;
    let skills_dir = central_repo::skills_dir();
    git_backup::with_repo_lock("git restore snapshot", || {
        // The safety-point and restore commits are made by this device.
        apply_device_identity(store, &skills_dir);
        git_backup::restore_snapshot_version_unlocked(&skills_dir, point)?;
        reconcile_skills_index_unlocked(store)
    })
    .map_err(AppError::git)?;
    sync_apply::reapply_resources(ctx);
    Ok(())
}

/// What this host did to its agents (SID-99), newest first: the last 200
/// `agent_*` audit entries.
pub fn agent_log(store: &SkillStore) -> Result<Vec<super::audit_log::AuditEntry>, AppError> {
    store
        .list_audit_with_prefix("agent_", 200)
        .map_err(AppError::db)
}

/// Pause or resume automatic sync on this host (`backup_auto_enabled`).
pub fn set_paused(store: &SkillStore, paused: bool) -> Result<(), AppError> {
    store
        .set_setting(
            auto_backup::SETTING_ENABLED,
            if paused { "off" } else { "on" },
        )
        .map_err(AppError::db)
}

/// Clear a recorded failure and stamp the last sync time.
fn record_success(store: &SkillStore) {
    let _ = store.set_setting(auto_backup::SETTING_LAST_ERROR, "");
    stamp_last_sync(store);
}

pub(crate) fn stamp_last_sync(store: &SkillStore) {
    let now = chrono::Utc::now().timestamp_millis().to_string();
    let _ = store.set_setting(SETTING_LAST_SYNC_AT, &now);
}

/// Outcome of a full sync transaction.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncOutcome {
    /// Local changes were committed as part of this sync.
    pub committed: bool,
    /// Merge result when a merge ran (None when nothing to pull).
    pub merge: Option<merge::MergeSummary>,
    pub pushed: bool,
}

const SYNC_PUSH_ATTEMPTS: usize = 3;

/// Full sync as one transaction under the repo lock the caller holds
/// (merge-engine design §9 并发收敛): commit → fetch/merge → push, retrying
/// the fetch/merge/push tail when another device pushes concurrently.
pub(crate) fn run_sync_blocking(
    store: &SkillStore,
    skills_dir: &Path,
    message: &str,
) -> anyhow::Result<SyncOutcome> {
    apply_device_identity(store, skills_dir);

    // Local changes first — they must be safe before any network step.
    sync_metadata::write_all_from_db_unlocked(store)?;
    // Rebuild the oversized exclusions BEFORE the dirty check: a previously
    // excluded skill that shrank below the limit re-enters the backup by
    // making .gitignore (and the skill itself) show up as changes.
    if let Err(e) =
        git_backup::apply_oversized_exclusions(skills_dir, git_backup::SKILL_SIZE_LIMIT_BYTES)
    {
        log::warn!("backup size: exclusion scan failed (continuing): {e:#}");
    }
    let mut committed = false;
    if git_backup::has_uncommitted_changes(skills_dir)? {
        git_backup::commit_all_unlocked(skills_dir, message)?;
        committed = true;
    }

    // Best-effort initial fetch: a missing remote branch (fresh remote) or a
    // network failure must not block the local commit; push surfaces real
    // connectivity errors below when there is something to push.
    let branch = git_backup::current_branch(skills_dir);
    if let Err(e) = git_backup::fetch_branch(skills_dir, &branch) {
        log::info!("git sync: initial fetch failed (continuing): {e:#}");
    }

    let mut merge_summary: Option<merge::MergeSummary> = None;
    let mut pushed = false;
    for attempt in 0..SYNC_PUSH_ATTEMPTS {
        let status = git_backup::get_status(skills_dir)?;
        if status.behind > 0 {
            let summary = merge::object_merge_pull_unlocked(store, skills_dir)?;
            reconcile_skills_index_unlocked(store)?;
            merge_summary = Some(summary);
        }

        let status = git_backup::get_status(skills_dir)?;
        let needs_push = committed || status.ahead > 0 || status.upstream_health == "no_upstream";
        if !needs_push {
            break;
        }
        match git_backup::push_unlocked(skills_dir) {
            Ok(()) => {
                pushed = true;
                break;
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let rejected = msg.contains("non-fast-forward")
                    || msg.contains("fetch first")
                    || msg.contains("[rejected]")
                    || msg.contains("failed to push some refs");
                if !rejected || attempt + 1 == SYNC_PUSH_ATTEMPTS {
                    return Err(e);
                }
                log::info!(
                    "git sync: push rejected (attempt {}), refetching",
                    attempt + 1
                );
                git_backup::fetch_branch(skills_dir, &branch)?;
            }
        }
    }

    if pushed {
        // A successful sync also clears a lingering auto-backup failure card.
        let _ = store.set_setting(auto_backup::SETTING_LAST_ERROR, "");
    }
    store.log_audit(
        AuditDraft::new("sync")
            .detail(format!(
                "committed={} merged={} pushed={}",
                committed,
                merge_summary.is_some(),
                pushed
            ))
            .ok(),
    );
    Ok(SyncOutcome {
        committed,
        merge: merge_summary,
        pushed,
    })
}
