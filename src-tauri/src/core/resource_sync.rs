//! Atomic resource-level decisions inside the existing Git backup merge.
//! Unlike residual files, a bundle may never be merged using newest-wins.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use git2::{Oid, Repository, Tree};
use serde_json::{json, Value};

use super::merge::protocol::parse_trailer_ids;
use super::{
    central_repo,
    error::AppError,
    host::HostCtx,
    merge::snapshot::{FileEntry, Snapshot},
    repo_lock::RepoLock,
    resource_store,
    skill_store::SkillStore,
};

const PREFIX: &str = ".agents-manager/";
/// Where release 1 kept its per-host conflict rows; dropped on the next pull.
const LEGACY_STATE: &str = "resource_sync_conflict";
pub const TRAILER: &str = "Agents-Manager-Resources: 1";
/// Instruction and MCP conflicts are recorded in Sync's history like skill
/// conflicts (SID-108), so every host sees them and any host resolves them.
pub const TRAILER_CONFLICTS: &str = "Agents-Manager-Resource-Conflicts";
pub const TRAILER_RESOLVED: &str = "Agents-Manager-Resource-Resolved";
type Files = BTreeMap<String, FileEntry>;

fn key(path: &str) -> Option<String> {
    let rest = path.strip_prefix(PREFIX)?;
    let mut parts = rest.split('/');
    let kind = parts.next()?;
    let id = parts.next()?;
    match kind {
        "instructions" => Some(format!("instructions/{id}")),
        "mcps" => Some(format!("mcps/{}", id.strip_suffix(".json")?)),
        _ => None,
    }
}

fn groups(files: &Files) -> BTreeMap<String, Files> {
    let mut out: BTreeMap<String, Files> = BTreeMap::new();
    for (path, entry) in files {
        if let Some(key) = key(path) {
            out.entry(key).or_default().insert(path.clone(), *entry);
        }
    }
    out
}

pub fn validate_tree(repo: &Repository, tree: &Tree) -> Result<()> {
    let Ok(root_entry) = tree.get_path(Path::new(".agents-manager")) else {
        return Ok(());
    };
    let root = repo
        .find_tree(root_entry.id())
        .context("Resource namespace must be a directory")?;
    let schema = root
        .get_name("schema.json")
        .context("Resource library schema is missing")?;
    if schema.filemode() != 0o100644 || repo.find_blob(schema.id())?.size() > 1024 {
        bail!("Invalid resource library schema file");
    }
    let schema_value: Value = serde_json::from_slice(repo.find_blob(schema.id())?.content())
        .context("Invalid resource library schema")?;
    if schema_value.get("version").and_then(Value::as_u64) != Some(1) {
        bail!("Expanded library requires another Agents Manager version");
    }
    for entry in root.iter() {
        match entry.name().unwrap_or_default() {
            "schema.json" => {}
            "instructions" => {
                let instructions = repo.find_tree(entry.id())?;
                for item in instructions.iter() {
                    let id = item.name().context("Invalid instruction identifier")?;
                    resource_store::validate_id(id)?;
                    let bundle = repo.find_tree(item.id())?;
                    let def = bundle
                        .get_name("definition.json")
                        .context("Instruction definition is missing")?;
                    let raw = repo.find_blob(def.id())?;
                    if raw.size() > resource_store::MAX_FILE_BYTES as usize {
                        bail!("Instruction definition too large");
                    }
                    let value: Value = serde_json::from_slice(raw.content())
                        .context("Invalid instruction definition")?;
                    if value.get("id").and_then(Value::as_str) != Some(id) {
                        bail!("Instruction identifier mismatch");
                    }
                    let revision = value
                        .get("revision")
                        .and_then(Value::as_str)
                        .context("Instruction revision missing")?;
                    resource_store::validate_id(revision)?;
                    if value
                        .get("name")
                        .and_then(Value::as_str)
                        .is_none_or(|name| name.trim().is_empty())
                        || value.get("updated_at").and_then(Value::as_i64).is_none()
                    {
                        bail!("Instruction metadata is incomplete");
                    }
                    if bundle
                        .iter()
                        .any(|entry| !matches!(entry.name(), Some("definition.json" | "files")))
                    {
                        bail!("Unknown instruction bundle content");
                    }
                    let files = bundle
                        .get_name("files")
                        .context("Instruction payload is missing")?;
                    if repo.find_tree(files.id())?.is_empty() {
                        bail!("Instruction bundle is empty");
                    }
                    validate_payload_tree(repo, &bundle, 0, &mut 0)?;
                }
            }
            "mcps" => {
                let mcps = repo.find_tree(entry.id())?;
                for item in mcps.iter() {
                    let id = item
                        .name()
                        .and_then(|s| s.strip_suffix(".json"))
                        .context("Invalid MCP resource filename")?;
                    resource_store::validate_id(id)?;
                    let blob = repo
                        .find_blob(item.id())
                        .context("MCP resource must be a file")?;
                    if blob.size() > resource_store::MAX_FILE_BYTES as usize {
                        bail!("MCP definition too large");
                    }
                    let value: Value =
                        serde_json::from_slice(blob.content()).context("Invalid MCP definition")?;
                    if value.get("id").and_then(Value::as_str) != Some(id) {
                        bail!("MCP identifier mismatch");
                    }
                    super::mcps::validate_portable(&value).map_err(|_| {
                        anyhow::anyhow!("Backup contains an invalid or nonportable MCP definition")
                    })?;
                    if item.filemode() != 0o100644 && item.filemode() != 0o100755 {
                        bail!("Linked MCP definition is unsupported");
                    }
                }
            }
            _ => bail!("Unknown resource namespace; upgrade Agents Manager before synchronizing"),
        }
    }
    Ok(())
}

fn validate_payload_tree(
    repo: &Repository,
    tree: &Tree,
    depth: usize,
    count: &mut usize,
) -> Result<()> {
    if depth > 24 {
        bail!("Instruction bundle is too deep");
    }
    for entry in tree.iter() {
        resource_store::safe_relative(entry.name().context("Invalid instruction path")?)?;
        *count += 1;
        if *count > 5000 {
            bail!("Instruction bundle contains too many files");
        }
        match entry.kind() {
            Some(git2::ObjectType::Tree) => {
                validate_payload_tree(repo, &repo.find_tree(entry.id())?, depth + 1, count)?
            }
            Some(git2::ObjectType::Blob)
                if entry.filemode() == 0o100644 || entry.filemode() == 0o100755 =>
            {
                let blob = repo.find_blob(entry.id())?;
                if blob.size() > resource_store::MAX_FILE_BYTES as usize {
                    bail!("Instruction file too large");
                }
                std::str::from_utf8(blob.content()).context("Instruction payload is not UTF-8")?;
            }
            _ => bail!("Instruction backups cannot contain symlinks or submodules"),
        }
    }
    Ok(())
}

/// Old writers cannot be retroactively taught this contract. Detect their
/// commits on an expanded repository and refuse further automatic merging.
pub fn check_writers(repo: &Repository, base: Oid, tip: Oid) -> Result<()> {
    let mut walk = repo.revwalk()?;
    walk.push(tip)?;
    walk.hide(base)?;
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        if commit.tree()?.get_name(".agents-manager").is_some()
            && !commit
                .message()
                .unwrap_or_default()
                .lines()
                .any(|line| line.trim() == TRAILER)
        {
            bail!("Expanded library has writes from an incompatible client. Upgrade every device sharing this backup and review its changes before syncing.");
        }
    }
    Ok(())
}

pub fn check_current_writer(repo: &Repository) -> Result<()> {
    let Ok(commit) = repo.head().and_then(|head| head.peel_to_commit()) else {
        return Ok(());
    };
    if commit.tree()?.get_name(".agents-manager").is_some()
        && !commit
            .message()
            .unwrap_or_default()
            .lines()
            .any(|line| line.trim() == TRAILER)
    {
        bail!("Expanded library was written by an incompatible client. Upgrade all shared-backup devices and review those changes before synchronizing.");
    }
    Ok(())
}

/// Drop release 1's per-host conflict rows and retention refs: conflicts now
/// live in Sync's history, and the next merge finds any that remain.
pub fn drop_legacy_records(repo: &Repository, store: &SkillStore) -> Result<()> {
    for record in store.resource_state_list(LEGACY_STATE)? {
        if let Some(id) = record["id"].as_str() {
            store.resource_state_delete(LEGACY_STATE, id)?;
        }
    }
    let legacy: Vec<String> = repo
        .references_glob("refs/skills-manager/resource-conflicts/*")?
        .flatten()
        .filter_map(|r| r.name().map(str::to_owned))
        .collect();
    for name in legacy {
        if let Ok(mut reference) = repo.find_reference(&name) {
            reference.delete()?;
        }
    }
    Ok(())
}

/// Merged instruction and MCP files, plus the resources changed differently
/// on both sides. Those keep this host's version and are recorded as
/// conflicts in the merge commit (SID-108); a bundle never merges newest-wins.
pub struct Merged {
    pub files: Files,
    pub conflicts: Vec<String>,
}

pub fn merge(base: &Snapshot, ours: &Snapshot, theirs: &Snapshot) -> Merged {
    let b = groups(&base.residual);
    let o = groups(&ours.residual);
    let t = groups(&theirs.residual);
    let keys: BTreeSet<_> = b.keys().chain(o.keys()).chain(t.keys()).collect();
    let mut merged = Merged {
        files: BTreeMap::new(),
        conflicts: Vec::new(),
    };
    for item_key in keys {
        let (base_item, local, remote) = (b.get(item_key), o.get(item_key), t.get(item_key));
        let choice = if local == remote || remote == base_item {
            local
        } else if local == base_item {
            remote
        } else {
            merged.conflicts.push(item_key.clone());
            local
        };
        if let Some(files) = choice {
            merged.files.extend(files.clone());
        }
    }
    merged
}

/// The conflict trailer line for a merge commit.
pub fn conflicts_trailer(keys: &[String]) -> Option<String> {
    (!keys.is_empty()).then(|| format!("{TRAILER_CONFLICTS}: {}", keys.join(", ")))
}

/// Open instruction and MCP conflicts in HEAD's history, each with the merge
/// that recorded it last: its second parent holds the other host's version.
fn open_conflicts(repo: &Repository) -> Result<BTreeMap<String, Oid>> {
    let mut open = BTreeMap::new();
    let Ok(head) = repo.head().and_then(|head| head.peel_to_commit()) else {
        return Ok(open);
    };
    let mut walk = repo.revwalk()?;
    walk.push(head.id())?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE)?;
    for oid in walk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let message = commit.message().unwrap_or_default();
        for key in parse_trailer_ids(message, TRAILER_CONFLICTS) {
            open.insert(key, oid);
        }
        for key in parse_trailer_ids(message, TRAILER_RESOLVED) {
            open.remove(&key);
        }
    }
    Ok(open)
}

/// An instruction or MCP changed on two hosts, as the review lists it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceConflict {
    pub key: String,
    pub kind: String,
    pub name: String,
    /// The host whose version Sync kept, and the host with the other one.
    pub this_host: String,
    pub other_host: String,
}

pub fn conflicts(repo: &Repository) -> Result<Vec<ResourceConflict>> {
    open_conflicts(repo)?
        .into_iter()
        .map(|(key, declared)| describe(repo, key, declared).map(|(conflict, ..)| conflict))
        .collect()
}

type Contents = Option<BTreeMap<String, String>>;

/// A conflict with the files Sync has now and the other host's files.
fn describe(
    repo: &Repository,
    key: String,
    declared: Oid,
) -> Result<(ResourceConflict, Contents, Contents)> {
    let merge = repo.find_commit(declared)?;
    let other = merge.parent(1)?;
    let head = repo.head()?.peel_to_commit()?;
    let kept = content(repo, &head.id().to_string(), &key)?;
    let alternative = content(repo, &other.id().to_string(), &key)?;
    let author = |commit: &git2::Commit| commit.author().name().unwrap_or_default().to_owned();
    let conflict = ResourceConflict {
        kind: key.split('/').next().unwrap_or_default().to_owned(),
        name: resource_name(&key, kept.as_ref().or(alternative.as_ref())),
        this_host: author(&merge),
        other_host: author(&other),
        key,
    };
    Ok((conflict, kept, alternative))
}

fn resource_name(key: &str, files: Option<&BTreeMap<String, String>>) -> String {
    files
        .and_then(|files| {
            files
                .iter()
                .find(|(path, _)| path.ends_with("definition.json") || key.starts_with("mcps/"))
                .and_then(|(_, raw)| serde_json::from_str::<Value>(raw).ok())
                .and_then(|v| v["name"].as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| key.to_owned())
}

/// Resolve a conflict, then apply the result to this host's agents.
pub fn resolve(ctx: &HostCtx, key: &str, use_other: bool) -> Result<()> {
    {
        let _lock = RepoLock::acquire_foreground("resolve instruction or MCP conflict")?;
        resolve_unlocked(&ctx.store, key, use_other)?;
    }
    super::sync_apply::reapply_resources(ctx);
    Ok(())
}

/// Settle an open conflict with a commit carrying the resolved trailer: keep
/// the version Sync has, or put back the other host's. Every host applies it
/// on its next pull. Caller holds the repo lock.
pub fn resolve_unlocked(store: &SkillStore, key: &str, use_other: bool) -> Result<()> {
    let skills_dir = central_repo::skills_dir();
    super::git_backup::ensure_no_interrupted_git_operation(&skills_dir)?;
    super::sync_metadata::write_all_from_db_unlocked(store)?;
    if super::git_backup::has_uncommitted_changes(&skills_dir)? {
        super::git_backup::commit_all_unlocked(&skills_dir, "backup")?;
    }
    let repo = Repository::open(&skills_dir)?;
    let declared = *open_conflicts(&repo)?
        .get(key)
        .context("This conflict is already resolved")?;
    if use_other {
        let other = repo.find_commit(declared)?.parent(1)?.tree()?;
        let files = groups(&super::merge::snapshot::read_snapshot(&repo, &other)?.residual)
            .remove(key)
            .unwrap_or_default();
        let current = skills_dir
            .join(PREFIX)
            .join(match key.strip_prefix("mcps/") {
                Some(id) => format!("mcps/{id}.json"),
                None => key.to_owned(),
            });
        if current.is_dir() {
            std::fs::remove_dir_all(&current)?;
        } else if current.exists() {
            std::fs::remove_file(&current)?;
        }
        for (path, entry) in files {
            let target = skills_dir.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(target, repo.find_blob(entry.oid)?.content())?;
        }
    }
    let label = if use_other {
        "other version"
    } else {
        "kept version"
    };
    let message = format!(
        "{}\n{TRAILER_RESOLVED}: {key}",
        super::merge::protocol::app_commit_message(&format!("sync: resolve {key}: {label}"))
    );
    super::git_backup::commit_resolution_unlocked(&skills_dir, &message)
}

fn content(
    repo: &Repository,
    tip: &str,
    item_key: &str,
) -> Result<Option<BTreeMap<String, String>>> {
    let tree = repo.find_commit(Oid::from_str(tip)?)?.tree()?;
    validate_tree(repo, &tree)?;
    let snap = super::merge::snapshot::read_snapshot(repo, &tree)?;
    groups(&snap.residual)
        .get(item_key)
        .map(|files| {
            files
                .iter()
                .map(|(path, entry)| {
                    let blob = repo.find_blob(entry.oid)?;
                    Ok((
                        path.clone(),
                        std::str::from_utf8(blob.content())?.to_owned(),
                    ))
                })
                .collect()
        })
        .transpose()
}

pub fn dispatch(ctx: &HostCtx, request: Value) -> Result<Value, AppError> {
    dispatch_inner(ctx, request).map_err(|error| AppError::invalid_input(error.to_string()))
}

fn dispatch_inner(ctx: &HostCtx, request: Value) -> Result<Value> {
    match request["action"].as_str() {
        Some("list") => {
            let Ok(repo) = Repository::open(central_repo::skills_dir()) else {
                return Ok(json!({"conflicts": []}));
            };
            let mut out = Vec::new();
            for (key, declared) in open_conflicts(&repo)? {
                let (conflict, local, remote) = describe(&repo, key, declared)?;
                let mut value = serde_json::to_value(&conflict)?;
                value["id"] = json!(conflict.key);
                value["local"] = json!(local);
                value["remote"] = json!(remote);
                out.push(value);
            }
            Ok(json!({"conflicts": out}))
        }
        Some("resolve") => {
            let key = request["id"].as_str().context("Missing conflict id")?;
            let use_other = match request["choice"].as_str() {
                Some("local") => false,
                Some("remote") => true,
                _ => bail!("Choose local or remote"),
            };
            resolve(ctx, key, use_other)?;
            Ok(json!({"resolved": true}))
        }
        _ => bail!("Unknown resource sync action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(n: u8) -> FileEntry {
        FileEntry {
            oid: Oid::from_bytes(&[n; 20]).unwrap(),
            mode: 0o100644,
        }
    }
    #[test]
    fn a_bundle_changed_on_both_sides_keeps_ours_and_is_recorded() {
        // Never newest-wins: ours stays and the merge records a conflict.
        let path =
            ".agents-manager/instructions/00000000-0000-0000-0000-000000000001/definition.json";
        let mut base = Snapshot::default();
        base.residual.insert(path.into(), entry(1));
        let mut ours = Snapshot::default();
        ours.residual.insert(path.into(), entry(2));
        let mut theirs = Snapshot::default();
        theirs.residual.insert(path.into(), entry(3));
        let merged = merge(&base, &ours, &theirs);
        assert_eq!(merged.files[path], entry(2));
        assert_eq!(
            merged.conflicts,
            ["instructions/00000000-0000-0000-0000-000000000001"]
        );
        assert_eq!(
            conflicts_trailer(&merged.conflicts).unwrap(),
            format!("{TRAILER_CONFLICTS}: {}", merged.conflicts[0])
        );
    }
    #[test]
    fn independent_resources_merge_and_deleted_bundle_stays_deleted() {
        let path = ".agents-manager/mcps/00000000-0000-0000-0000-000000000001.json";
        let mut base = Snapshot::default();
        base.residual.insert(path.into(), entry(1));
        let merged = merge(&base, &Snapshot::default(), &Snapshot::default());
        assert!(merged.files.is_empty() && merged.conflicts.is_empty());
    }

    #[test]
    fn previous_writer_marker_is_detected_on_expanded_backup() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("fixture", "fixture@example.test").unwrap();
        let empty = repo.treebuilder(None).unwrap().write().unwrap();
        let base = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "base",
                &repo.find_tree(empty).unwrap(),
                &[],
            )
            .unwrap();
        let schema = repo.blob(b"{\"version\":1}").unwrap();
        let mut resources = repo.treebuilder(None).unwrap();
        resources.insert("schema.json", schema, 0o100644).unwrap();
        let resource_tree = resources.write().unwrap();
        let mut root = repo.treebuilder(None).unwrap();
        root.insert(".agents-manager", resource_tree, 0o040000)
            .unwrap();
        let tree = repo.find_tree(root.write().unwrap()).unwrap();
        let parent = repo.find_commit(base).unwrap();
        // Exact trailer emitted by the previous main writer.
        let old = repo
            .commit(
                None,
                &sig,
                &sig,
                "backup\n\nSkills-Manager-Protocol: 2",
                &tree,
                &[&parent],
            )
            .unwrap();
        assert!(check_writers(&repo, base, old).is_err());
        let new = repo
            .commit(
                None,
                &sig,
                &sig,
                &format!("backup\n\n{TRAILER}"),
                &tree,
                &[&parent],
            )
            .unwrap();
        check_writers(&repo, base, new).unwrap();
        validate_tree(&repo, &tree).unwrap();
    }
}
