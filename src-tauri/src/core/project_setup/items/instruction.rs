use serde_json::{json, Value};

use std::path::{Path, PathBuf};

use super::{missing, resolved, AgentStep, Reason, Run, Status, Step};
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

/// Each native path that leads to a file the bundle writes, with the agents
/// that read it: the bundle's own paths, and any link at the checkout root
/// that resolves to one of them, such as `AGENTS.md` linked to `CLAUDE.md`.
fn entry_paths(run: &Run, item: &SetupItem) -> Result<Vec<(String, Vec<String>)>, AppError> {
    let Some(bundle) = run.instruction(&item.id)? else {
        return Ok(Vec::new());
    };
    let root = Path::new(&run.project.path);
    let written: Vec<&String> = bundle["files"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(path, _)| path)
        .collect();
    let targets: Vec<PathBuf> = written.iter().map(|p| resolved(&root.join(p))).collect();
    let links = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_symlink()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !written.contains(&name))
        .filter(|name| targets.contains(&resolved(&root.join(name))));
    Ok(written
        .iter()
        .map(|path| path.to_string())
        .chain(links)
        .map(|path| {
            let readers = instructions::project_file_readers(&path)
                .map(str::to_owned)
                .collect();
            (path, readers)
        })
        .collect())
}

/// Every agent that reads a file the bundle writes. A file such as root
/// `AGENTS.md` is read by several agents, so writing it for one sets it up
/// for all of them.
pub(super) fn instruction_readers(run: &Run, item: &SetupItem) -> Result<Vec<String>, AppError> {
    let mut readers: Vec<String> = Vec::new();
    for (_, agents) in entry_paths(run, item)? {
        for reader in agents {
            if !readers.contains(&reader) {
                readers.push(reader);
            }
        }
    }
    Ok(readers)
}

/// The native paths to the bundle's files that an agent outside the item
/// also reads.
pub(super) fn instruction_shared_paths(
    run: &Run,
    item: &SetupItem,
) -> Result<Vec<String>, AppError> {
    Ok(entry_paths(run, item)?
        .into_iter()
        .filter(|(_, readers)| readers.iter().any(|r| !item.agents.contains(r)))
        .map(|(path, _)| path)
        .collect())
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
        return whole(Step::because(Status::Blocked, Reason::NoSupportedAgent));
    }
    if instruction_readers(run, item)?
        .iter()
        .any(|reader| !item.agents.contains(reader))
    {
        return whole(Step::asks(Status::NeedsReview, Reason::UnapprovedAgents));
    }
    if instructions::recovery_pending(run.ctx)? {
        return whole(Step::because(Status::Blocked, Reason::PendingRecovery));
    }
    // One agent's failure never hides what was written for the others.
    Ok(agents
        .into_iter()
        .map(|agent| {
            // Asked again before each write: an earlier one may have left
            // a recovery in this run.
            let step = match instructions::recovery_pending(run.ctx) {
                Ok(true) => Step::because(Status::Blocked, Reason::PendingRecovery),
                Ok(false) => {
                    apply_for(run, item, agent, deletes_approved).unwrap_or_else(Step::error)
                }
                Err(error) => Step::error(error),
            };
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
    if let Some(step) = review(&preview(true)?, agent, deletes_approved) {
        return Ok(step);
    }
    if !run.write {
        return Ok(Step::of(Status::NotApplied));
    }
    // The preview submitted is checked again: the destination may have
    // changed since the dry run.
    let stored = preview(false)?;
    if let Some(step) = review(&stored, agent, deletes_approved) {
        return Ok(step);
    }
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

/// Why `preview` must not be applied for `agent`, if it must not: an edit
/// there, a deletion not exactly approved, or nothing to do.
fn review(preview: &Value, agent: &str, deletes_approved: &[String]) -> Option<Step> {
    if changes(preview, "conflict").next().is_some() {
        return Some(Step::because(Status::NeedsReview, Reason::EditedHere));
    }
    // The Library is unchanged, but the file there was edited or removed.
    if changes(preview, "unchanged").any(|change| !in_place(change)) {
        return Some(Step::because(Status::NeedsReview, Reason::EditedHere));
    }
    let deletes: Vec<String> = changes(preview, "delete")
        .map(|c| format!("{agent}:{}", c["path"].as_str().unwrap_or_default()))
        .collect();
    let approved: Vec<&String> = deletes_approved
        .iter()
        .filter(|file| file.starts_with(&format!("{agent}:")))
        .collect();
    if !deletes.is_empty() && deletes.iter().collect::<Vec<_>>() != approved {
        return Some(Step::asks(Status::NeedsReview, Reason::WouldDelete));
    }
    all_in_place(preview).then(|| Step::of(Status::Unchanged))
}

/// Whether a change's file is on disk as the Library has it, as the last
/// apply wrote it (a merge), or absent when the Library dropped it.
fn in_place(change: &Value) -> bool {
    change["status"] == "unchanged"
        && (change["previous"] == change["incoming"]
            || (change["previous"].is_string() && change["previous"] == change["deployed"]))
}

fn all_in_place(preview: &Value) -> bool {
    preview["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .all(in_place)
}
