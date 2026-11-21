use serde_json::{json, Value};

use super::{missing, worst, Run, Status, Step};
use crate::core::error::AppError;
use crate::core::instructions;
use crate::core::project_setup::{ItemKind, SetupItem};

fn instruction_agents<'i>(
    run: &'i Run<'_>,
    item: &'i SetupItem,
) -> impl Iterator<Item = &'i String> + 'i {
    item.agents
        .iter()
        .filter(|agent| run.supports(ItemKind::Instruction, agent))
}

fn instruction_preview(
    run: &Run,
    item: &SetupItem,
    agent: &str,
    dry_run: bool,
) -> Result<Value, AppError> {
    instructions::dispatch(
        run.ctx,
        json!({ "action": "preview", "instruction_id": item.id, "dry_run": dry_run,
                "target": { "agent_key": agent, "project_id": run.project.id } }),
    )
}

fn changes<'p>(preview: &'p Value, status: &'p str) -> impl Iterator<Item = &'p Value> {
    preview["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |change| change["status"] == status)
}

/// The files applying an Instruction bundle would delete, as `agent:path`.
pub(in crate::core::project_setup) fn instruction_deletes(
    run: &Run,
    item: &SetupItem,
) -> Result<Vec<String>, AppError> {
    let mut files = Vec::new();
    for agent in instruction_agents(run, item) {
        let preview = instruction_preview(run, item, agent, true)?;
        files.extend(
            changes(&preview, "delete")
                .map(|c| format!("{agent}:{}", c["path"].as_str().unwrap_or_default())),
        );
    }
    Ok(files)
}

pub(super) fn apply_instruction(
    run: &Run,
    item: &SetupItem,
    allow_delete: bool,
) -> Result<Step, AppError> {
    if let Err(error) = instructions::dispatch(run.ctx, json!({ "action": "get", "id": item.id })) {
        return Ok(missing(error));
    }
    let agents: Vec<&String> = instruction_agents(run, item).collect();
    if agents.is_empty() {
        return Ok(Step::because(Status::Blocked, "no_supported_agent"));
    }
    if run.instruction_recovery_pending {
        return Ok(Step::because(Status::Blocked, "pending_recovery"));
    }
    agents
        .into_iter()
        .map(|agent| {
            let preview = |dry_run: bool| instruction_preview(run, item, agent, dry_run);
            let has = |preview: &Value, status: &str| changes(preview, status).next().is_some();
            let unchanged = |preview: &Value| {
                preview["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .all(|change| change["status"] == "unchanged")
            };
            let planned = preview(true)?;
            if has(&planned, "conflict") {
                return Ok(Step::because(Status::NeedsReview, "edited_here"));
            }
            if has(&planned, "delete") && !allow_delete {
                return Ok(Step::because(Status::NeedsReview, "would_delete"));
            }
            if unchanged(&planned) {
                return Ok(Step::of(Status::Unchanged));
            }
            if !run.write {
                return Ok(Step::of(Status::NotApplied));
            }
            let stored = preview(false)?;
            let applied = instructions::dispatch(
                run.ctx,
                json!({ "action": "apply", "preview_id": stored["preview_id"] }),
            )?;
            if applied["status"] != "complete" {
                return Ok(Step::failed("apply_incomplete", false));
            }
            Ok(if unchanged(&preview(true)?) {
                Step::of(Status::Changed)
            } else {
                Step::failed("verification_failed", false)
            })
        })
        .collect::<Result<Vec<_>, AppError>>()
        .map(worst)
}
