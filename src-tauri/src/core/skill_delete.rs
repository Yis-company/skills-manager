//! Removing managed skills from the library, shared by the GUI and the CLI.

use serde::Serialize;
use std::path::PathBuf;

use crate::core::{
    audit_log::AuditDraft, error::AppError, skill_store::SkillStore, sync_engine, sync_metadata,
};

#[derive(Debug, Serialize)]
pub struct BatchDeleteSkillsResult {
    pub deleted: usize,
    pub failed: Vec<String>,
}

pub fn delete_managed_skills_by_ids(
    store: &SkillStore,
    skill_ids: &[String],
) -> Result<BatchDeleteSkillsResult, AppError> {
    sync_metadata::with_repo_lock("delete skills", || {
        let mut deleted = 0;
        let mut failed = Vec::new();

        for skill_id in skill_ids {
            let Some(skill) = store.get_skill_by_id(skill_id)? else {
                store.log_audit(
                    AuditDraft::new("remove")
                        .skill(skill_id.clone(), "")
                        .fail("not found"),
                );
                failed.push(skill_id.clone());
                continue;
            };

            let targets = store.get_targets_for_skill(skill_id)?;
            let all_targets = store.get_all_targets()?;
            for target in &targets {
                // Another skill may still use this physical path. Rows for
                // this skill all disappear together, so they are not survivors.
                let still_referenced = all_targets.iter().any(|other| {
                    other.target_path == target.target_path && other.skill_id != target.skill_id
                });
                if !still_referenced {
                    sync_engine::remove_recorded_target_or_warn(
                        &PathBuf::from(&target.target_path),
                        &target.mode,
                    );
                }
            }

            let central = PathBuf::from(&skill.central_path);
            if central.exists() {
                std::fs::remove_dir_all(&central).ok();
            }

            store.delete_skill(skill_id)?;
            store.log_audit(
                AuditDraft::new("remove")
                    .skill(skill_id.clone(), skill.name.clone())
                    .ok(),
            );
            deleted += 1;
        }

        if deleted > 0 {
            sync_metadata::write_all_from_db_unlocked(store)?;
        }

        Ok(BatchDeleteSkillsResult { deleted, failed })
    })
    .map_err(AppError::db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::skill_store::SkillTargetRecord;
    use crate::core::test_support::{sample_skill, test_repo, write_skill_dir};
    use std::fs;

    #[test]
    fn batch_delete_removes_skills_targets_and_stale_metadata_once() {
        let repo = test_repo();
        let skill_one_dir = write_skill_dir("skill-one");
        let skill_two_dir = write_skill_dir("skill-two");
        repo.store
            .insert_skill(&sample_skill("skill-1", "skill-one", &skill_one_dir))
            .unwrap();
        repo.store
            .insert_skill(&sample_skill("skill-2", "skill-two", &skill_two_dir))
            .unwrap();

        let target_dir = repo._tmp.path().join("target-skill-one");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("SKILL.md"), "# target").unwrap();
        repo.store
            .insert_target(&SkillTargetRecord {
                id: "target-1".to_string(),
                skill_id: "skill-1".to_string(),
                tool: "cursor".to_string(),
                target_path: target_dir.to_string_lossy().to_string(),
                mode: "copy".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
                source_hash: None,
            })
            .unwrap();

        sync_metadata::write_all_from_db_unlocked(&repo.store).unwrap();
        assert!(sync_metadata::metadata_dir()
            .join("skills/skill-1.json")
            .exists());
        assert!(sync_metadata::metadata_dir()
            .join("skills/skill-2.json")
            .exists());

        let result = delete_managed_skills_by_ids(
            &repo.store,
            &["skill-1".to_string(), "missing-skill".to_string()],
        )
        .unwrap();

        assert_eq!(result.deleted, 1);
        assert_eq!(result.failed, vec!["missing-skill".to_string()]);
        assert!(repo.store.get_skill_by_id("skill-1").unwrap().is_none());
        assert!(repo.store.get_skill_by_id("skill-2").unwrap().is_some());
        assert!(!skill_one_dir.exists());
        assert!(skill_two_dir.exists());
        assert!(!target_dir.exists());
        assert!(!sync_metadata::metadata_dir()
            .join("skills/skill-1.json")
            .exists());
        assert!(sync_metadata::metadata_dir()
            .join("skills/skill-2.json")
            .exists());
    }

    #[test]
    fn deleting_a_skill_preserves_user_content_that_replaced_a_recorded_link() {
        let repo = test_repo();
        let dir = write_skill_dir("my-skill");
        repo.store
            .insert_skill(&sample_skill("s1", "my-skill", &dir))
            .unwrap();

        let target = repo._tmp.path().join("agent").join("my-skill");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("mine.txt"), "DO_NOT_OVERWRITE").unwrap();
        repo.store
            .insert_target(&SkillTargetRecord {
                id: "t1".to_string(),
                skill_id: "s1".to_string(),
                tool: "test_agent".to_string(),
                target_path: target.to_string_lossy().to_string(),
                mode: "symlink".to_string(),
                status: "ok".to_string(),
                synced_at: Some(1),
                last_error: None,
                source_hash: None,
            })
            .unwrap();

        let result = delete_managed_skills_by_ids(&repo.store, &["s1".to_string()]).unwrap();

        assert_eq!(result.deleted, 1);
        assert_eq!(
            fs::read_to_string(target.join("mine.txt")).unwrap(),
            "DO_NOT_OVERWRITE"
        );
        assert!(repo.store.get_targets_for_skill("s1").unwrap().is_empty());
    }

    #[test]
    fn deleting_a_skill_removes_a_target_shared_by_its_tool_rows() {
        let repo = test_repo();
        let dir = write_skill_dir("my-skill");
        repo.store
            .insert_skill(&sample_skill("s1", "my-skill", &dir))
            .unwrap();

        let target = repo._tmp.path().join("agent").join("my-skill");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("SKILL.md"), "deployed content").unwrap();
        for (id, tool) in [("t1", "agent_a"), ("t2", "agent_b")] {
            repo.store
                .insert_target(&SkillTargetRecord {
                    id: id.to_string(),
                    skill_id: "s1".to_string(),
                    tool: tool.to_string(),
                    target_path: target.to_string_lossy().to_string(),
                    mode: "copy".to_string(),
                    status: "ok".to_string(),
                    synced_at: Some(1),
                    last_error: None,
                    source_hash: None,
                })
                .unwrap();
        }

        let result = delete_managed_skills_by_ids(&repo.store, &["s1".to_string()]).unwrap();

        assert_eq!(result.deleted, 1);
        assert!(!target.exists());
        assert!(repo.store.get_targets_for_skill("s1").unwrap().is_empty());
    }
}
