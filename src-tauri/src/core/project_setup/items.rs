//! Each item is checked against its destination first. Only an absent or
//! cleanly replaceable destination is written, then checked again; anything
//! edited, unapproved or ambiguous is reported and left untouched, while
//! independent items carry on.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{ItemKind, SavedSetup, SetupItem};
use crate::commands::projects::{agent_skill_groups, available_agent_keys};
use crate::core::error::{AppError, ErrorKind};
use crate::core::host::HostCtx;
use crate::core::project_scanner::AgentSkillConfig;
use crate::core::skill_store::ProjectRecord;
use crate::core::{instructions, mcps};

pub(super) mod instruction;
pub(super) mod mcp;
pub(super) mod skill;

use instruction::apply_instruction;
use mcp::{apply_mcps, check_mcp};
use skill::apply_skill;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    // Declared from least to most in need of attention.
    Unchanged,
    Changed,
    /// Inspection only: absent or behind, so apply would write it.
    NotApplied,
    Missing,
    Blocked,
    NeedsReview,
    Failed,
}

/// One item's verified result. `reason` says why it was not applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ItemResult {
    pub(super) kind: ItemKind,
    pub(super) id: String,
    pub(super) agents: Vec<String>,
    pub(super) status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
    /// For a failure: whether applying again may succeed with nothing else
    /// done first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) retryable: Option<bool>,
}

#[derive(Clone)]
pub(super) struct Step {
    pub(super) status: Status,
    pub(super) reason: Option<String>,
    pub(super) retryable: bool,
}

impl Step {
    pub(super) fn of(status: Status) -> Self {
        Self {
            status,
            reason: None,
            retryable: false,
        }
    }
    pub(super) fn because(status: Status, reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Self::of(status)
        }
    }
    /// A failure; `retryable` when applying again may succeed as it is.
    pub(super) fn failed(reason: impl Into<String>, retryable: bool) -> Self {
        Self {
            retryable,
            ..Self::because(Status::Failed, reason)
        }
    }
    /// A failure from `error`: a database, file or network error may pass.
    pub(super) fn error(error: AppError) -> Self {
        let retryable = matches!(
            error.kind,
            ErrorKind::Database | ErrorKind::Io | ErrorKind::Network
        );
        Self::failed(error.message, retryable)
    }
}

/// An item one request runs, with its one-run hold (a skip or a restore to
/// wait for) and whether the files its Instruction bundle deletes are approved.
pub(super) struct Planned {
    pub(super) item: SetupItem,
    pub(super) held: Option<Step>,
    pub(super) deletes_approved: bool,
}

impl Planned {
    pub(super) fn new(item: SetupItem) -> Self {
        Self {
            item,
            held: None,
            deletes_approved: false,
        }
    }
}

/// Check each saved item against its destination; see [`check_items`].
pub(super) fn check_saved(run: &Run, setup: &SavedSetup) -> Vec<ItemResult> {
    let plan: Vec<_> = setup.items.iter().cloned().map(Planned::new).collect();
    check_items(run, &plan)
}

/// Check each planned item against its destination and, when `run` writes,
/// apply and verify it.
pub(super) fn check_items(run: &Run, plan: &[Planned]) -> Vec<ItemResult> {
    // Each item's steps, one per agent where it varies by agent. MCP
    // connections are queued and applied in batches per agent config.
    let mut steps: Vec<Vec<Step>> = Vec::with_capacity(plan.len());
    let mut queued: Vec<(String, usize)> = Vec::new();
    for (index, planned) in plan.iter().enumerate() {
        let item = &planned.item;
        steps.push(Vec::new());
        if let Some(held) = &planned.held {
            steps[index].push(held.clone());
            continue;
        }
        let step = match item.kind {
            _ if item.agents.is_empty() => Ok(Step::because(Status::Blocked, "no_agents")),
            ItemKind::Skill => apply_skill(run, item),
            ItemKind::Instruction => apply_instruction(run, item, planned.deletes_approved),
            ItemKind::Mcp => match check_mcp(run, item) {
                Ok(agents) => {
                    queued.extend(agents.into_iter().map(|agent| (agent, index)));
                    continue;
                }
                Err(step) => Ok(step),
            },
        };
        steps[index].push(step.unwrap_or_else(Step::error));
    }
    queued.sort();
    for batch in queued.chunk_by(|a, b| a.0 == b.0) {
        let agent = &batch[0].0;
        let batch: Vec<(usize, &SetupItem)> = batch
            .iter()
            .map(|(_, index)| (*index, &plan[*index].item))
            .collect();
        for ((index, _), step) in batch.iter().zip(apply_mcps(run, agent, &batch)) {
            steps[*index].push(step);
        }
    }
    plan.iter()
        .zip(steps)
        .map(|(Planned { item, .. }, steps)| {
            let step = worst(steps);
            ItemResult {
                kind: item.kind,
                id: item.id.clone(),
                agents: item.agents.clone(),
                status: step.status,
                reason: step.reason,
                retryable: (step.status == Status::Failed).then_some(step.retryable),
            }
        })
        .collect()
}

/// How many `items` have each of `statuses`, by status name.
pub(super) fn counts(items: &[ItemResult], statuses: &[Status]) -> Value {
    statuses
        .iter()
        .map(|status| {
            let count = items.iter().filter(|item| item.status == *status).count();
            (
                json!(status).as_str().unwrap_or_default().to_owned(),
                json!(count),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// Every agent in `lists`, sorted, each once.
pub(super) fn sorted_agents<'a>(lists: impl Iterator<Item = &'a Vec<String>>) -> Vec<&'a str> {
    let mut agents: Vec<&str> = lists.flatten().map(String::as_str).collect();
    agents.sort_unstable();
    agents.dedup();
    agents
}

/// Lookups made once per request rather than once per item.
pub(super) struct Run<'a> {
    pub(super) ctx: &'a HostCtx,
    pub(super) project: ProjectRecord,
    /// False for inspection: check each destination and write nothing.
    pub(super) write: bool,
    pub(super) skill_groups: Vec<(AgentSkillConfig, Vec<String>)>,
    pub(super) available_skill_groups: HashSet<String>,
    pub(super) instruction_recovery_pending: bool,
    pub(super) mcp_definitions: Vec<Value>,
    /// Agents with project-scoped MCP configuration.
    pub(super) mcp_agents: Vec<String>,
}

impl<'a> Run<'a> {
    pub(super) fn new(
        ctx: &'a HostCtx,
        project: ProjectRecord,
        write: bool,
    ) -> Result<Self, AppError> {
        let capabilities = mcps::dispatch(ctx, json!({ "action": "capabilities" }))?;
        let mcp_agents = capabilities["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|target| {
                target["scopes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|scope| scope["kind"] == "project")
            })
            .filter_map(|target| target["agentKey"].as_str().map(str::to_owned))
            .collect();
        let definitions = mcps::dispatch(ctx, json!({ "action": "list" }))?;
        Ok(Self {
            skill_groups: agent_skill_groups(&ctx.store),
            available_skill_groups: available_agent_keys(&ctx.store, &project),
            instruction_recovery_pending: instructions::recovery_pending(ctx)?,
            mcp_definitions: definitions["definitions"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            mcp_agents,
            ctx,
            project,
            write,
        })
    }

    /// The Library's MCP connection with this ID.
    pub(super) fn mcp_definition(&self, id: &str) -> Option<&Value> {
        self.mcp_definitions.iter().find(|d| d["id"] == id)
    }
}

/// Not found means the Library confirms the item is gone; any other failure
/// to read it leaves the question open.
fn missing(error: AppError) -> Step {
    let reason = if error.kind == ErrorKind::NotFound {
        "confirmed_absent"
    } else {
        "unavailable"
    };
    Step::because(Status::Missing, reason)
}

/// The worst of several agents' steps for one item.
fn worst(steps: impl IntoIterator<Item = Step>) -> Step {
    steps
        .into_iter()
        .max_by_key(|step| step.status)
        .unwrap_or_else(|| Step::because(Status::Blocked, "no_supported_agent"))
}
