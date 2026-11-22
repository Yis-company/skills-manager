//! What needs a decision comes back as one grouped proposal: a whole first
//! setup, or the items that need approval. The caller confirms by sending the
//! proposal back with a choice on each decision. Each decision is rebuilt
//! from the current state and must match, or the confirmation is stale; only
//! then is the checkout registered, the setup saved and applied.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::apply::{respond, summarize, Outcome};
use super::items::instruction::instruction_deletes;
use super::items::mcp::{approval, effective_values};
use super::items::skill::inside;
use super::items::{shared_paths, sorted_agents, with_readers, Planned, Run, Status, Step};
use super::{save_setup, ItemKind, SavedSetup, SetupItem};
use crate::commands::projects::{add_project_core, registration_dirs};
use crate::core::error::AppError;

/// The decisions a request needs, grouped for one confirmation, and the
/// state they were made against. Confirming one that no longer matches the
/// current state is refused as stale.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Proposal {
    pub(super) checkout_path: PathBuf,
    pub(super) project_id: Option<String>,
    /// Whether confirming registers the checkout, with `deploy_mode`.
    pub(super) register: bool,
    pub(super) deploy_mode: String,
    /// The saved setup's version, 0 for none.
    pub(super) setup_version: u64,
    pub(super) decisions: Vec<Decision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Decision {
    kind: ItemKind,
    id: String,
    /// The agents the item targets now.
    agents: Vec<String>,
    reason: Reason,
    name: Option<String>,
    /// What approving saves: the item for a first setup, the MCP values by
    /// agent, the full agent list, or the files an Instruction bundle would
    /// delete.
    proposed: Value,
    /// With unapproved agents: the native paths shared with them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    paths: Vec<String>,
    choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    choice: Option<Choice>,
    /// The Library ID to use instead, with `replace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replacement: Option<String>,
}

/// Why a decision is asked: a first setup, or an item result's reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Reason {
    FirstSetup,
    ApprovalNeeded,
    ValueChanged,
    UnapprovedAgents,
    WouldDelete,
    ConfirmedAbsent,
}

impl Reason {
    /// The name an item result's `reason` carries.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::FirstSetup => "first_setup",
            Self::ApprovalNeeded => "approval_needed",
            Self::ValueChanged => "value_changed",
            Self::UnapprovedAgents => "unapproved_agents",
            Self::WouldDelete => "would_delete",
            Self::ConfirmedAbsent => "confirmed_absent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Choice {
    Approve,
    Skip,
    Restore,
    Replace,
    Revise,
}
pub(super) const CONFIRM: &str =
    "Show the proposal's decisions to the user. To go ahead, call again with \
    a new operation_id, the same agents if any, and confirm: the proposal, with a choice on \
    each decision.";

/// The items one request runs.
pub(super) struct Plan {
    pub(super) items: Vec<Planned>,
    /// Whether the selection must be saved first.
    pub(super) save: bool,
}

/// A first setup's proposal: nothing is written until it is confirmed.
pub(super) fn propose(scope: &Value, operation_id: &str, proposal: Proposal) -> Value {
    let mut result = summarize(scope, operation_id, &[]);
    result["agents"] = json!(sorted_agents(
        proposal.decisions.iter().map(|decision| &decision.agents)
    ));
    result["outcome"] = json!(Outcome::ConfirmationNeeded);
    result["next_action"] = json!(CONFIRM);
    result["proposal"] = json!(proposal);
    respond(result, false, false)
}

/// Register the checkout when the proposal says to, then save the confirmed
/// selection over the version it was proposed against. Returns the new version.
pub(super) fn save_confirmed(
    run: &mut Run,
    proposal: &Proposal,
    items: Vec<SetupItem>,
) -> Result<u64, AppError> {
    if proposal.register {
        // Registering creates agent folders: none may lead out of the checkout.
        let root = Path::new(&run.project.path);
        for dir in registration_dirs(root, &proposal.deploy_mode)? {
            if !inside(root, &dir) {
                return Err(AppError::invalid_input(format!(
                    "{} leads outside the checkout, so registering it would write there; \
                     nothing was registered or saved",
                    dir.display()
                )));
            }
        }
        let added = add_project_core(
            run.ctx,
            run.project.path.clone(),
            Some(proposal.deploy_mode.clone()),
        )?;
        run.project.id = added.id;
    }
    Ok(save_setup(
        &run.ctx.store,
        &run.project.id,
        proposal.setup_version,
        items,
    )?
    .version)
}

/// Check a confirmation against the current state and turn its choices into
/// a plan. Reads only; any difference from a fresh proposal is stale.
pub(super) fn confirmed(
    run: &Run,
    now: &Proposal,
    saved: Option<SavedSetup>,
    confirm: &Value,
) -> Result<Plan, AppError> {
    let confirm: Proposal = serde_json::from_value(confirm.clone())
        .map_err(|e| AppError::invalid_input(format!("Invalid confirm: {e}")))?;
    let stale = || {
        AppError::invalid_input(
            "This proposal is stale: the checkout, its saved setup or a proposed item changed \
             since it was proposed. Apply again without confirm for a fresh proposal.",
        )
    };
    if (
        &confirm.checkout_path,
        &confirm.project_id,
        confirm.register,
        &confirm.deploy_mode,
        confirm.setup_version,
    ) != (
        &now.checkout_path,
        &now.project_id,
        now.register,
        &now.deploy_mode,
        now.setup_version,
    ) {
        return Err(stale());
    }
    run.load_skills(confirm.decisions.iter().map(|d| (d.kind, d.id.as_str())))?;
    let first = saved.is_none();
    let original: Vec<SetupItem> = saved.map(|setup| setup.items).unwrap_or_default();
    let mut planned: Vec<Planned> = original.iter().cloned().map(Planned::new).collect();
    let mut keep = vec![true; planned.len()];
    for decision in confirm.decisions {
        let Some(choice) = decision.choice.filter(|c| decision.choices.contains(c)) else {
            return Err(AppError::invalid_input(format!(
                "Choose one of {} for {}",
                json!(decision.choices),
                decision.id
            )));
        };
        let asked = SetupItem {
            kind: decision.kind,
            id: decision.id.clone(),
            agents: decision.agents.clone(),
            approved: None,
        };
        let matching: Vec<usize> = (0..original.len())
            .filter(|n| {
                let item = &original[*n];
                (item.kind, &item.id, &item.agents) == (asked.kind, &asked.id, &asked.agents)
            })
            .collect();
        let current = match matching.first() {
            _ if first => asked,
            Some(n) => original[*n].clone(),
            None => return Err(stale()),
        };
        let echoed = Decision {
            choice: None,
            replacement: None,
            ..decision.clone()
        };
        if decide(run, &current, decision.reason)? != Some(echoed) {
            return Err(stale());
        }
        if first {
            if choice == Choice::Approve {
                let item = serde_json::from_value(decision.proposed)
                    .map_err(|e| AppError::invalid_input(format!("Invalid proposed item: {e}")))?;
                planned.push(Planned::new(item));
                keep.push(true);
            }
            continue;
        }
        for n in matching {
            let Planned {
                item,
                held,
                deletes_approved,
            } = &mut planned[n];
            match (decision.reason, choice) {
                (_, Choice::Skip) => *held = Some(Step::because(Status::Blocked, "skipped")),
                (_, Choice::Restore) => {
                    *held = Some(Step::because(Status::Missing, "awaiting_restore"))
                }
                (_, Choice::Revise) => keep[n] = false,
                (_, Choice::Replace) => {
                    let replacement = SetupItem {
                        id: decision.replacement.clone().ok_or_else(|| {
                            AppError::invalid_input("replace needs a replacement Library ID")
                        })?,
                        approved: None,
                        ..item.clone()
                    };
                    if library_item(run, &replacement)?.is_none() {
                        return Err(AppError::invalid_input(format!(
                            "{} is not in the Library",
                            replacement.id
                        )));
                    }
                    *item = replacement;
                }
                (Reason::WouldDelete, _) => {
                    *deletes_approved = serde_json::from_value(decision.proposed.clone())
                        .map_err(|e| AppError::invalid_input(format!("Invalid files: {e}")))?;
                }
                (Reason::UnapprovedAgents, _) => {
                    item.agents = serde_json::from_value(decision.proposed.clone())
                        .map_err(|e| AppError::invalid_input(format!("Invalid agents: {e}")))?;
                }
                _ => item.approved = Some(decision.proposed.clone()),
            }
        }
    }
    let items: Vec<_> = planned
        .into_iter()
        .zip(keep)
        .filter_map(|(planned, keep)| keep.then_some(planned))
        .collect();
    let save = first
        || items.len() != original.len()
        || items
            .iter()
            .zip(&original)
            .any(|(planned, was)| planned.item != *was);
    Ok(Plan { items, save })
}

/// The decision `reason` asks of `item` now, or `None` when it no longer
/// applies. Reads only, so a proposal and its confirmation can both use it.
pub(super) fn decide(
    run: &Run,
    item: &SetupItem,
    reason: Reason,
) -> Result<Option<Decision>, AppError> {
    let found = library_item(run, item)?;
    let approve = || vec![Choice::Approve, Choice::Skip];
    let mut paths = Vec::new();
    let (proposed, choices) = match (reason, item.kind, &found) {
        (Reason::ConfirmedAbsent, _, None) => (
            Value::Null,
            vec![Choice::Restore, Choice::Replace, Choice::Revise],
        ),
        (_, _, None) => return Ok(None),
        (Reason::FirstSetup, kind, Some(record)) => {
            let agents = with_readers(run, item)?;
            let approved = match kind {
                ItemKind::Mcp => {
                    let item = SetupItem {
                        agents: agents.clone(),
                        ..item.clone()
                    };
                    Some(effective_values(run, &item, record)?)
                }
                _ => None,
            };
            let item = SetupItem {
                agents,
                approved,
                ..item.clone()
            };
            (json!(item), approve())
        }
        (Reason::ApprovalNeeded | Reason::ValueChanged, ItemKind::Mcp, Some(record)) => {
            let effective = effective_values(run, item, record)?;
            if approval(item, &effective) != Some(reason) {
                return Ok(None);
            }
            (effective, approve())
        }
        (Reason::UnapprovedAgents, ItemKind::Skill | ItemKind::Instruction, Some(_)) => {
            let agents = with_readers(run, item)?;
            if agents == item.agents {
                return Ok(None);
            }
            paths = shared_paths(run, item)?;
            (json!(agents), approve())
        }
        (Reason::WouldDelete, ItemKind::Instruction, Some(_)) => {
            let files = instruction_deletes(run, item)?;
            if files.is_empty() {
                return Ok(None);
            }
            (json!(files), approve())
        }
        _ => return Ok(None),
    };
    Ok(Some(Decision {
        kind: item.kind,
        id: item.id.clone(),
        agents: item.agents.clone(),
        reason,
        name: found.and_then(|record| record["name"].as_str().map(str::to_owned)),
        proposed,
        paths,
        choices,
        choice: None,
        replacement: None,
    }))
}

/// The Library's record of `item`, or `None` when the Library confirms it
/// is gone.
fn library_item(run: &Run, item: &SetupItem) -> Result<Option<Value>, AppError> {
    Ok(match item.kind {
        ItemKind::Skill => run
            .skill(&item.id)?
            .map(|skill| json!({ "name": skill.name })),
        ItemKind::Instruction => run.instruction(&item.id)?.cloned(),
        ItemKind::Mcp => run.mcp_definition(&item.id)?.cloned(),
    })
}
