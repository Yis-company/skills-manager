use std::path::{Component, Path, PathBuf};

use super::{AgentStep, Reason, Run, Status, Step};
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
        meta.is_dir() && same_content(path, expected)
    };
    if same {
        Slot::Same
    } else {
        Slot::Different
    }
}

/// Whether two folders hold the same files, bytes and executable bits.
/// Anything that cannot be listed or read is unverified, never the same.
fn same_content(found: &Path, wanted: &Path) -> bool {
    let (Ok(found), Ok(wanted)) = (
        content_hash::list_content_files_strict(found),
        content_hash::list_content_files_strict(wanted),
    ) else {
        return false;
    };
    found.len() == wanted.len()
        && found.iter().zip(&wanted).all(|(a, b)| {
            a.relative_path == b.relative_path
                && a.exec_bits == b.exec_bits
                && matches!(
                    (std::fs::read(&a.path), std::fs::read(&b.path)),
                    (Ok(x), Ok(y)) if x == y
                )
        })
}

/// Whether `path`'s folder resolves inside the checkout at `root`. A folder
/// that is a link elsewhere, such as an agent folder shared between
/// checkouts, would take the write outside it.
pub(in crate::core::project_setup) fn inside(root: &Path, path: &Path) -> bool {
    if path.components().any(|c| c == Component::ParentDir) {
        return false;
    }
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    // The nearest folder that exists: those below it are created as folders.
    path.parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .find_map(|dir| dir.canonicalize().ok())
        .is_some_and(|dir| dir.starts_with(&root))
}

/// Where `path` leads: its nearest existing folder resolved through links,
/// with the parts below it that do not exist yet.
fn resolved(path: &Path) -> PathBuf {
    for dir in path.ancestors() {
        if let Ok(real) = dir.canonicalize() {
            return path
                .strip_prefix(dir)
                .map_or_else(|_| path.to_path_buf(), |rest| real.join(rest));
        }
    }
    path.to_path_buf()
}

type SkillGroup = (AgentSkillConfig, Vec<String>);

/// The agent folders a Skill is added to, and every agent folder group that
/// reads one of them: each of its agents sees the skill, so each must be
/// approved, also through a folder linked to it. A copy-mode project also
/// writes `.agents/skills`.
fn skill_targets<'r>(run: &'r Run, item: &SetupItem) -> (Vec<&'r SkillGroup>, Vec<&'r SkillGroup>) {
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
    let root = Path::new(&run.project.path);
    let written: Vec<PathBuf> = groups
        .iter()
        .map(|(config, _)| resolved(&root.join(&config.relative_skills_dir)))
        .collect();
    let reading = run
        .skill_groups
        .iter()
        .filter(|(config, _)| {
            groups.iter().any(|(group, _)| group.key == config.key)
                || written.contains(&resolved(&root.join(&config.relative_skills_dir)))
                || (copy_mode && project_deploy::is_vendored_dir(&config.relative_skills_dir))
        })
        .collect();
    (groups, reading)
}

/// Every agent that would read the folders a Skill is added to.
pub(super) fn skill_readers(run: &Run, item: &SetupItem) -> Vec<String> {
    let (_, reading) = skill_targets(run, item);
    reading
        .into_iter()
        .flat_map(|(_, members)| members.iter().cloned())
        .collect()
}

/// The folders a Skill is added to that an agent outside the item reads.
pub(super) fn skill_shared_paths(run: &Run, item: &SetupItem) -> Vec<String> {
    let (_, reading) = skill_targets(run, item);
    let mut paths: Vec<String> = reading
        .into_iter()
        .filter(|(_, members)| members.iter().any(|m| !item.agents.contains(m)))
        .map(|(config, _)| config.relative_skills_dir.clone())
        .collect();
    paths.dedup();
    paths
}

/// Set a Skill up for each of its agents. Every slot is written and
/// checked even when another fails, so each agent reports its own result.
pub(super) fn apply_skill(run: &Run, item: &SetupItem) -> Result<Vec<AgentStep>, AppError> {
    let whole = |step: Step| Ok(vec![(None, step)]);
    let (ctx, project) = (run.ctx, &run.project);
    let Some(skill) = run.skill(&item.id)? else {
        return whole(Step::asks(Status::Missing, Reason::ConfirmedAbsent));
    };
    let source = PathBuf::from(&skill.central_path);
    if !source.is_dir() {
        return whole(Step::because(Status::Missing, "unavailable"));
    }
    let dir_name = sync_engine::target_dir_name(&source, &skill.name);
    ensure_safe_skill_relative_path(&dir_name)?;
    let root = Path::new(&project.path);
    let copy_mode = is_copy_project(project);

    let (groups, _) = skill_targets(run, item);
    let readers = skill_readers(run, item);
    if groups.is_empty() {
        return whole(Step::because(Status::Blocked, Reason::NoSupportedAgent));
    }
    if readers.iter().any(|reader| !item.agents.contains(reader)) {
        return whole(Step::asks(Status::NeedsReview, Reason::UnapprovedAgents));
    }

    // Each slot, what must be there, and the agents that read it: the
    // library skill, or in copy mode the vendored copy, read by every
    // agent, and links to it.
    let mut slots: Vec<(PathBuf, PathBuf, Vec<String>)> = Vec::new();
    let vendored = project_deploy::vendored_path(root, &dir_name, true);
    if copy_mode {
        if !vendored.1 {
            return whole(Step::because(Status::NeedsReview, "disabled_here"));
        }
        slots.push((vendored.0.clone(), source.clone(), readers.clone()));
    }
    for (config, members) in groups.iter().copied() {
        let folder = root.join(&config.relative_skills_dir);
        if root
            .join(format!("{}-disabled", config.relative_skills_dir))
            .join(&dir_name)
            .symlink_metadata()
            .is_ok()
        {
            return whole(Step::because(Status::NeedsReview, "disabled_here"));
        }
        if copy_mode && project_deploy::is_vendored_dir(&config.relative_skills_dir) {
            continue;
        }
        let expected = if copy_mode { &vendored.0 } else { &source };
        let path = folder.join(&dir_name);
        match slots
            .iter_mut()
            .find(|(existing, ..)| resolved(existing) == resolved(&path))
        {
            Some((.., agents)) => agents.extend(members.iter().cloned()),
            None => slots.push((path, expected.clone(), members.clone())),
        }
    }
    if !slots.iter().all(|(path, ..)| inside(root, path)) {
        return whole(Step::because(Status::Blocked, "outside_checkout"));
    }
    let states: Vec<Slot> = slots
        .iter()
        .map(|(path, expected, _)| slot(path, expected))
        .collect();
    if states.contains(&Slot::Different) {
        return whole(Step::because(Status::NeedsReview, Reason::EditedHere));
    }
    if states.iter().all(|state| *state == Slot::Same) {
        return whole(Step::of(Status::Unchanged));
    }
    if !run.write {
        return whole(Step::of(Status::NotApplied));
    }

    // Each slot's write error, if any.
    let mut errors: Vec<Option<String>> = vec![None; slots.len()];
    if copy_mode {
        let keys: Vec<String> = groups
            .iter()
            .map(|(config, _)| config.key.clone())
            .collect();
        let configs = agent_skill_configs(&ctx.store);
        match project_deploy::deploy_copy_mode(root, &configs, &source, &dir_name, &keys) {
            Err(error) => errors.fill(Some(format!("{error:#}"))),
            Ok(failed) => {
                for failure in failed {
                    let Some((config, _)) = groups.iter().find(|(c, _)| c.key == failure.agent)
                    else {
                        continue;
                    };
                    let path = root.join(&config.relative_skills_dir).join(&dir_name);
                    if let Some(n) = slots.iter().position(|(p, ..)| *p == path) {
                        errors[n] = Some(failure.error);
                    }
                }
            }
        }
    } else {
        let configured = ctx.store.get_setting("sync_mode").map_err(AppError::db)?;
        let mode = sync_engine::sync_mode_for_tool(&groups[0].0.key, configured.as_deref());
        for (n, (path, ..)) in slots.iter().enumerate() {
            if states[n] == Slot::Absent {
                let folder = path.parent().unwrap_or(root);
                if let Err(error) = project_deploy::deploy_skill(&source, folder, &dir_name, mode) {
                    errors[n] = Some(format!("{error:#}"));
                }
            }
        }
    }

    // Each agent: changed when every slot it reads now holds the skill and
    // one was written, failed otherwise.
    let verified: Vec<bool> = slots
        .iter()
        .map(|(path, expected, _)| slot(path, expected) == Slot::Same)
        .collect();
    let agents = item
        .agents
        .iter()
        .filter(|agent| slots.iter().any(|(.., readers)| readers.contains(agent)));
    Ok(agents
        .map(|agent| {
            let read: Vec<usize> = (0..slots.len())
                .filter(|n| slots[*n].2.contains(agent))
                .collect();
            let step = match read.iter().find(|n| !verified[**n]) {
                Some(n) => match &errors[*n] {
                    Some(error) => Step::failed(error.clone(), true),
                    None => Step::failed("verification_failed", false),
                },
                None if read.iter().any(|n| states[*n] == Slot::Absent) => {
                    Step::of(Status::Changed)
                }
                None => Step::of(Status::Unchanged),
            };
            (Some(agent.clone()), step)
        })
        .collect())
}
