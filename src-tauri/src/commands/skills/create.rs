//! Writing a new skill straight into the library, with no source to update from.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::install::log_install_outcome;
use crate::commands::{presets, projects};
use crate::core::{
    audit_log::AuditDraft,
    central_repo,
    error::AppError,
    host::HostCtx,
    installer,
    repo_lock::RepoLock,
    scenario_service::{self, BatchApplyMode},
    skill_install::{self, InstallSourceMetadata},
    skill_metadata,
    skill_store::{ProjectRecord, ScenarioRecord, SkillStore},
    tool_service,
};

const MAX_NAME_CHARS: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillContent {
    Written {
        description: String,
        instructions: String,
    },
    Folder {
        path: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    pub content: SkillContent,
    #[serde(default)]
    pub agents: Vec<String>,
    /// Preset id or exact name.
    pub preset: Option<String>,
    /// Project id, path or exact name.
    pub project: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct CreateSkillTarget {
    pub kind: &'static str,
    pub name: String,
    /// Set when the skill was created but could not be applied here.
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSkillReport {
    /// Absent on a dry run.
    pub skill_id: Option<String>,
    pub name: String,
    pub path: String,
    pub source_type: &'static str,
    pub dry_run: bool,
    pub targets: Vec<CreateSkillTarget>,
    pub partial: bool,
}

#[tauri::command]
pub async fn create_skill(
    request: CreateSkillRequest,
    ctx: State<'_, HostCtx>,
) -> Result<CreateSkillReport, AppError> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || create_skill_core(&ctx, request)).await?
}

pub fn create_skill_core(
    ctx: &HostCtx,
    request: CreateSkillRequest,
) -> Result<CreateSkillReport, AppError> {
    let store = &ctx.store;
    validate_name(&request.name)?;
    validate_content(&request.name, &request.content)?;
    let agents = resolve_agents(store, &request.agents)?;
    let preset = request
        .preset
        .as_deref()
        .map(|reference| resolve_preset(store, reference))
        .transpose()?;
    let project = request
        .project
        .as_deref()
        .map(|reference| resolve_project(store, reference))
        .transpose()?;
    let destination = central_repo::skills_dir().join(&request.name);

    let skill_id = {
        let _lock = RepoLock::acquire_foreground("create skill").map_err(AppError::db)?;
        ensure_name_is_free(store, &request.name, &destination)?;
        scenario_service::preflight_add_new_skill_to_tools(
            store,
            &request.name,
            &destination,
            &agents,
        )?;
        if request.dry_run {
            None
        } else {
            Some(write_skill(store, &request, &destination)?)
        }
    };

    let targets = match &skill_id {
        Some(id) => apply_targets(ctx, id, &request.name, &agents, &preset, &project),
        None => planned_targets(&agents, &preset, &project),
    };
    let partial = targets.iter().any(|target| target.error.is_some());
    Ok(CreateSkillReport {
        skill_id,
        name: request.name,
        path: destination.to_string_lossy().to_string(),
        source_type: "local",
        dry_run: request.dry_run,
        targets,
        partial,
    })
}

/// Refused rather than rewritten: the caller named this skill, and agents find
/// it by that name.
fn validate_name(name: &str) -> Result<(), AppError> {
    let valid = name.chars().count() <= MAX_NAME_CHARS
        && !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::invalid_input(format!(
            "Invalid skill name '{name}': use up to {MAX_NAME_CHARS} lowercase letters, digits \
             and single hyphens, e.g. release-notes"
        )))
    }
}

fn validate_description(description: Option<&str>) -> Result<(), AppError> {
    let description = description.map(str::trim).unwrap_or_default();
    if description.is_empty() {
        return Err(AppError::invalid_input(
            "A skill needs a description, so agents know when to use it",
        ));
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(AppError::invalid_input(format!(
            "A skill description can be at most {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(())
}

fn validate_content(name: &str, content: &SkillContent) -> Result<(), AppError> {
    match content {
        SkillContent::Written {
            description,
            instructions,
        } => {
            validate_description(Some(description))?;
            if instructions.trim().is_empty() {
                return Err(AppError::invalid_input("A skill needs instructions"));
            }
        }
        SkillContent::Folder { path } => {
            if !skill_metadata::is_valid_skill_dir(path) {
                return Err(AppError::invalid_input(format!(
                    "{} is not a folder containing SKILL.md",
                    path.display()
                )));
            }
            let meta = skill_metadata::parse_skill_md(path);
            if meta.name.as_deref() != Some(name) {
                return Err(AppError::invalid_input(format!(
                    "The name in {}/SKILL.md must be '{name}'",
                    path.display()
                )));
            }
            validate_description(meta.description.as_deref())?;
        }
    }
    Ok(())
}

fn resolve_agents(store: &SkillStore, requested: &[String]) -> Result<Vec<String>, AppError> {
    let infos = tool_service::list_tool_info(store);
    let mut agents: Vec<String> = Vec::new();
    for key in requested {
        let agent = infos
            .iter()
            .find(|info| info.key == *key)
            .ok_or_else(|| AppError::invalid_input(format!("Unknown agent: {key}")))?;
        if !agent.installed {
            return Err(AppError::invalid_input(format!(
                "Agent is not installed: {}",
                agent.display_name
            )));
        }
        if !agent.enabled {
            return Err(AppError::invalid_input(format!(
                "Agent is disabled: {}",
                agent.display_name
            )));
        }
        if !agents.contains(key) {
            agents.push(key.clone());
        }
    }
    Ok(agents)
}

fn resolve_preset(store: &SkillStore, reference: &str) -> Result<ScenarioRecord, AppError> {
    let mut matches: Vec<ScenarioRecord> = store
        .get_all_scenarios()
        .map_err(AppError::db)?
        .into_iter()
        .filter(|preset| preset.id == reference || preset.name == reference)
        .collect();
    match matches.len() {
        0 => Err(AppError::not_found(format!(
            "Preset not found: {reference}"
        ))),
        1 => Ok(matches.remove(0)),
        _ => Err(AppError::invalid_input(format!(
            "Preset name is ambiguous: {reference}"
        ))),
    }
}

fn resolve_project(store: &SkillStore, reference: &str) -> Result<ProjectRecord, AppError> {
    let projects = store.get_all_projects().map_err(AppError::db)?;
    let by_id_or_path = projects
        .iter()
        .find(|project| project.id == reference)
        .or_else(|| {
            projects
                .iter()
                .find(|project| same_path(&project.path, reference))
        });
    if let Some(project) = by_id_or_path {
        return Ok(project.clone());
    }
    let mut by_name: Vec<ProjectRecord> = projects
        .into_iter()
        .filter(|project| project.name == reference)
        .collect();
    match by_name.len() {
        0 => Err(AppError::not_found(format!(
            "No registered project matches '{reference}'. Add it as a project workspace in the \
             app first."
        ))),
        1 => Ok(by_name.remove(0)),
        _ => Err(AppError::invalid_input(format!(
            "Project name is ambiguous: {reference}. Use its path or id."
        ))),
    }
}

fn same_path(left: &str, right: &str) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

/// Deliberately not `installer::unique_skill_dest`, which reuses a matching
/// folder or picks `name-2`. A new skill must not replace or shadow another.
fn ensure_name_is_free(store: &SkillStore, name: &str, destination: &Path) -> Result<(), AppError> {
    let in_library = store
        .get_all_skills()
        .map_err(AppError::db)?
        .iter()
        .any(|skill| skill.name.eq_ignore_ascii_case(name));
    if in_library || destination.exists() {
        return Err(AppError::skill_exists(format!(
            "A skill named '{name}' is already in the library"
        )));
    }
    Ok(())
}

#[derive(Serialize)]
struct Frontmatter<'a> {
    name: &'a str,
    description: &'a str,
}

fn render_skill_md(name: &str, description: &str, instructions: &str) -> Result<String, AppError> {
    let frontmatter = serde_yaml::to_string(&Frontmatter {
        name,
        description: description.trim(),
    })
    .map_err(AppError::internal)?;
    Ok(format!(
        "---\n{frontmatter}---\n\n{}\n",
        instructions.trim()
    ))
}

/// Only ever called once `destination` is known to be absent, so removing it
/// after a failure cannot touch anything that existed before.
fn write_skill(
    store: &SkillStore,
    request: &CreateSkillRequest,
    destination: &Path,
) -> Result<String, AppError> {
    let name = &request.name;
    let outcome = (|| {
        let installed = match &request.content {
            SkillContent::Written {
                description,
                instructions,
            } => {
                let stage = tempfile::tempdir().map_err(AppError::io)?;
                std::fs::write(
                    stage.path().join("SKILL.md"),
                    render_skill_md(name, description, instructions)?,
                )
                .map_err(AppError::io)?;
                installer::install_skill_dir_to_destination(stage.path(), name, destination)
            }
            SkillContent::Folder { path } => {
                installer::install_skill_dir_to_destination(path, name, destination)
            }
        }
        .map_err(AppError::io)?;
        let metadata = InstallSourceMetadata {
            source_type: "local".to_string(),
            source_ref: None,
            source_ref_resolved: None,
            source_subpath: None,
            source_branch: None,
            source_revision: None,
            remote_revision: None,
            update_status: "local_only".to_string(),
        };
        let id = skill_install::store_installed_skill_unlocked(store, &installed, &metadata, None)?;
        Ok((id, name.clone()))
    })();
    log_install_outcome(store, "create", outcome.as_ref());
    match outcome {
        Ok((id, _)) => Ok(id),
        Err(error) => {
            if let Err(cleanup) = std::fs::remove_dir_all(destination) {
                if cleanup.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("Failed to clean up {}: {cleanup}", destination.display());
                }
            }
            Err(error)
        }
    }
}

fn planned_targets(
    agents: &[String],
    preset: &Option<ScenarioRecord>,
    project: &Option<ProjectRecord>,
) -> Vec<CreateSkillTarget> {
    let target = |kind, name: &str| CreateSkillTarget {
        kind,
        name: name.to_string(),
        error: None,
    };
    let mut targets: Vec<_> = agents.iter().map(|key| target("agent", key)).collect();
    targets.extend(preset.iter().map(|preset| target("preset", &preset.name)));
    targets.extend(
        project
            .iter()
            .map(|project| target("project", &project.name)),
    );
    targets
}

/// The skill is already safe in the library, so one target failing doesn't stop
/// the others. Each failure is reported for the caller to act on.
fn apply_targets(
    ctx: &HostCtx,
    skill_id: &str,
    skill_name: &str,
    agents: &[String],
    preset: &Option<ScenarioRecord>,
    project: &Option<ProjectRecord>,
) -> Vec<CreateSkillTarget> {
    let mut targets = apply_agents(&ctx.store, skill_id, skill_name, agents);
    if let Some(preset) = preset {
        let added = presets::add_skill_to_preset_core(ctx, skill_id.to_string(), preset.id.clone());
        targets.push(CreateSkillTarget {
            kind: "preset",
            name: preset.name.clone(),
            error: added.err().map(|error| error.message),
        });
    }
    if let Some(project) = project {
        let added = projects::export_skill_to_project_core(
            ctx,
            skill_id.to_string(),
            project.id.clone(),
            None,
        );
        targets.push(CreateSkillTarget {
            kind: "project",
            name: project.name.clone(),
            error: added.err().map(|error| error.message),
        });
    }
    targets
}

/// Batch deployment logs a pair it couldn't write rather than failing the
/// batch, so the recorded targets are what says whether each agent got it.
fn apply_agents(
    store: &SkillStore,
    skill_id: &str,
    skill_name: &str,
    agents: &[String],
) -> Vec<CreateSkillTarget> {
    let ids = [skill_id.to_string()];
    let applied = scenario_service::apply_skills_to_tools(store, &ids, agents, BatchApplyMode::Add);
    let recorded = store.get_targets_for_skill(skill_id);
    agents
        .iter()
        .map(|agent| {
            let error = match (&applied, &recorded) {
                (Err(error), _) => Some(error.message.clone()),
                (_, Err(error)) => Some(error.to_string()),
                (Ok(()), Ok(rows)) => (!rows
                    .iter()
                    .any(|row| row.tool == *agent && row.status == "ok"))
                .then(|| format!("Could not add the skill to {agent}")),
            };
            if error.is_none() {
                store.log_audit(
                    AuditDraft::new("deploy")
                        .skill(skill_id.to_string(), skill_name.to_string())
                        .tool(agent.clone())
                        .ok(),
                );
            }
            CreateSkillTarget {
                kind: "agent",
                name: agent.clone(),
                error,
            }
        })
        .collect()
}
