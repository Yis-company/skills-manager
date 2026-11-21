use serde_json::{json, Value};

use super::{missing, AgentStep, Reason, Run, Status, Step};
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

/// Every agent that reads a file the bundle writes. A file such as root
/// `AGENTS.md` is read by several agents, so writing it for one sets it up
/// for all of them.
pub(super) fn instruction_readers(run: &Run, item: &SetupItem) -> Result<Vec<String>, AppError> {
    let Some(bundle) = run.instruction(&item.id)? else {
        return Ok(Vec::new());
    };
    let mut readers = Vec::new();
    for file in bundle["files"].as_array().into_iter().flatten() {
        let path = file["path"].as_str().unwrap_or_default();
        for reader in instructions::project_file_readers(path) {
            if !readers.iter().any(|known| known == reader) {
                readers.push(reader.to_owned());
            }
        }
    }
    Ok(readers)
}

/// Apply an Instruction bundle for each of its agents. Files it would delete
/// are deleted only when they are exactly `deletes_approved`.
pub(super) fn apply_instruction(
    run: &Run,
    item: &SetupItem,
    deletes_approved: &[String],
) -> Result<Vec<AgentStep>, AppError> {
    let whole = |step: Step| Ok(vec![(None, step)]);
    match run.instruction(&item.id) {
        Ok(Some(_)) => {}
        Ok(None) => return whole(Step::asks(Status::Missing, Reason::ConfirmedAbsent)),
        Err(error) => return whole(missing(error)),
    }
    let agents: Vec<&String> = instruction_agents(run, item).collect();
    if agents.is_empty() {
        return whole(Step::because(Status::Blocked, "no_supported_agent"));
    }
    if instruction_readers(run, item)?
        .iter()
        .any(|reader| !item.agents.contains(reader))
    {
        return whole(Step::asks(Status::NeedsReview, Reason::UnapprovedAgents));
    }
    if run.instruction_recovery_pending {
        return whole(Step::because(Status::Blocked, "pending_recovery"));
    }
    // One agent's failure never hides what was written for the others.
    Ok(agents
        .into_iter()
        .map(|agent| {
            let step = apply_for(run, item, agent, deletes_approved).unwrap_or_else(Step::error);
            (Some(agent.clone()), step)
        })
        .collect())
}

fn apply_for(
    run: &Run,
    item: &SetupItem,
    agent: &str,
    deletes_approved: &[String],
) -> Result<Step, AppError> {
    let preview = |dry_run: bool| instruction_preview(run, item, agent, dry_run);
    let planned = preview(true)?;
    if changes(&planned, "conflict").next().is_some() {
        return Ok(Step::because(Status::NeedsReview, "edited_here"));
    }
    // The Library is unchanged, but the file there was edited or removed.
    if changes(&planned, "unchanged").any(|change| !in_place(change)) {
        return Ok(Step::because(Status::NeedsReview, "edited_here"));
    }
    let deletes: Vec<String> = changes(&planned, "delete")
        .map(|c| format!("{agent}:{}", c["path"].as_str().unwrap_or_default()))
        .collect();
    let approved: Vec<&String> = deletes_approved
        .iter()
        .filter(|file| file.starts_with(&format!("{agent}:")))
        .collect();
    if !deletes.is_empty() && deletes.iter().collect::<Vec<_>>() != approved {
        return Ok(Step::asks(Status::NeedsReview, Reason::WouldDelete));
    }
    if all_in_place(&planned) {
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
    Ok(if all_in_place(&preview(true)?) {
        Step::of(Status::Changed)
    } else {
        Step::failed("verification_failed", false)
    })
}

/// Whether a change's file is on disk as the Library has it, or absent when
/// the Library dropped it.
fn in_place(change: &Value) -> bool {
    change["status"] == "unchanged" && change["previous"] == change["incoming"]
}

fn all_in_place(preview: &Value) -> bool {
    preview["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .all(in_place)
}
