//! Host-scoped project Git request.

use serde_json::Value;
use tauri::State;

use crate::core::{
    error::AppError,
    host::HostCtx,
    project_git::{Repo, Request},
    skill_store::ProjectRecord,
};

use super::agents_model::agent_skill_configs;

#[tauri::command]
pub async fn project_git_request(
    ctx: State<'_, HostCtx>,
    project_id: String,
    request: Request,
) -> Result<Value, AppError> {
    let ctx = ctx.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        project_git_request_core(&ctx, &project_id, request)
    })
    .await?
}

pub fn project_git_request_core(
    ctx: &HostCtx,
    project_id: &str,
    request: Request,
) -> Result<Value, AppError> {
    let record: ProjectRecord = ctx
        .store
        .get_project_by_id(project_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Project not found"))?;
    if record.workspace_type == "linked" {
        return Err(AppError::invalid_input(
            "Git actions are available for standard project workspaces only.",
        ));
    }
    let project_root = std::path::PathBuf::from(&record.path);
    let skills_roots = agent_skill_configs(&ctx.store)
        .into_iter()
        .map(|config| project_root.join(config.relative_skills_dir))
        .collect();
    Repo::new(project_root, skills_roots)?.request(request)
}
