//! Apply synced changes to this host's agents (SID-99). After the library
//! changes under the database (a pull, a sync, a conflict choice, a restore,
//! a clone) and at startup, the active preset is applied again:
//! - skills that left the preset, or one of its agents, are removed;
//! - skills that left the library are removed, links and copies alike;
//! - renamed skills move to their new folder name;
//! - anything missing is deployed.
//!
//! Skills added directly to an agent stay until the next preset switch
//! (SID-98). A copy edited in its agent folder is never overwritten or
//! removed: it is flagged "Edited here" until the user picks "Use Sync
//! version" or "Keep my edit". Deployed instructions and MCPs follow their
//! library definitions when the update applies cleanly, and otherwise wait
//! for review. Each change is written to the audit log, which doubles as the
//! host's agent log.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::audit_log::AuditDraft;
use super::central_repo;
use super::error::AppError;
use super::host::HostCtx;
use super::scenario_service::{
    self, DeployIntent, ScenarioSyncTarget, STATUS_EDITED, STATUS_EDITED_LEFT,
};
use super::skill_store::{SkillStore, SkillTargetRecord};
use super::sync_engine;
use super::{instructions, mcps};

/// Audit actions of the agent log.
pub const ACTION_DEPLOY: &str = "agent_deploy";
pub const ACTION_REMOVE: &str = "agent_remove";
pub const ACTION_FLAG: &str = "agent_flag";
pub const ACTION_RESOLVE: &str = "agent_resolve_edit";
/// An instruction or MCP update that meets edits on this host and waits.
pub const ACTION_REVIEW: &str = "agent_review";

/// What this host had on its agents before the library changed. The
/// database forgets a deleted skill's deployments (cascade), so this is the
/// only record of what to clean up.
#[derive(Default)]
pub struct Baseline {
    /// (skill id, agent) pairs the active preset deployed.
    preset_targets: HashSet<(String, String)>,
    targets: Vec<SkillTargetRecord>,
    /// Skill id → (name, folder in the library).
    skills: HashMap<String, (String, PathBuf)>,
}

/// Read the baseline before the library changes. A database error yields a
/// partial baseline, which only skips removals.
pub fn capture(store: &SkillStore) -> Baseline {
    let mut preset_targets = HashSet::new();
    if let Ok(Some(active)) = store.get_active_scenario_id() {
        for skill_id in store
            .get_skill_ids_for_scenario(&active)
            .unwrap_or_default()
        {
            for tool in store
                .get_enabled_tools_for_scenario_skill(&active, &skill_id)
                .unwrap_or_default()
            {
                preset_targets.insert((skill_id.clone(), tool));
            }
        }
    }
    Baseline {
        preset_targets,
        targets: store.get_all_targets().unwrap_or_default(),
        skills: store
            .get_all_skills()
            .unwrap_or_default()
            .into_iter()
            .map(|skill| (skill.id, (skill.name, PathBuf::from(skill.central_path))))
            .collect(),
    }
}

/// Apply the active preset again after the library changed. Never fails the
/// caller: the library change already landed. Returns what was recorded.
pub fn reapply_active_preset(store: &SkillStore, before: &Baseline) -> Vec<AuditDraft> {
    let entries = reapply(store, before).unwrap_or_else(|e| {
        log::warn!("sync apply: {e}");
        vec![AuditDraft::new(ACTION_DEPLOY).fail(e.to_string())]
    });
    for entry in &entries {
        store.log_audit(entry.clone());
    }
    entries
}

fn reapply(store: &SkillStore, before: &Baseline) -> Result<Vec<AuditDraft>, AppError> {
    let Some(active) = active_preset(store)? else {
        return Ok(Vec::new());
    };
    let mut desired = scenario_service::collect_scenario_sync_targets(store, &active)?;
    follow_renames(&mut desired, before);
    let desired_keys: HashSet<(String, String)> = desired
        .iter()
        .map(|t| (t.skill_id.clone(), t.tool.clone()))
        .collect();

    let mut entries = Vec::new();
    for target in &before.targets {
        let key = (target.skill_id.clone(), target.tool.clone());
        let left_preset = before.preset_targets.contains(&key) && !desired_keys.contains(&key);
        let left_library = store
            .get_skill_by_id(&target.skill_id)
            .map_err(AppError::db)?
            .is_none();
        if !left_preset && !left_library {
            continue;
        }
        let name = before
            .skills
            .get(&target.skill_id)
            .map_or(target.skill_id.as_str(), |(name, _)| name.as_str());
        let entry = |action| {
            AuditDraft::new(action)
                .skill(&target.skill_id, name)
                .tool(&target.tool)
        };
        if scenario_service::keep_if_edited(store, target, STATUS_EDITED_LEFT) {
            entries.push(entry(ACTION_FLAG).detail("Edited here").ok());
            continue;
        }
        sync_engine::remove_recorded_target_or_warn(Path::new(&target.target_path), &target.mode);
        store
            .delete_target(&target.skill_id, &target.tool)
            .map_err(AppError::db)?;
        entries.push(entry(ACTION_REMOVE).ok());
    }

    type TargetState = HashMap<(String, String), (Option<i64>, String)>;
    let state = |store: &SkillStore| -> Result<TargetState, AppError> {
        Ok(store
            .get_all_targets()
            .map_err(AppError::db)?
            .into_iter()
            .map(|t| ((t.skill_id, t.tool), (t.synced_at, t.status)))
            .collect())
    };
    let before_sync = state(store)?;
    let refusals = scenario_service::sync_desired_targets(store, &desired)?;
    let after_sync = state(store)?;
    for target in &desired {
        let key = (target.skill_id.clone(), target.tool.clone());
        let entry = |action| {
            AuditDraft::new(action)
                .skill(&target.skill_id, &target.skill_name)
                .tool(&target.tool)
        };
        let was = before_sync.get(&key);
        match after_sync.get(&key) {
            Some((_, status))
                if status == STATUS_EDITED
                    && was.map(|(_, s)| s.as_str()) != Some(STATUS_EDITED) =>
            {
                entries.push(entry(ACTION_FLAG).detail("Edited here").ok());
            }
            Some((at, _)) if was.map(|(at, _)| at) == Some(at) => {}
            Some(_) => entries.push(entry(ACTION_DEPLOY).ok()),
            None => {
                let reason = refusals
                    .iter()
                    .find(|r| r.target == target.target)
                    .map_or_else(|| "not deployed".to_string(), |r| r.reason.clone());
                entries.push(entry(ACTION_DEPLOY).fail(reason));
            }
        }
    }
    Ok(entries)
}

/// Apply the user's choice for a copy flagged "Edited here" (SID-99).
/// "Keep my edit" leaves the copy alone until Sync changes the skill again,
/// which flags it again. "Use Sync version" moves the edit to a backup folder
/// under `edited-backups/`, then puts the library version back, or nothing if
/// the skill left this agent.
pub fn resolve_edited_copy(
    store: &SkillStore,
    skill_id: &str,
    tool: &str,
    keep_mine: bool,
) -> Result<(), AppError> {
    let target = store
        .get_targets_for_skill(skill_id)
        .map_err(AppError::db)?
        .into_iter()
        .find(|t| t.tool == tool && t.status.starts_with(STATUS_EDITED))
        .ok_or_else(|| AppError::not_found("No edited copy of this skill on that agent"))?;
    let skill = store.get_skill_by_id(skill_id).map_err(AppError::db)?;
    if keep_mine {
        let hash = skill.as_ref().and_then(|s| s.content_hash.as_deref());
        store
            .set_target_status(skill_id, tool, "ok", hash)
            .map_err(AppError::db)?;
    } else {
        let path = PathBuf::from(&target.target_path);
        let backup = central_repo::base_dir()
            .join("edited-backups")
            .join(chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string())
            .join(tool)
            .join(path.file_name().unwrap_or_default());
        if let Some(parent) = backup.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::io)?;
        }
        std::fs::rename(&path, &backup).map_err(AppError::io)?;
        store.delete_target(skill_id, tool).map_err(AppError::db)?;
        if target.status == STATUS_EDITED && skill.is_some() {
            scenario_service::sync_single_skill_to_tool(
                store,
                skill_id,
                tool,
                DeployIntent::Managed,
            )?;
        }
    }
    store.log_audit(
        AuditDraft::new(ACTION_RESOLVE)
            .skill(
                skill_id,
                skill.map_or_else(|| skill_id.to_string(), |s| s.name),
            )
            .tool(tool)
            .detail(if keep_mine {
                "keep my edit"
            } else {
                "use Sync version"
            })
            .ok(),
    );
    Ok(())
}

/// Deployed instructions and MCPs follow their library definitions after a
/// sync (SID-99). An update that applies cleanly happens on its own; one that
/// meets edits made on this host, or can't be previewed, waits for review in
/// the agent log. Which items are deployed stays per host. Runs through the
/// regular preview and apply, which take the repo lock: call it after the
/// sync released it.
pub fn reapply_resources(ctx: &HostCtx) -> Vec<AuditDraft> {
    let mut entries = Vec::new();
    let instructions = |request| instructions::dispatch(ctx, request);
    let mcps = |request| mcps::dispatch(ctx, request);
    let deployed = instructions(json!({"action": "deployments"})).unwrap_or_default();
    for item in deployed["items"].as_array().into_iter().flatten() {
        let id = item["instruction_id"].as_str().unwrap_or_default();
        // A bundle deleted in the library keeps its files on this host.
        let Ok(found) = instructions(json!({"action": "get", "id": id})) else {
            continue;
        };
        let definition = &found["item"];
        if definition["revision"] == item["definition_revision"] {
            continue;
        }
        let update = instructions(
            json!({"action": "preview", "target": item["target"], "instruction_id": id}),
        )
        .and_then(|preview| {
            let conflict = preview["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|change| change["status"] == "conflict");
            match conflict {
                Some(change) => Err(AppError::invalid_input(format!(
                    "{}: {}",
                    change["path"].as_str().unwrap_or_default(),
                    change["conflict"].as_str().unwrap_or("edited here")
                ))),
                None => Ok(preview),
            }
        });
        let applied = update.as_ref().map_err(|e| e.to_string()).map(|preview| {
            instructions(json!({"action": "apply", "preview_id": preview["preview_id"]}))
        });
        entries.push(resource_entry(
            "instruction",
            id,
            definition,
            &item["target"],
            applied,
        ));
    }

    let deployed = mcps(json!({"action": "deployments"})).unwrap_or_default();
    for item in deployed["deployments"].as_array().into_iter().flatten() {
        let id = item["definitionId"].as_str().unwrap_or_default();
        let Ok(found) = mcps(json!({"action": "get", "id": id})) else {
            continue;
        };
        let definition = &found["definition"];
        if definition["revision"] == item["revision"] {
            continue;
        }
        let operation =
            json!({"kind": "update", "definitionId": id, "serverName": item["serverName"]});
        let applied =
            mcps(json!({"action": "preview", "target": item["target"], "operations": [operation]}))
                .map_err(|e| e.to_string())
                .map(|preview| mcps(json!({"action": "apply", "previewId": preview["previewId"]})));
        entries.push(resource_entry(
            "MCP",
            id,
            definition,
            &item["target"],
            applied,
        ));
    }

    for entry in &entries {
        ctx.store.log_audit(entry.clone());
    }
    entries
}

/// Agent log entry for one instruction or MCP update: the outer result is the
/// preview (an error waits for review), the inner one the apply.
fn resource_entry(
    kind: &str,
    id: &str,
    definition: &Value,
    target: &Value,
    applied: Result<Result<Value, AppError>, String>,
) -> AuditDraft {
    let agent = target["agent_key"]
        .as_str()
        .or_else(|| target["agentKey"].as_str())
        .unwrap_or_default();
    let entry = |action| {
        AuditDraft::new(action)
            .skill(id, definition["name"].as_str().unwrap_or(id))
            .tool(agent)
    };
    match applied {
        Err(reason) => entry(ACTION_REVIEW).fail(format!("{kind}: {reason}")),
        Ok(Ok(result))
            if result["partial"] != true
                && result["applied"] != false
                && result["failed"].as_array().is_none_or(Vec::is_empty) =>
        {
            entry(ACTION_DEPLOY).detail(kind).ok()
        }
        Ok(Ok(result)) => entry(ACTION_DEPLOY).fail(format!("{kind}: {}", result["status"])),
        Ok(Err(e)) => entry(ACTION_DEPLOY).fail(format!("{kind}: {e}")),
    }
}

/// The active preset. One deleted on another host can't stay active here:
/// the first remaining preset takes over, as at startup.
fn active_preset(store: &SkillStore) -> Result<Option<String>, AppError> {
    let presets = store.get_all_scenarios().map_err(AppError::db)?;
    let active = store.get_active_scenario_id().map_err(AppError::db)?;
    if let Some(id) = active.filter(|id| presets.iter().any(|p| p.id == *id)) {
        return Ok(Some(id));
    }
    let Some(first) = presets.first() else {
        return Ok(None);
    };
    store.set_active_scenario(&first.id).map_err(AppError::db)?;
    Ok(Some(first.id.clone()))
}

/// A renamed skill keeps its recorded deployment path, which still carries
/// the old folder name. Point it at the new name; the sync then moves it.
fn follow_renames(desired: &mut [ScenarioSyncTarget], before: &Baseline) {
    for target in desired {
        let Some((name, old_source)) = before.skills.get(&target.skill_id) else {
            continue;
        };
        if *old_source == target.source {
            continue;
        }
        let old_name = sync_engine::target_dir_name(old_source, name);
        if target.target.file_name() == Some(OsStr::new(&old_name)) {
            target.target = target.target.with_file_name(sync_engine::target_dir_name(
                &target.source,
                &target.skill_name,
            ));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::core::skill_store::ScenarioRecord;
    use crate::core::test_support::{sample_skill, test_repo, write_skill_dir, TestRepo};
    use crate::core::tool_adapters::{self, CustomToolDef};
    use std::fs;

    /// A library whose only agent deploys into a temp folder, with preset
    /// "web" active.
    fn setup() -> (TestRepo, PathBuf) {
        let repo = test_repo();
        let agent_dir = repo._tmp.path().join("agent-skills");
        fs::create_dir_all(&agent_dir).unwrap();
        let custom = vec![CustomToolDef {
            key: "test_agent".to_string(),
            display_name: "Test Agent".to_string(),
            skills_dir: agent_dir.to_string_lossy().to_string(),
            project_relative_skills_dir: None,
            category: Default::default(),
        }];
        let builtin: Vec<String> = tool_adapters::default_tool_adapters()
            .into_iter()
            .map(|a| a.key)
            .collect();
        let store = &repo.store;
        store
            .set_setting("custom_tools", &serde_json::to_string(&custom).unwrap())
            .unwrap();
        store
            .set_setting("disabled_tools", &serde_json::to_string(&builtin).unwrap())
            .unwrap();
        store
            .insert_scenario(&ScenarioRecord {
                id: "web".to_string(),
                name: "Web".to_string(),
                description: None,
                icon: None,
                sort_order: 0,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        store.set_active_scenario("web").unwrap();
        (repo, agent_dir)
    }

    fn add_skill(store: &SkillStore, id: &str, in_preset: bool) -> PathBuf {
        let dir = write_skill_dir(id);
        store.insert_skill(&sample_skill(id, id, &dir)).unwrap();
        if in_preset {
            store.add_skill_to_scenario("web", id).unwrap();
        }
        dir
    }

    fn actions(entries: &[AuditDraft]) -> Vec<(String, String, bool)> {
        entries
            .iter()
            .map(|e| {
                (
                    e.action.clone(),
                    e.skill_name.clone().unwrap_or_default(),
                    e.success,
                )
            })
            .collect()
    }

    #[test]
    fn a_skill_added_to_the_preset_elsewhere_gets_deployed() {
        let (repo, agent_dir) = setup();
        let store = &repo.store;
        add_skill(store, "alpha", true);
        reapply_active_preset(store, &capture(store));
        let before = capture(store);

        // The pull brings a new skill in the active preset.
        let beta = add_skill(store, "beta", true);
        let entries = reapply_active_preset(store, &before);

        assert_eq!(fs::read_link(agent_dir.join("beta")).unwrap(), beta);
        assert_eq!(
            actions(&entries),
            vec![(ACTION_DEPLOY.to_string(), "beta".to_string(), true)]
        );
        // The agent log keeps it.
        let log = store.list_audit(Some(1)).unwrap();
        assert_eq!(log[0].action, ACTION_DEPLOY);
        assert_eq!(log[0].tool.as_deref(), Some("test_agent"));
    }

    #[test]
    fn a_skill_deleted_elsewhere_is_removed_from_agents() {
        let (repo, agent_dir) = setup();
        let store = &repo.store;
        let alpha = add_skill(store, "alpha", true);
        reapply_active_preset(store, &capture(store));
        assert!(agent_dir.join("alpha").is_symlink());
        let before = capture(store);

        // The pull deletes the skill: its folder and its database rows go.
        fs::remove_dir_all(&alpha).unwrap();
        store.delete_skill("alpha").unwrap();
        let entries = reapply_active_preset(store, &before);

        assert!(!agent_dir.join("alpha").exists() && !agent_dir.join("alpha").is_symlink());
        assert_eq!(
            actions(&entries),
            vec![(ACTION_REMOVE.to_string(), "alpha".to_string(), true)]
        );
    }

    #[test]
    fn a_skill_that_left_the_preset_goes_but_direct_deploys_stay() {
        let (repo, agent_dir) = setup();
        let store = &repo.store;
        add_skill(store, "alpha", true);
        add_skill(store, "beta", true);
        reapply_active_preset(store, &capture(store));
        // Added directly to the agent, outside the preset.
        add_skill(store, "gamma", false);
        scenario_service::sync_single_skill_to_tool(
            store,
            "gamma",
            "test_agent",
            scenario_service::DeployIntent::Managed,
        )
        .unwrap();
        assert!(agent_dir.join("gamma").is_symlink());
        let before = capture(store);

        store.remove_skill_from_scenario("web", "beta").unwrap();
        let entries = reapply_active_preset(store, &before);

        assert!(agent_dir.join("alpha").is_symlink());
        assert!(!agent_dir.join("beta").is_symlink());
        assert!(agent_dir.join("gamma").is_symlink());
        assert_eq!(
            actions(&entries),
            vec![(ACTION_REMOVE.to_string(), "beta".to_string(), true)]
        );
    }

    #[test]
    fn a_renamed_skill_moves_to_its_new_folder() {
        let (repo, agent_dir) = setup();
        let store = &repo.store;
        let alpha = add_skill(store, "alpha", true);
        reapply_active_preset(store, &capture(store));
        let before = capture(store);

        // The pull renames the skill's folder.
        let renamed = alpha.with_file_name("alpha-renamed");
        fs::rename(&alpha, &renamed).unwrap();
        let mut record = store.get_skill_by_id("alpha").unwrap().unwrap();
        record.central_path = renamed.to_string_lossy().to_string();
        store.upsert_skill(&record).unwrap();
        reapply_active_preset(store, &before);

        assert!(!agent_dir.join("alpha").is_symlink());
        assert_eq!(
            fs::read_link(agent_dir.join("alpha-renamed")).unwrap(),
            renamed
        );
    }

    /// Copy mode, skill "alpha" deployed at version h1, then edited in the
    /// agent folder while Sync brings version h2: flagged "Edited here".
    fn flagged_copy() -> (TestRepo, PathBuf) {
        let (repo, agent_dir) = setup();
        let store = &repo.store;
        store.set_setting("sync_mode", "copy").unwrap();
        add_skill(store, "alpha", true);
        set_version(store, "alpha", "h1", "library v1");
        reapply_active_preset(store, &capture(store));
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(agent_dir.join("alpha/SKILL.md"), "my edit").unwrap();

        let before = capture(store);
        set_version(store, "alpha", "h2", "library v2");
        let entries = reapply_active_preset(store, &before);
        assert_eq!(
            actions(&entries),
            vec![(ACTION_FLAG.to_string(), "alpha".to_string(), true)]
        );
        (repo, agent_dir)
    }

    fn set_version(store: &SkillStore, id: &str, hash: &str, content: &str) {
        let mut record = store.get_skill_by_id(id).unwrap().unwrap();
        fs::write(Path::new(&record.central_path).join("SKILL.md"), content).unwrap();
        record.content_hash = Some(hash.to_string());
        store.upsert_skill(&record).unwrap();
    }

    fn status(store: &SkillStore, id: &str) -> String {
        store.get_targets_for_skill(id).unwrap()[0].status.clone()
    }

    #[test]
    fn an_edited_copy_is_flagged_not_overwritten() {
        let (repo, agent_dir) = flagged_copy();
        assert_eq!(
            fs::read_to_string(agent_dir.join("alpha/SKILL.md")).unwrap(),
            "my edit"
        );
        assert_eq!(status(&repo.store, "alpha"), STATUS_EDITED);
    }

    #[test]
    fn keep_my_edit_is_flagged_again_on_the_next_change() {
        let (repo, agent_dir) = flagged_copy();
        let store = &repo.store;
        resolve_edited_copy(store, "alpha", "test_agent", true).unwrap();
        assert_eq!(status(store, "alpha"), "ok");
        assert!(reapply_active_preset(store, &capture(store)).is_empty());

        let before = capture(store);
        set_version(store, "alpha", "h3", "library v3");
        reapply_active_preset(store, &before);
        assert_eq!(status(store, "alpha"), STATUS_EDITED);
        assert_eq!(
            fs::read_to_string(agent_dir.join("alpha/SKILL.md")).unwrap(),
            "my edit"
        );
    }

    #[test]
    fn use_sync_version_backs_up_the_edit_and_updates_the_copy() {
        let (repo, agent_dir) = flagged_copy();
        let store = &repo.store;
        resolve_edited_copy(store, "alpha", "test_agent", false).unwrap();

        assert_eq!(
            fs::read_to_string(agent_dir.join("alpha/SKILL.md")).unwrap(),
            "library v2"
        );
        assert_eq!(status(store, "alpha"), "ok");
        let backups: Vec<_> =
            walkdir::WalkDir::new(central_repo::base_dir().join("edited-backups"))
                .into_iter()
                .flatten()
                .filter(|e| e.file_name() == "SKILL.md")
                .collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0].path().ends_with("test_agent/alpha/SKILL.md"));
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "my edit");
    }

    #[test]
    fn a_switch_keeps_an_edited_copy_until_the_user_chooses() {
        let (repo, agent_dir) = flagged_copy();
        let store = &repo.store;
        store
            .insert_scenario(&ScenarioRecord {
                id: "empty".to_string(),
                name: "Empty".to_string(),
                description: None,
                icon: None,
                sort_order: 1,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        scenario_service::apply_scenario_to_default(store, "empty").unwrap();
        assert_eq!(
            fs::read_to_string(agent_dir.join("alpha/SKILL.md")).unwrap(),
            "my edit"
        );
        assert_eq!(status(store, "alpha"), STATUS_EDITED_LEFT);

        // "Use Sync version" now means: not on this agent.
        resolve_edited_copy(store, "alpha", "test_agent", false).unwrap();
        assert!(!agent_dir.join("alpha").exists());
        assert!(store.get_targets_for_skill("alpha").unwrap().is_empty());
    }

    #[test]
    fn deployed_instructions_and_mcps_follow_clean_updates_and_wait_on_edits() {
        use crate::core::host::NoopEvents;
        use crate::core::skill_store::ProjectRecord;
        use std::sync::Arc;

        let repo = test_repo();
        // Library paths must not run through the /var symlink on macOS.
        central_repo::set_test_base_dir_override(Some(
            repo._tmp.path().canonicalize().unwrap().join("repo"),
        ));
        let store = SkillStore::new(&repo._tmp.path().join("resources.db")).unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let project = |id: &str| {
            let dir = repo._tmp.path().join(id);
            fs::create_dir_all(&dir).unwrap();
            ctx.store
                .insert_project(&ProjectRecord {
                    id: id.into(),
                    name: id.into(),
                    path: dir.to_string_lossy().into(),
                    workspace_type: "project".into(),
                    linked_agent_key: None,
                    linked_agent_name: None,
                    disabled_path: None,
                    sort_order: 0,
                    created_at: 0,
                    updated_at: 0,
                    agent_keys: None,
                    deploy_mode: "link".into(),
                })
                .unwrap();
            dir
        };
        let clean = project("clean");
        let edited = project("edited");
        let ins = |request| instructions::dispatch(&ctx, request).unwrap();
        let mcp = |request| mcps::dispatch(&ctx, request).unwrap();

        let bundle = ins(json!({"action": "save", "name": "Rules", "files": {"CLAUDE.md": "v1"}}));
        let bundle_id = bundle["item"]["id"].clone();
        for project_id in ["clean", "edited"] {
            let target = json!({"agent_key": "claude_code", "project_id": project_id});
            let preview =
                ins(json!({"action": "preview", "target": target, "instruction_id": bundle_id}));
            ins(json!({"action": "apply", "preview_id": preview["preview_id"]}));
        }
        fs::write(edited.join("CLAUDE.md"), "my edit").unwrap();

        let server = |args: &[&str]| json!({"command": "npx", "args": args});
        let saved = mcp(json!({"action": "save", "definition": {
            "name": "docs", "transport": "stdio", "server": server(&["docs@1"])
        }}));
        let definition = &saved["definition"];
        let mcp_target = json!({"agentKey": "cursor", "projectId": "clean"});
        let operation = json!({"kind": "deploy", "definitionId": definition["id"]});
        let preview =
            mcp(json!({"action": "preview", "target": mcp_target, "operations": [operation]}));
        mcp(json!({"action": "apply", "previewId": preview["previewId"]}));

        // The pull brings new versions of both.
        ins(json!({"action": "save", "id": bundle_id, "name": "Rules",
            "files": {"CLAUDE.md": "v2"}, "expected_revision": bundle["item"]["revision"]}));
        mcp(
            json!({"action": "save", "expectedRevision": definition["revision"], "definition": {
                "id": definition["id"], "name": "docs", "transport": "stdio", "server": server(&["docs@2"])
            }}),
        );
        let entries = reapply_resources(&ctx);

        assert_eq!(fs::read_to_string(clean.join("CLAUDE.md")).unwrap(), "v2");
        assert_eq!(
            fs::read_to_string(edited.join("CLAUDE.md")).unwrap(),
            "my edit"
        );
        let cursor_config = fs::read_to_string(clean.join(".cursor/mcp.json")).unwrap();
        assert!(cursor_config.contains("docs@2"), "{cursor_config}");
        // Deployments come back in id order, so compare sorted.
        let summary = |entries: &[AuditDraft]| {
            let mut summary: Vec<_> = entries
                .iter()
                .map(|e| {
                    (
                        e.action.clone(),
                        e.skill_name.clone().unwrap_or_default(),
                        e.success,
                    )
                })
                .collect();
            summary.sort();
            summary
        };
        assert_eq!(
            summary(&entries),
            vec![
                (ACTION_DEPLOY.to_string(), "Rules".to_string(), true),
                (ACTION_DEPLOY.to_string(), "docs".to_string(), true),
                (ACTION_REVIEW.to_string(), "Rules".to_string(), false),
            ]
        );
        // Up to date now; only the edited copy still waits.
        assert_eq!(
            summary(&reapply_resources(&ctx)),
            vec![(ACTION_REVIEW.to_string(), "Rules".to_string(), false)]
        );
    }
}
