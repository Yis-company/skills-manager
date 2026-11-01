//! Read-only, bounded evidence for consequential Sync decisions.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};
use git2::{ObjectType, Repository, Tree};
use serde_json::{json, Value};
use walkdir::WalkDir;

use super::{
    central_repo, merge, resource_store, resource_sync, scenario_service, skill_store::SkillStore,
    sync_metadata, sync_service,
};

const MAX_FILES: usize = 256;
const MAX_BYTES: usize = 32 * 1024;
const MAX_LINES: usize = 400;

type Files = BTreeMap<String, Value>;

fn version(bytes: &[u8], stamp: String, executable: bool) -> Value {
    let text = std::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'));
    let kind =
        if bytes.len() > MAX_BYTES || text.is_some_and(|text| text.lines().count() > MAX_LINES) {
            "too_large"
        } else if text.is_none() {
            "binary"
        } else {
            "text"
        };
    json!({"kind": kind, "text": if kind == "text" { text } else { None }, "stamp": stamp, "executable": executable})
}

fn disk_files(root: &Path) -> Result<Files> {
    let mut files = Files::new();
    if !root.exists() {
        return Ok(files);
    }
    if root.symlink_metadata()?.file_type().is_symlink() {
        bail!("Cannot compare a linked resource root");
    }
    if root.is_file() {
        add_disk_file(&mut files, root, "definition.json".into())?;
        return Ok(files);
    }
    for entry in WalkDir::new(root).follow_links(false).max_depth(25) {
        let entry = entry?;
        if entry.depth() == 25 && entry.file_type().is_dir() {
            bail!("Comparison is too deep");
        }
        if entry.path() == root || entry.file_type().is_dir() {
            continue;
        }
        if files.len() >= MAX_FILES {
            bail!("Too many files to compare safely (limit {MAX_FILES})");
        }
        let relative = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        add_disk_file(&mut files, entry.path(), relative)?;
    }
    Ok(files)
}

fn add_disk_file(files: &mut Files, path: &Path, name: String) -> Result<()> {
    let meta = path.symlink_metadata()?;
    let stamp = format!(
        "{}:{:?}:{:?}",
        meta.len(),
        meta.modified()?,
        meta.permissions()
    );
    if meta.file_type().is_symlink() {
        files.insert(name, json!({"kind":"symlink", "text":null, "stamp":format!("{stamp}:{:?}", std::fs::read_link(path)?), "executable":false}));
    } else if meta.is_file() {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        files.insert(name, version(&bytes, stamp, executable));
    } else {
        bail!("Unsupported file type in comparison");
    }
    Ok(())
}

fn tree_files(
    repo: &Repository,
    tree: &Tree,
    prefix: &str,
    files: &mut Files,
    depth: usize,
) -> Result<()> {
    if depth > 24 {
        bail!("Comparison is too deep");
    }
    for entry in tree.iter() {
        if files.len() >= MAX_FILES {
            bail!("Too many files to compare safely (limit {MAX_FILES})");
        }
        let name = entry.name().context("Invalid file name")?;
        resource_store::safe_relative(name)?;
        let path = format!("{prefix}{name}");
        match entry.kind() {
            Some(ObjectType::Tree) => tree_files(
                repo,
                &repo.find_tree(entry.id())?,
                &format!("{path}/"),
                files,
                depth + 1,
            )?,
            Some(ObjectType::Blob) => {
                let blob = repo.find_blob(entry.id())?;
                let value = if entry.filemode() == 0o120000 {
                    json!({"kind":"symlink","text":null,"stamp":entry.id().to_string(),"executable":false})
                } else {
                    version(
                        blob.content(),
                        entry.id().to_string(),
                        entry.filemode() == 0o100755,
                    )
                };
                files.insert(path, value);
            }
            _ => bail!("Unsupported Git entry in comparison"),
        }
    }
    Ok(())
}

fn comparison(left: Files, right: Files) -> Vec<Value> {
    let keys: BTreeSet<_> = left.keys().chain(right.keys()).cloned().collect();
    keys.into_iter().map(|path| {
        let a = left.get(&path);
        let b = right.get(&path);
        let kind = [a, b].into_iter().flatten().find(|v| v["kind"] != "text").map(|v| v["kind"].as_str().unwrap_or("binary")).unwrap_or("text");
        json!({"relative_path":path,"status":if a.is_none(){"added"}else if b.is_none(){"removed"}else{"modified"},"content_kind":if kind == "symlink" {"binary"} else {kind},"original_text":a.and_then(|v|v["text"].as_str()),"updated_text":b.and_then(|v|v["text"].as_str()),"executable_before":a.is_some_and(|v|v["executable"]==true),"executable_after":b.is_some_and(|v|v["executable"]==true),"original_stamp":a.map(|v|&v["stamp"]),"updated_stamp":b.map(|v|&v["stamp"])})
    }).collect()
}

fn preset_state(store: &SkillStore, id: &str) -> Result<Value> {
    let preset = store
        .get_all_scenarios()?
        .into_iter()
        .find(|p| p.id == id)
        .context("Preset no longer exists")?;
    let mut skills = Vec::new();
    for skill in store.get_skill_ids_for_scenario(id)? {
        let name = store
            .get_skill_by_id(&skill)?
            .map(|s| s.name)
            .unwrap_or_else(|| skill.clone());
        let toggles = store.get_scenario_skill_tool_toggles(id, &skill)?;
        skills.push(json!({"id":skill,"name":name,"agents":toggles.iter().map(|t|json!({"agent":t.tool,"on":t.enabled})).collect::<Vec<_>>()}));
    }
    Ok(json!({"id":id,"name":preset.name,"skills":skills}))
}

pub fn preview(store: &SkillStore, kind: &str, key: &str, tool: Option<&str>) -> Result<Value> {
    if kind == "switch" {
        return scenario_service::preview_switch_review(store, key);
    }
    let left_label: String;
    let right_label: String;
    let mut revision = Value::Null;
    let (left, right) = match kind {
        "skill" => {
            let conflict = sync_service::conflicts(store)
                .into_iter()
                .find(|c| c.skill_id == key)
                .context("Conflict no longer exists")?;
            left_label = conflict.this_host;
            right_label = conflict.other_host;
            let repo = Repository::open(central_repo::skills_dir())?;
            let head = repo.head()?.peel_to_commit()?;
            let other = merge::pending::ref_target(&repo, &merge::pending::conflict_ref(key))
                .or_else(|| {
                    store
                        .list_pending_conflicts()
                        .ok()?
                        .into_iter()
                        .find(|c| c.skill_id == key)
                        .and_then(|c| git2::Oid::from_str(&c.theirs_commit).ok())
                })
                .context("Other version is unavailable")?;
            let snap = merge::snapshot::read_snapshot(&repo, &repo.find_commit(other)?.tree()?)?;
            let mut right = Files::new();
            if let Some(tree) = snap.skills.get(key).and_then(|s| s.content) {
                tree_files(&repo, &repo.find_tree(tree)?, "", &mut right, 0)?;
            }
            let left = match store.get_skill_by_id(key)? {
                Some(skill) => {
                    let root = std::fs::canonicalize(&skill.central_path)?;
                    if !root.starts_with(std::fs::canonicalize(central_repo::skills_dir())?) {
                        bail!("Skill is outside the library");
                    }
                    disk_files(&root)?
                }
                None => Files::new(),
            };
            revision = json!([head.id().to_string(), other.to_string()]);
            (left, right)
        }
        "resource" => {
            let (resource_kind, id) = key.split_once('/').context("Invalid resource key")?;
            if !matches!(resource_kind, "instructions" | "mcps") {
                bail!("Invalid resource kind");
            }
            resource_store::validate_id(id)?;
            let repo = Repository::open(central_repo::skills_dir())?;
            let (conflict, kept, alternative, declared) =
                resource_sync::comparison_versions(&repo, key)?;
            left_label = conflict.this_host;
            right_label = conflict.other_host;
            let mut right = Files::new();
            for (name, entry) in alternative {
                let blob = repo.find_blob(entry.oid)?;
                right.insert(
                    name,
                    version(
                        blob.content(),
                        entry.oid.to_string(),
                        entry.mode == 0o100755,
                    ),
                );
            }
            let current_path = if key.starts_with("mcps/") {
                resource_store::root().join(format!("{key}.json"))
            } else {
                resource_store::root().join(key)
            };
            resource_store::reject_symlinks(&current_path)?;
            let current = disk_files(&current_path)?;
            if key.starts_with("mcps/") && current_path.exists() {
                let portable: Value =
                    serde_json::from_str(&resource_store::read_limited(&current_path)?)?;
                super::mcps::validate_portable(&portable).map_err(|_| {
                    anyhow::anyhow!("Nonportable MCP definition cannot be compared")
                })?;
            }
            let left = current
                .into_iter()
                .map(|(name, value)| {
                    let path = if key.starts_with("mcps/") {
                        format!(".agents-manager/{key}.json")
                    } else {
                        format!(".agents-manager/{key}/{name}")
                    };
                    (path, value)
                })
                .collect();
            revision = json!([
                repo.head()?.target().map(|id| id.to_string()),
                declared.to_string(),
                kept.keys().collect::<Vec<_>>()
            ]);
            (left, right)
        }
        "edited" => {
            let target = store
                .get_targets_for_skill(key)?
                .into_iter()
                .find(|t| {
                    Some(t.tool.as_str()) == tool
                        && t.status.starts_with(scenario_service::STATUS_EDITED)
                })
                .context("Edited copy no longer exists")?;
            left_label = target.tool.clone();
            right_label = "Sync library".into();
            let right = if target.status == scenario_service::STATUS_EDITED_LEFT {
                Files::new()
            } else {
                match store.get_skill_by_id(key)? {
                    Some(skill) => disk_files(Path::new(&skill.central_path))?,
                    None => Files::new(),
                }
            };
            revision = json!([target.status, target.target_path]);
            (disk_files(Path::new(&target.target_path))?, right)
        }
        "preset" => {
            let reviews = sync_metadata::preset_reviews(store)?;
            let other = reviews.get(key).context("Preset review no longer exists")?;
            let a = preset_state(store, key)?;
            let b = preset_state(store, other)?;
            left_label = a["name"].as_str().unwrap_or(key).into();
            right_label = b["name"].as_str().unwrap_or(other).into();
            let files = |value: &Value| -> Result<Files> {
                Ok(BTreeMap::from([(
                    "preset.json".into(),
                    version(
                        serde_json::to_string_pretty(value)?.as_bytes(),
                        String::new(),
                        false,
                    ),
                )]))
            };
            (files(&a)?, files(&b)?)
        }
        _ => bail!("Unsupported comparison"),
    };
    Ok(
        json!({"kind":kind,"key":key,"tool":tool,"left_label":left_label,"right_label":right_label,"revision":revision,"files":comparison(left,right)}),
    )
}

/// Called inside the mutation's existing lock. No preview operation writes.
pub fn check(
    store: &SkillStore,
    expected: Option<&str>,
    kind: &str,
    key: &str,
    tool: Option<&str>,
) -> Result<()> {
    if let Some(raw) = expected {
        let expected: Value = serde_json::from_str(raw).context("Invalid review")?;
        if expected != preview(store, kind, key, tool)? {
            bail!("Changes since review. Close this review and compare again before applying.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        host::HostCtx,
        host_dispatch,
        skill_store::{ScenarioRecord, SkillRecord, SkillTargetRecord},
    };
    use std::sync::Arc;

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            central_repo::set_test_base_dir_override(None);
        }
    }

    fn skill(store: &SkillStore, root: &Path) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("SKILL.md"), "library version\n").unwrap();
        store
            .insert_skill(&SkillRecord {
                id: "s".into(),
                name: "Example".into(),
                description: None,
                source_type: "import".into(),
                source_ref: None,
                source_ref_resolved: None,
                source_subpath: None,
                source_branch: None,
                source_revision: None,
                remote_revision: None,
                central_path: root.display().to_string(),
                content_hash: None,
                enabled: true,
                created_at: 1,
                updated_at: 1,
                status: "ok".into(),
                update_status: "local_only".into(),
                last_checked_at: None,
                last_check_error: None,
            })
            .unwrap();
    }

    #[test]
    fn large_binary_and_deleted_files_are_explicit_without_unbounded_text() {
        assert_eq!(
            version(&[0, 1, 2], "binary".into(), false)["kind"],
            "binary"
        );
        let long = "line\n".repeat(MAX_LINES + 1);
        let large = version(long.as_bytes(), "large".into(), false);
        assert_eq!(large["kind"], "too_large");
        assert!(large["text"].is_null());
        let files = comparison(
            BTreeMap::from([("gone.md".into(), version(b"keep me", "old".into(), false))]),
            Files::new(),
        );
        assert_eq!(files[0]["status"], "removed");
        assert_eq!(files[0]["original_text"], "keep me");
        assert!(files[0]["updated_text"].is_null());
    }

    #[test]
    fn edited_copy_review_is_read_only_and_stale_resolution_preserves_the_edit() {
        let _lock = central_repo::test_base_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(
            std::fs::canonicalize(temp.path()).unwrap().join("library"),
        ));
        let _reset = Reset;
        std::fs::create_dir_all(central_repo::skills_dir()).unwrap();
        let store = Arc::new(SkillStore::new(&temp.path().join("state.db")).unwrap());
        skill(&store, &central_repo::skills_dir().join("example"));
        let agent = temp.path().join("agent-copy");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(agent.join("SKILL.md"), "my edit\n").unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "target".into(),
                skill_id: "s".into(),
                tool: "codex".into(),
                target_path: agent.display().to_string(),
                mode: "copy".into(),
                status: "edited".into(),
                synced_at: Some(1),
                last_error: None,
                source_hash: None,
            })
            .unwrap();
        let context = HostCtx::headless(store.clone());
        let args = json!({"kind":"edited","key":"s","tool":"codex"});
        let review = host_dispatch::dispatch(&context, "sync_review", &args).unwrap();
        assert_eq!(review["files"][0]["original_text"], "my edit\n");
        assert_eq!(review["files"][0]["updated_text"], "library version\n");
        assert_eq!(
            store.get_targets_for_skill("s").unwrap()[0].status,
            "edited"
        );
        assert!(!central_repo::base_dir().join("edited-backups").exists());
        std::fs::write(agent.join("SKILL.md"), "a newer local edit\n").unwrap();
        let error = host_dispatch::dispatch(
            &context,
            "resolve_edited_copy",
            &json!({"skillId":"s","tool":"codex","keepMine":false,"review":review.to_string()}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Changes since review"));
        assert_eq!(
            std::fs::read_to_string(agent.join("SKILL.md")).unwrap(),
            "a newer local edit\n"
        );
        assert_eq!(
            store.get_targets_for_skill("s").unwrap()[0].status,
            "edited"
        );
        assert!(!central_repo::base_dir().join("edited-backups").exists());
        let current = host_dispatch::dispatch(&context, "sync_review", &args).unwrap();
        host_dispatch::dispatch(
            &context,
            "resolve_edited_copy",
            &json!({"skillId":"s","tool":"codex","keepMine":true,"review":current.to_string()}),
        )
        .unwrap();
        assert_eq!(store.get_targets_for_skill("s").unwrap()[0].status, "ok");
        assert_eq!(
            std::fs::read_to_string(agent.join("SKILL.md")).unwrap(),
            "a newer local edit\n"
        );
    }

    #[test]
    fn preset_review_names_preserved_edits_and_rejects_changed_targets_before_writing() {
        let _lock = central_repo::test_base_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(
            std::fs::canonicalize(temp.path()).unwrap().join("library"),
        ));
        let _reset = Reset;
        std::fs::create_dir_all(central_repo::skills_dir()).unwrap();
        let store = SkillStore::new(&temp.path().join("state.db")).unwrap();
        skill(&store, &central_repo::skills_dir().join("example"));
        store
            .insert_scenario(&ScenarioRecord {
                id: "empty".into(),
                name: "Empty preset".into(),
                description: None,
                icon: None,
                sort_order: 0,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        let copy = temp.path().join("edited");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("SKILL.md"), "changed").unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t".into(),
                skill_id: "s".into(),
                tool: "codex".into(),
                target_path: copy.display().to_string(),
                mode: "copy".into(),
                status: "ok".into(),
                synced_at: Some(1),
                last_error: None,
                source_hash: None,
            })
            .unwrap();
        let review = preview(&store, "switch", "empty", None).unwrap();
        assert_eq!(review["removals"].as_array().unwrap().len(), 0);
        assert_eq!(review["preserved"][0]["agent"], "codex");
        assert_eq!(store.get_targets_for_skill("s").unwrap()[0].status, "ok");
        store.delete_target("s", "codex").unwrap();
        assert!(scenario_service::apply_scenario_reviewed(
            &store,
            "empty",
            Some(&review.to_string())
        )
        .is_err());
        assert_eq!(store.get_active_scenario_id().unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(copy.join("SKILL.md")).unwrap(),
            "changed"
        );
    }

    #[test]
    #[cfg(unix)]
    fn comparison_never_reads_through_a_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("selected");
        std::fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside");
        std::fs::write(&outside, "do not expose").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        let files = disk_files(&root).unwrap();
        assert_eq!(files["linked"]["kind"], "symlink");
        assert!(files["linked"]["text"].is_null());
        assert!(disk_files(&root.join("linked")).is_err());
    }
    fn commit(repo: &Repository, message: &str, parents: &[git2::Oid], update: bool) -> git2::Oid {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.update_all(["*"], None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = git2::Signature::now("Fixture host", "fixture@example.test").unwrap();
        let parents = parents
            .iter()
            .map(|id| repo.find_commit(*id).unwrap())
            .collect::<Vec<_>>();
        repo.commit(
            if update { Some("HEAD") } else { None },
            &signature,
            &signature,
            message,
            &tree,
            &parents.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn retained_skill_versions_are_compared_and_stale_refs_cannot_replace_local_work() {
        use crate::core::skill_store::PendingConflictRow;
        let _lock = central_repo::test_base_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(
            std::fs::canonicalize(temp.path()).unwrap().join("library"),
        ));
        let _reset = Reset;
        std::fs::create_dir_all(central_repo::skills_dir()).unwrap();
        let store = Arc::new(SkillStore::new(&temp.path().join("state.db")).unwrap());
        let source = central_repo::skills_dir().join("example");
        skill(&store, &source);
        sync_metadata::write_all_from_db_unlocked(&store).unwrap();
        let repo = Repository::init(central_repo::skills_dir()).unwrap();
        let ours = commit(&repo, "ours", &[], true);
        std::fs::write(source.join("SKILL.md"), "other host version\n").unwrap();
        std::fs::write(source.join("binary.dat"), [0, 1, 2]).unwrap();
        let other = commit(&repo, "other", &[ours], false);
        store
            .replace_pending_conflicts(&[PendingConflictRow {
                skill_id: "s".into(),
                theirs_commit: other.to_string(),
                theirs_path: Some("example".into()),
                detected_at: 1,
            }])
            .unwrap();
        repo.reference(&merge::pending::conflict_ref("s"), other, true, "fixture")
            .unwrap();
        std::fs::write(source.join("SKILL.md"), "uncommitted local work\n").unwrap();
        std::fs::remove_file(source.join("binary.dat")).unwrap();
        let before = repo.head().unwrap().target();
        let review = preview(&store, "skill", "s", None).unwrap();
        let entries = review["files"].as_array().unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry["original_text"] == "uncommitted local work\n"
                && entry["updated_text"] == "other host version\n"));
        assert!(entries
            .iter()
            .any(|entry| entry["relative_path"] == "binary.dat"
                && entry["status"] == "added"
                && entry["content_kind"] == "binary"));
        assert_eq!(repo.head().unwrap().target(), before);
        let newer = commit(&repo, "newer", &[other], false);
        repo.reference(&merge::pending::conflict_ref("s"), newer, true, "fixture")
            .unwrap();
        let ctx = HostCtx::headless(store.clone());
        assert!(sync_service::resolve_reviewed(
            &ctx,
            "s",
            merge::resolve::ResolveAction::UseRemote,
            Some(&review.to_string())
        )
        .is_err());
        assert_eq!(repo.head().unwrap().target(), before);
        assert_eq!(
            std::fs::read_to_string(source.join("SKILL.md")).unwrap(),
            "uncommitted local work\n"
        );
        assert_eq!(store.list_pending_conflicts().unwrap().len(), 1);
    }

    #[test]
    fn instruction_binary_payload_is_listed_and_local_edits_invalidate_resource_review() {
        let _lock = central_repo::test_base_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(
            std::fs::canonicalize(temp.path()).unwrap().join("library"),
        ));
        let _reset = Reset;
        std::fs::create_dir_all(central_repo::skills_dir()).unwrap();
        let store = SkillStore::new(&temp.path().join("state.db")).unwrap();
        let key = "instructions/00000000-0000-0000-0000-000000000001";
        let bundle = resource_store::root().join(key);
        std::fs::create_dir_all(bundle.join("files")).unwrap();
        std::fs::write(
            resource_store::root().join("schema.json"),
            r#"{"version":1}"#,
        )
        .unwrap();
        std::fs::write(bundle.join("definition.json"),r#"{"id":"00000000-0000-0000-0000-000000000001","revision":"00000000-0000-0000-0000-000000000002","name":"Instructions","updated_at":1}"#).unwrap();
        std::fs::write(bundle.join("files/binary.dat"), [0, 1, 2]).unwrap();
        std::fs::write(bundle.join("files/guide.md"), "ours").unwrap();
        let repo = Repository::init(central_repo::skills_dir()).unwrap();
        let ours = commit(&repo, "ours", &[], true);
        std::fs::write(bundle.join("files/guide.md"), "other").unwrap();
        let other = commit(&repo, "other", &[ours], false);
        std::fs::write(bundle.join("files/guide.md"), "ours").unwrap();
        commit(
            &repo,
            &resource_sync::conflicts_trailer(&[key.into()]).unwrap(),
            &[ours, other],
            true,
        );
        assert_eq!(resource_sync::conflicts(&repo).unwrap().len(), 1);
        let review = preview(&store, "resource", key, None).unwrap();
        assert!(review["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["content_kind"] == "binary"));
        assert!(review["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["original_text"] == "ours" && entry["updated_text"] == "other"));
        std::fs::write(bundle.join("files/guide.md"), "local edit after review").unwrap();
        assert!(check(&store, Some(&review.to_string()), "resource", key, None).is_err());
        assert_eq!(
            std::fs::read_to_string(bundle.join("files/guide.md")).unwrap(),
            "local edit after review"
        );
        assert!(preview(&store, "resource", "instructions/../../outside", None).is_err());
    }
    #[test]
    fn same_name_presets_show_memberships_and_stale_choices_do_not_delete_a_preset() {
        let _lock = central_repo::test_base_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(
            std::fs::canonicalize(temp.path()).unwrap().join("library"),
        ));
        let _reset = Reset;
        std::fs::create_dir_all(central_repo::skills_dir()).unwrap();
        let store = SkillStore::new(&temp.path().join("state.db")).unwrap();
        skill(&store, &central_repo::skills_dir().join("example"));
        for id in ["first", "second"] {
            store
                .insert_scenario(&ScenarioRecord {
                    id: id.into(),
                    name: id.into(),
                    description: None,
                    icon: None,
                    sort_order: 0,
                    created_at: 1,
                    updated_at: 1,
                })
                .unwrap();
        }
        store.add_skill_to_scenario("first", "s").unwrap();
        sync_metadata::write_all_from_db_unlocked(&store).unwrap();
        sync_metadata::write_preset_reviews_unlocked(&BTreeMap::from([(
            "first".into(),
            "second".into(),
        )]))
        .unwrap();
        let repo = Repository::init(central_repo::skills_dir()).unwrap();
        commit(&repo, "preset conflict", &[], true);
        let review = preview(&store, "preset", "first", None).unwrap();
        assert!(review["files"][0]["original_text"]
            .as_str()
            .unwrap()
            .contains("Example"));
        assert!(!review["files"][0]["updated_text"]
            .as_str()
            .unwrap()
            .contains("Example"));
        store.add_skill_to_scenario("second", "s").unwrap();
        assert!(sync_service::resolve_preset_reviewed(
            &store,
            "first",
            sync_service::PresetChoice::Keep("second".into()),
            Some(&review.to_string())
        )
        .is_err());
        assert_eq!(store.get_all_scenarios().unwrap().len(), 2);
        assert_eq!(sync_metadata::preset_reviews(&store).unwrap().len(), 1);
        let current = preview(&store, "preset", "first", None).unwrap();
        sync_service::resolve_preset_reviewed(
            &store,
            "first",
            sync_service::PresetChoice::KeepBoth,
            Some(&current.to_string()),
        )
        .unwrap();
        assert_eq!(store.get_all_scenarios().unwrap().len(), 2);
        assert!(sync_metadata::preset_reviews(&store).unwrap().is_empty());
        assert!(
            repo.head()
                .unwrap()
                .peel_to_commit()
                .unwrap()
                .parent_count()
                > 0
        );
    }
    #[test]
    fn mcp_review_keeps_credential_references_and_rejects_nonportable_local_values() {
        let _lock = central_repo::test_base_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(
            std::fs::canonicalize(temp.path()).unwrap().join("library"),
        ));
        let _reset = Reset;
        std::fs::create_dir_all(central_repo::skills_dir()).unwrap();
        let store = SkillStore::new(&temp.path().join("state.db")).unwrap();
        let id = "00000000-0000-0000-0000-000000000003";
        let key = format!("mcps/{id}");
        let file = resource_store::root().join(format!("{key}.json"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            resource_store::root().join("schema.json"),
            r#"{"version":1}"#,
        )
        .unwrap();
        let mut definition = json!({"id":id,"revision":"00000000-0000-0000-0000-000000000004","name":"Fixture MCP","transport":"http","server":{"url":"https://local.example.test/mcp","headers":{"Authorization":"Bearer ${env:SYNC_REVIEW_FIXTURE_TOKEN}"}},"updatedAt":"2026-11-01T00:00:00Z"});
        std::fs::write(&file, definition.to_string()).unwrap();
        let repo = Repository::init(central_repo::skills_dir()).unwrap();
        let ours = commit(&repo, "ours", &[], true);
        definition["server"]["url"] = json!("https://other.example.test/mcp");
        std::fs::write(&file, definition.to_string()).unwrap();
        let other = commit(&repo, "other", &[ours], false);
        definition["server"]["url"] = json!("https://local.example.test/mcp");
        std::fs::write(&file, definition.to_string()).unwrap();
        commit(
            &repo,
            &resource_sync::conflicts_trailer(std::slice::from_ref(&key)).unwrap(),
            &[ours, other],
            true,
        );
        let review = preview(&store, "resource", &key, None).unwrap();
        assert!(review["files"][0]["original_text"]
            .as_str()
            .unwrap()
            .contains("${env:SYNC_REVIEW_FIXTURE_TOKEN}"));
        assert!(review["files"][0]["updated_text"]
            .as_str()
            .unwrap()
            .contains("other.example.test"));
        definition["server"]["headers"]["Authorization"] = json!("forbidden-inline-fixture-value");
        std::fs::write(&file, definition.to_string()).unwrap();
        let error = preview(&store, "resource", &key, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Nonportable MCP"));
        assert!(!error.contains("forbidden-inline-fixture-value"));
    }
    #[test]
    #[cfg(unix)]
    fn reviewed_preset_apply_removes_only_its_owned_link_and_keeps_library_content() {
        let _lock = central_repo::test_base_dir_lock();
        let temp = tempfile::tempdir().unwrap();
        central_repo::set_test_base_dir_override(Some(
            std::fs::canonicalize(temp.path()).unwrap().join("library"),
        ));
        let _reset = Reset;
        std::fs::create_dir_all(central_repo::skills_dir()).unwrap();
        let store = SkillStore::new(&temp.path().join("state.db")).unwrap();
        let source = central_repo::skills_dir().join("example");
        skill(&store, &source);
        store
            .insert_scenario(&ScenarioRecord {
                id: "empty".into(),
                name: "Empty preset".into(),
                description: None,
                icon: None,
                sort_order: 0,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        let target = temp.path().join("owned-agent-link");
        std::os::unix::fs::symlink(&source, &target).unwrap();
        store
            .insert_target(&SkillTargetRecord {
                id: "t".into(),
                skill_id: "s".into(),
                tool: "codex".into(),
                target_path: target.display().to_string(),
                mode: "symlink".into(),
                status: "ok".into(),
                synced_at: Some(1),
                last_error: None,
                source_hash: None,
            })
            .unwrap();
        let review = preview(&store, "switch", "empty", None).unwrap();
        assert_eq!(review["removals"][0]["path"], target.display().to_string());
        scenario_service::apply_scenario_reviewed(&store, "empty", Some(&review.to_string()))
            .unwrap();
        assert_eq!(
            store.get_active_scenario_id().unwrap().as_deref(),
            Some("empty")
        );
        assert!(target.symlink_metadata().is_err());
        assert!(store.get_targets_for_skill("s").unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(source.join("SKILL.md")).unwrap(),
            "library version\n"
        );
    }
}
