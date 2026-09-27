//! Atomic resource-level decisions inside the existing Git backup merge.
//! Unlike residual files, a bundle may never be merged using newest-wins.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use git2::{Oid, Repository, Tree};
use serde_json::{json, Value};

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
const STATE: &str = "resource_sync_conflict";
pub const TRAILER: &str = "Agents-Manager-Resources: 1";
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

pub fn clear_resolved(repo: &Repository, store: &SkillStore) -> Result<()> {
    for record in store.resource_state_list(STATE)? {
        if record["choice"].as_str().is_some() {
            if let Some(id) = record["id"].as_str() {
                store.resource_state_delete(STATE, id)?;
                remove_conflict_refs(repo, id);
            }
        }
    }
    Ok(())
}

/// Git and SQLite cannot commit together. If a process ended after installing
/// the reviewed merge, prove that exact merge exists before clearing its UI row.
pub fn reconcile_completed(repo: &Repository, store: &SkillStore) -> Result<()> {
    let Ok(head) = repo.head().and_then(|head| head.peel_to_commit()) else {
        return Ok(());
    };
    for record in store.resource_state_list(STATE)? {
        let Some(choice @ ("local" | "remote")) = record["choice"].as_str() else {
            continue;
        };
        let ours = Oid::from_str(record["ours"].as_str().context("Invalid conflict tip")?)?;
        let theirs = Oid::from_str(record["theirs"].as_str().context("Invalid conflict tip")?)?;
        if ![ours, theirs].iter().all(|tip| {
            head.id() == *tip || repo.graph_descendant_of(head.id(), *tip).unwrap_or(false)
        }) {
            continue;
        }
        let selected = repo
            .find_commit(if choice == "local" { ours } else { theirs })?
            .tree()?;
        let expected = groups(&super::merge::snapshot::read_snapshot(repo, &selected)?.residual);
        let key = record["key"].as_str().context("Invalid conflict key")?;
        let mut walk = repo.revwalk()?;
        walk.push(head.id())?;
        walk.hide(ours)?;
        walk.hide(theirs)?;
        for oid in walk {
            let commit = repo.find_commit(oid?)?;
            let parents: Vec<_> = commit.parent_ids().collect();
            if !parents.contains(&ours) || !parents.contains(&theirs) {
                continue;
            }
            let actual =
                groups(&super::merge::snapshot::read_snapshot(repo, &commit.tree()?)?.residual);
            if actual.get(key) == expected.get(key) {
                let id = record["id"].as_str().context("Invalid conflict id")?;
                store.resource_state_delete(STATE, id)?;
                remove_conflict_refs(repo, id);
            }
            break;
        }
    }
    Ok(())
}

pub fn merge(
    repo: &Repository,
    store: &SkillStore,
    base: &Snapshot,
    ours: &Snapshot,
    theirs: &Snapshot,
    ours_tip: Oid,
    theirs_tip: Oid,
) -> Result<Files> {
    let b = groups(&base.residual);
    let o = groups(&ours.residual);
    let t = groups(&theirs.residual);
    let keys: BTreeSet<_> = b.keys().chain(o.keys()).chain(t.keys()).collect();
    let mut result = BTreeMap::new();
    let previous = store.resource_state_list(STATE)?;
    let mut active = BTreeSet::new();
    let mut blocked = Vec::new();
    for item_key in keys {
        let (base_item, local, remote) = (b.get(item_key), o.get(item_key), t.get(item_key));
        let choice = if local == remote || remote == base_item {
            local
        } else if local == base_item {
            remote
        } else {
            let existing = previous.iter().find(|v| {
                v["key"] == *item_key
                    && v["ours"] == ours_tip.to_string()
                    && v["theirs"] == theirs_tip.to_string()
            });
            let record = existing.cloned().unwrap_or_else(|| {
                json!({
                    "id":uuid::Uuid::new_v4().to_string(),"key":item_key,
                    "ours":ours_tip.to_string(),"theirs":theirs_tip.to_string(),"choice":null
                })
            });
            let id = record["id"].as_str().context("Invalid resource conflict")?;
            active.insert(id.to_owned());
            // Retain both commits while a human reviews this conflict, even if
            // the remote branch advances and Git later garbage-collects.
            if repo.find_commit(ours_tip).is_ok() && repo.find_commit(theirs_tip).is_ok() {
                repo.reference(
                    &format!("refs/skills-manager/resource-conflicts/{id}/local"),
                    ours_tip,
                    true,
                    "retain resource conflict",
                )?;
                repo.reference(
                    &format!("refs/skills-manager/resource-conflicts/{id}/remote"),
                    theirs_tip,
                    true,
                    "retain resource conflict",
                )?;
            }
            store.resource_state_put(STATE, id, &record)?;
            match record["choice"].as_str() {
                Some("local") => local,
                Some("remote") => remote,
                _ => {
                    blocked.push(item_key.to_owned());
                    local
                }
            }
        };
        if let Some(files) = choice {
            result.extend(files.clone());
        }
    }
    for stale in previous {
        if let Some(id) = stale["id"].as_str() {
            if !active.contains(id) {
                store.resource_state_delete(STATE, id)?;
                remove_conflict_refs(repo, id);
            }
        }
    }
    if !blocked.is_empty() {
        bail!("Instruction/MCP library changes need review in Library before syncing again ({} conflicting resources). No merged files were applied.", blocked.len());
    }
    Ok(result)
}

fn remove_conflict_refs(repo: &Repository, id: &str) {
    for side in ["local", "remote"] {
        if let Ok(mut reference) = repo.find_reference(&format!(
            "refs/skills-manager/resource-conflicts/{id}/{side}"
        )) {
            let _ = reference.delete();
        }
    }
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
            let records = ctx.store.resource_state_list(STATE)?;
            if records.is_empty() {
                return Ok(json!({"conflicts":[]}));
            }
            let repo = Repository::open(central_repo::skills_dir())?;
            let mut conflicts = Vec::new();
            for mut value in records {
                let item_key = value["key"]
                    .as_str()
                    .context("Invalid resource conflict")?
                    .to_owned();
                let local = content(
                    &repo,
                    value["ours"].as_str().context("Missing local version")?,
                    &item_key,
                )?;
                let remote = content(
                    &repo,
                    value["theirs"].as_str().context("Missing remote version")?,
                    &item_key,
                )?;
                let kind = item_key.split('/').next().unwrap_or_default();
                let name = local
                    .as_ref()
                    .or(remote.as_ref())
                    .and_then(|files| {
                        files
                            .iter()
                            .find(|(path, _)| path.ends_with("definition.json") || kind == "mcps")
                            .and_then(|(_, raw)| serde_json::from_str::<Value>(raw).ok())
                            .and_then(|v| v["name"].as_str().map(str::to_owned))
                    })
                    .unwrap_or_else(|| item_key.clone());
                value["kind"] = json!(kind);
                value["name"] = json!(name);
                value["local"] = json!(local);
                value["remote"] = json!(remote);
                conflicts.push(value);
            }
            Ok(json!({"conflicts":conflicts}))
        }
        Some("resolve") => {
            let _lock = RepoLock::acquire_foreground("review resource library conflict")?;
            let id = request["id"].as_str().context("Missing conflict id")?;
            let choice = request["choice"]
                .as_str()
                .context("Missing conflict choice")?;
            if !matches!(choice, "local" | "remote") {
                bail!("Choose local or remote");
            }
            let mut record = ctx
                .store
                .resource_state_get(STATE, id)?
                .context("Conflict no longer exists")?;
            record["choice"] = json!(choice);
            ctx.store.resource_state_put(STATE, id, &record)?;
            Ok(json!({"resolved":true}))
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
    fn bundle_changes_are_never_newest_wins() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let store = SkillStore::new(&dir.path().join("state.db")).unwrap();
        let path =
            ".agents-manager/instructions/00000000-0000-0000-0000-000000000001/definition.json";
        let mut base = Snapshot::default();
        base.residual.insert(path.into(), entry(1));
        let mut ours = Snapshot::default();
        ours.residual.insert(path.into(), entry(2));
        let mut theirs = Snapshot::default();
        theirs.residual.insert(path.into(), entry(3));
        assert!(merge(
            &repo,
            &store,
            &base,
            &ours,
            &theirs,
            entry(4).oid,
            entry(5).oid
        )
        .is_err());
        let mut record = store.resource_state_list(STATE).unwrap().pop().unwrap();
        record["choice"] = json!("remote");
        store
            .resource_state_put(STATE, record["id"].as_str().unwrap(), &record)
            .unwrap();
        let merged = merge(
            &repo,
            &store,
            &base,
            &ours,
            &theirs,
            entry(4).oid,
            entry(5).oid,
        )
        .unwrap();
        assert_eq!(merged[path], entry(3));
        // A different remote invalidates the previous reviewed choice.
        assert!(merge(
            &repo,
            &store,
            &base,
            &ours,
            &theirs,
            entry(4).oid,
            entry(6).oid
        )
        .is_err());
    }
    #[test]
    fn independent_resources_merge_and_deleted_bundle_stays_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let store = SkillStore::new(&dir.path().join("state.db")).unwrap();
        let path = ".agents-manager/mcps/00000000-0000-0000-0000-000000000001.json";
        let mut base = Snapshot::default();
        base.residual.insert(path.into(), entry(1));
        let ours = Snapshot::default();
        let theirs = Snapshot::default();
        assert!(merge(
            &repo,
            &store,
            &base,
            &ours,
            &theirs,
            entry(2).oid,
            entry(3).oid
        )
        .unwrap()
        .is_empty());
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
