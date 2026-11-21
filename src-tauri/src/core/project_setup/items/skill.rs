use std::path::{Path, PathBuf};

use super::{Reason, Run, Status, Step};
use crate::commands::projects::{
    agent_skill_configs, ensure_safe_skill_relative_path, is_copy_project,
};
use crate::core::error::AppError;
use crate::core::project_scanner::AgentSkillConfig;
use crate::core::project_setup::SetupItem;
use crate::core::{content_hash, project_deploy, sync_engine};

/// What sits at a deployment slot, compared with what belongs there.
#[derive(PartialEq)]
enum Slot {
    Absent,
    Same,
    Different,
}

/// A link is the same only when it leads to `expected`; a real folder, when
/// its content equals it. An unedited copy behind the Library reads as
/// different: the accepted v1 limit.
fn slot(path: &Path, expected: &Path) -> Slot {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Slot::Absent;
    };
    let same = if meta.file_type().is_symlink() {
        path.canonicalize().ok().is_some_and(|target| {
            expected
                .canonicalize()
                .is_ok_and(|expected| expected == target)
        })
    } else {
        meta.is_dir()
            && content_hash::hash_directory(path).ok()
                == content_hash::hash_directory(expected).ok()
    };
    if same {
        Slot::Same
    } else {
        Slot::Different
    }
}

type SkillGroup = (AgentSkillConfig, Vec<String>);

/// The agent folders a Skill deploys to, and every agent that reads one of
/// them: each sees the skill, so each must be approved. A copy-mode project
/// also writes `.agents/skills`.
fn skill_targets<'r>(run: &'r Run, item: &SetupItem) -> (Vec<&'r SkillGroup>, Vec<String>) {
    let groups: Vec<&SkillGroup> = run
        .skill_groups
        .iter()
        .filter(|(config, members)| {
            run.available_skill_groups.contains(&config.key)
                && members.iter().any(|m| item.agents.contains(m))
        })
        .collect();
    if groups.is_empty() {
        return (groups, Vec::new());
    }
    let copy_mode = is_copy_project(&run.project);
    let readers = run
        .skill_groups
        .iter()
        .filter(|(config, _)| {
            groups.iter().any(|(group, _)| group.key == config.key)
                || (copy_mode && project_deploy::is_vendored_dir(&config.relative_skills_dir))
        })
        .flat_map(|(_, members)| members.iter().cloned())
        .collect();
    (groups, readers)
}

/// Every agent that would read the folders a Skill deploys to.
pub(super) fn skill_readers(run: &Run, item: &SetupItem) -> Vec<String> {
    skill_targets(run, item).1
}

pub(super) fn apply_skill(run: &Run, item: &SetupItem) -> Result<Step, AppError> {
    let (ctx, project) = (run.ctx, &run.project);
    let Some(skill) = run.skill(&item.id)? else {
        return Ok(Step::asks(Status::Missing, Reason::ConfirmedAbsent));
    };
    let source = PathBuf::from(&skill.central_path);
    if !source.is_dir() {
        return Ok(Step::because(Status::Missing, "unavailable"));
    }
    let dir_name = sync_engine::target_dir_name(&source, &skill.name);
    ensure_safe_skill_relative_path(&dir_name)?;
    let root = Path::new(&project.path);
    let copy_mode = is_copy_project(project);

    let (groups, readers) = skill_targets(run, item);
    if groups.is_empty() {
        return Ok(Step::because(Status::Blocked, "no_supported_agent"));
    }
    if readers.iter().any(|reader| !item.agents.contains(reader)) {
        return Ok(Step::asks(Status::NeedsReview, Reason::UnapprovedAgents));
    }

    // Each slot and what must be there: the library skill, or in copy mode
    // the vendored copy and links to it.
    let mut slots: Vec<(PathBuf, PathBuf)> = Vec::new();
    let vendored = project_deploy::vendored_path(root, &dir_name, true);
    if copy_mode {
        if !vendored.1 {
            return Ok(Step::because(Status::NeedsReview, "disabled_here"));
        }
        slots.push((vendored.0.clone(), source.clone()));
    }
    for (config, _) in groups.iter().copied() {
        let folder = root.join(&config.relative_skills_dir);
        if root
            .join(format!("{}-disabled", config.relative_skills_dir))
            .join(&dir_name)
            .symlink_metadata()
            .is_ok()
        {
            return Ok(Step::because(Status::NeedsReview, "disabled_here"));
        }
        if copy_mode && project_deploy::is_vendored_dir(&config.relative_skills_dir) {
            continue;
        }
        let expected = if copy_mode { &vendored.0 } else { &source };
        let path = folder.join(&dir_name);
        if !slots.iter().any(|(existing, _)| *existing == path) {
            slots.push((path, expected.clone()));
        }
    }
    let states: Vec<Slot> = slots
        .iter()
        .map(|(path, expected)| slot(path, expected))
        .collect();
    if states.contains(&Slot::Different) {
        return Ok(Step::because(Status::NeedsReview, "edited_here"));
    }
    if states.iter().all(|state| *state == Slot::Same) {
        return Ok(Step::of(Status::Unchanged));
    }
    if !run.write {
        return Ok(Step::of(Status::NotApplied));
    }

    if copy_mode {
        let keys: Vec<String> = groups
            .iter()
            .map(|(config, _)| config.key.clone())
            .collect();
        let configs = agent_skill_configs(&ctx.store);
        let failed = project_deploy::deploy_copy_mode(root, &configs, &source, &dir_name, &keys)
            .map_err(AppError::io)?;
        if let Some(failure) = failed.first() {
            return Ok(Step::failed(failure.error.clone(), true));
        }
    } else {
        let configured = ctx.store.get_setting("sync_mode").map_err(AppError::db)?;
        for ((path, _), state) in slots.iter().zip(&states) {
            if *state == Slot::Absent {
                let folder = path.parent().unwrap_or(root);
                let mode = sync_engine::sync_mode_for_tool(&groups[0].0.key, configured.as_deref());
                project_deploy::deploy_skill(&source, folder, &dir_name, mode)
                    .map_err(AppError::io)?;
            }
        }
    }
    let verified = slots
        .iter()
        .all(|(path, expected)| slot(path, expected) == Slot::Same);
    Ok(if verified {
        Step::of(Status::Changed)
    } else {
        Step::failed("verification_failed", false)
    })
}
