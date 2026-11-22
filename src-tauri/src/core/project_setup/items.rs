//! Each item is checked against its destination first. Only an absent or
//! cleanly replaceable destination is written, then checked again; anything
//! edited, unapproved or ambiguous is reported and left untouched, while
//! independent items carry on.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::confirm::Reason;
use super::{ItemKind, SetupItem};
use crate::commands::projects::{agent_skill_groups, available_agent_keys};
use crate::core::error::{AppError, ErrorKind};
use crate::core::host::HostCtx;
use crate::core::project_scanner::AgentSkillConfig;
use crate::core::skill_store::{ProjectRecord, SkillRecord};
use crate::core::{instructions, mcps};

pub(super) mod instruction;
pub(super) mod mcp;
pub(super) mod skill;

use instruction::{apply_instruction, instruction_readers, instruction_shared_paths};
use mcp::{apply_mcps, check_mcp};
use skill::{apply_skill, skill_readers, skill_shared_paths};

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
    /// The item's agents this kind of item cannot be set up for; `agents`
    /// holds the rest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) unsupported: Vec<String>,
    /// Each agent's own result, when the agents' results differ.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) targets: Vec<TargetResult>,
    /// The decision `reason` asks for, when a confirmation can resolve it.
    #[serde(skip)]
    pub(super) asks: Option<Reason>,
}

/// One agent's result for an item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TargetResult {
    pub(super) agent: String,
    pub(super) status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
}

#[derive(Clone)]
pub(super) struct Step {
    pub(super) status: Status,
    pub(super) reason: Option<String>,
    pub(super) retryable: bool,
    pub(super) asks: Option<Reason>,
}

impl Step {
    pub(super) fn of(status: Status) -> Self {
        Self {
            status,
            reason: None,
            retryable: false,
            asks: None,
        }
    }
    pub(super) fn because(status: Status, reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Self::of(status)
        }
    }
    /// A result a confirmation can resolve: `reason` names its decision.
    pub(super) fn asks(status: Status, reason: Reason) -> Self {
        Self {
            asks: Some(reason),
            ..Self::because(status, reason)
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
/// wait for) and the files its Instruction bundle may delete, as `agent:path`.
pub(super) struct Planned {
    pub(super) item: SetupItem,
    pub(super) held: Option<Step>,
    pub(super) deletes_approved: Vec<String>,
}

impl Planned {
    pub(super) fn new(item: SetupItem) -> Self {
        Self {
            item,
            held: None,
            deletes_approved: Vec::new(),
        }
    }
}

/// Check each item against its destination; see [`check_items`].
pub(super) fn check_setup(run: &Run, items: &[SetupItem]) -> Vec<ItemResult> {
    let plan: Vec<_> = items.iter().cloned().map(Planned::new).collect();
    check_items(run, &plan)
}

/// One step for a whole item, or one agent's step.
type AgentStep = (Option<String>, Step);

/// Check each planned item against its destination and, when `run` writes,
/// apply and verify it.
pub(super) fn check_items(run: &Run, plan: &[Planned]) -> Vec<ItemResult> {
    // On failure, each Skill's own lookup reports the error.
    let _ = run.load_skills(plan.iter().map(|p| (p.item.kind, p.item.id.as_str())));
    // Each item's steps, one per agent where it varies by agent. MCP
    // connections are queued and applied in batches per agent config.
    let mut steps: Vec<Vec<AgentStep>> = Vec::with_capacity(plan.len());
    let mut queued: Vec<(String, usize)> = Vec::new();
    for (index, planned) in plan.iter().enumerate() {
        let item = &planned.item;
        let whole = |step: Result<Step, AppError>| vec![(None, step.unwrap_or_else(Step::error))];
        steps.push(match (&planned.held, item.kind) {
            (Some(held), _) => vec![(None, held.clone())],
            _ if item.agents.is_empty() => whole(Ok(Step::because(Status::Blocked, "no_agents"))),
            (None, ItemKind::Skill) => {
                apply_skill(run, item).unwrap_or_else(|error| vec![(None, Step::error(error))])
            }
            (None, ItemKind::Instruction) => {
                apply_instruction(run, item, &planned.deletes_approved)
                    .unwrap_or_else(|error| vec![(None, Step::error(error))])
            }
            (None, ItemKind::Mcp) => match check_mcp(run, item) {
                Ok(agents) => {
                    queued.extend(agents.into_iter().map(|agent| (agent, index)));
                    Vec::new()
                }
                Err(step) => vec![(None, step)],
            },
        });
    }
    queued.sort();
    for batch in queued.chunk_by(|a, b| a.0 == b.0) {
        let agent = &batch[0].0;
        let batch: Vec<(usize, &SetupItem)> = batch
            .iter()
            .map(|(_, index)| (*index, &plan[*index].item))
            .collect();
        for ((index, _), step) in batch.iter().zip(apply_mcps(run, agent, &batch)) {
            steps[*index].push((Some(agent.clone()), step));
        }
    }
    plan.iter()
        .zip(steps)
        .map(|(Planned { item, .. }, steps)| {
            let step = worst(steps.iter().map(|(_, step)| step.clone()));
            let mut targets: Vec<TargetResult> = steps
                .into_iter()
                .filter_map(|(agent, step)| {
                    Some(TargetResult {
                        agent: agent?,
                        status: step.status,
                        reason: step.reason,
                    })
                })
                .collect();
            // Listed per agent unless every agent has the item's own result.
            if targets
                .iter()
                .all(|target| (target.status, &target.reason) == (step.status, &step.reason))
            {
                targets.clear();
            }
            let (agents, unsupported) = item
                .agents
                .iter()
                .cloned()
                .partition(|agent| run.supports(item.kind, agent));
            ItemResult {
                kind: item.kind,
                id: item.id.clone(),
                agents,
                unsupported,
                targets,
                status: step.status,
                reason: step.reason,
                retryable: (step.status == Status::Failed).then_some(step.retryable),
                asks: step.asks,
            }
        })
        .collect()
}

impl ItemResult {
    /// Replace this item's result with `failure`.
    pub(super) fn fail(&mut self, failure: Step) {
        self.status = failure.status;
        self.reason = failure.reason;
        self.retryable = Some(failure.retryable);
        self.targets.clear();
        self.asks = None;
    }

    /// Whether this item, or any of its agents, changed.
    pub(super) fn changed(&self) -> bool {
        self.status == Status::Changed
            || self
                .targets
                .iter()
                .any(|target| target.status == Status::Changed)
    }
}

/// An item's agents followed by every other agent that would read what it
/// writes: each sees the item, so each must be approved. An item with no
/// agent it can be set up for writes nothing, so it gains no readers: no
/// targeting never expands to every agent.
pub(super) fn with_readers(run: &Run, item: &SetupItem) -> Result<Vec<String>, AppError> {
    if !item
        .agents
        .iter()
        .any(|agent| run.supports(item.kind, agent))
    {
        return Ok(item.agents.clone());
    }
    let readers = match item.kind {
        ItemKind::Skill => skill_readers(run, item),
        ItemKind::Instruction => instruction_readers(run, item)?,
        ItemKind::Mcp => Vec::new(),
    };
    let mut agents = item.agents.clone();
    for reader in readers {
        if !agents.contains(&reader) {
            agents.push(reader);
        }
    }
    Ok(agents)
}

/// The native paths `item` writes that agents outside it also read.
pub(super) fn shared_paths(run: &Run, item: &SetupItem) -> Result<Vec<String>, AppError> {
    Ok(match item.kind {
        ItemKind::Skill => skill_shared_paths(run, item),
        ItemKind::Instruction => instruction_shared_paths(run, item)?,
        ItemKind::Mcp => Vec::new(),
    })
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

/// Lookups made once per request rather than once per item. The Library is
/// read only for the items a request names, so an unreadable item it does
/// not name never blocks it.
pub(super) struct Run<'a> {
    pub(super) ctx: &'a HostCtx,
    pub(super) project: ProjectRecord,
    /// False for inspection: check each destination and write nothing.
    pub(super) write: bool,
    pub(super) skill_groups: Vec<(AgentSkillConfig, Vec<String>)>,
    pub(super) available_skill_groups: HashSet<String>,
    pub(super) instruction_recovery_pending: bool,
    /// The Library's Skills looked up so far, by ID; `None` for one it lacks.
    skills: RefCell<BTreeMap<String, Option<SkillRecord>>>,
    /// The Library's Instruction bundles and MCP connections looked up so
    /// far, by ID; `None` for one the Library confirms is gone.
    instructions: RefCell<BTreeMap<String, Option<Value>>>,
    mcp_definitions: RefCell<BTreeMap<String, Option<Value>>>,
    /// This project's MCP deployments looked up so far, by agent.
    mcp_deployments: RefCell<BTreeMap<String, Vec<Value>>>,
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
        Ok(Self {
            skills: RefCell::default(),
            instructions: RefCell::default(),
            mcp_definitions: RefCell::default(),
            mcp_deployments: RefCell::default(),
            skill_groups: agent_skill_groups(&ctx.store),
            available_skill_groups: available_agent_keys(&ctx.store, &project),
            instruction_recovery_pending: instructions::recovery_pending(ctx)?,
            mcp_agents,
            ctx,
            project,
            write,
        })
    }

    /// Whether `agent` can have this kind of item set up in the project.
    pub(super) fn supports(&self, kind: ItemKind, agent: &str) -> bool {
        match kind {
            ItemKind::Skill => self.skill_groups.iter().any(|(config, members)| {
                self.available_skill_groups.contains(&config.key)
                    && members.iter().any(|member| member == agent)
            }),
            ItemKind::Instruction => instructions::AGENT_DIRS
                .iter()
                .any(|(key, _)| *key == agent),
            ItemKind::Mcp => self.mcp_agents.iter().any(|key| key == agent),
        }
    }

    /// Look up every Skill `items` name and this run has not looked up yet,
    /// in one query.
    pub(super) fn load_skills<'i>(
        &self,
        items: impl IntoIterator<Item = (ItemKind, &'i str)>,
    ) -> Result<(), AppError> {
        let mut known = self.skills.borrow_mut();
        let mut wanted: Vec<&str> = items
            .into_iter()
            .filter(|(kind, id)| *kind == ItemKind::Skill && !known.contains_key(*id))
            .map(|(_, id)| id)
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        if wanted.is_empty() {
            return Ok(());
        }
        let found = self
            .ctx
            .store
            .get_skills_by_ids(&wanted)
            .map_err(AppError::db)?;
        known.extend(wanted.into_iter().map(|id| (id.to_owned(), None)));
        known.extend(
            found
                .into_iter()
                .map(|skill| (skill.id.clone(), Some(skill))),
        );
        Ok(())
    }

    /// The Library's Skill with this ID.
    pub(super) fn skill(&self, id: &str) -> Result<Option<SkillRecord>, AppError> {
        if let Some(found) = self.skills.borrow().get(id) {
            return Ok(found.clone());
        }
        let found = self.ctx.store.get_skill_by_id(id).map_err(AppError::db)?;
        self.skills
            .borrow_mut()
            .insert(id.to_owned(), found.clone());
        Ok(found)
    }

    /// The Library's Instruction bundle with this ID, or `None` when the
    /// Library confirms it is gone.
    pub(super) fn instruction(&self, id: &str) -> Result<Option<Value>, AppError> {
        looked_up(&self.instructions, id, || {
            let mut reply = instructions::dispatch(self.ctx, json!({ "action": "get", "id": id }))?;
            Ok(reply["item"].take())
        })
    }

    /// The Library's MCP connection with this ID, or `None` when the
    /// Library confirms it is gone.
    pub(super) fn mcp_definition(&self, id: &str) -> Result<Option<Value>, AppError> {
        looked_up(&self.mcp_definitions, id, || {
            let mut reply = mcps::dispatch(self.ctx, json!({ "action": "get", "id": id }))?;
            Ok(reply["definition"].take())
        })
    }

    /// The override an agent's existing deployment of this MCP connection
    /// applies in this project, if any.
    pub(super) fn mcp_override(&self, agent: &str, id: &str) -> Result<Option<Value>, AppError> {
        if self.project.id.is_empty() {
            return Ok(None);
        }
        let mut known = self.mcp_deployments.borrow_mut();
        if !known.contains_key(agent) {
            let target = json!({ "agentKey": agent, "projectId": self.project.id });
            let mut reply = mcps::dispatch(
                self.ctx,
                json!({ "action": "deployments", "target": target }),
            )?;
            let deployments = match reply["deployments"].take() {
                Value::Array(deployments) => deployments,
                _ => Vec::new(),
            };
            known.insert(agent.to_owned(), deployments);
        }
        Ok(known[agent]
            .iter()
            .find(|d| d["definitionId"] == id)
            .map(|d| d["overrideConfig"].clone())
            .filter(|o| !o.is_null()))
    }
}

/// `id`'s entry in `known`, read with `read` the first time. Not found is
/// `None`; any other failure is not kept, so a later lookup reads again.
fn looked_up(
    known: &RefCell<BTreeMap<String, Option<Value>>>,
    id: &str,
    read: impl FnOnce() -> Result<Value, AppError>,
) -> Result<Option<Value>, AppError> {
    if let Some(found) = known.borrow().get(id) {
        return Ok(found.clone());
    }
    let found = match read() {
        Ok(found) => Some(found),
        Err(error) if error.kind == ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    known.borrow_mut().insert(id.to_owned(), found.clone());
    Ok(found)
}

/// Not found means the Library confirms the item is gone; any other failure
/// to read it leaves the question open.
fn missing(error: AppError) -> Step {
    if error.kind == ErrorKind::NotFound {
        Step::asks(Status::Missing, Reason::ConfirmedAbsent)
    } else {
        Step::because(Status::Missing, "unavailable")
    }
}

/// The worst of several agents' steps for one item. Of equal failures, one
/// that a retry cannot fix wins.
fn worst(steps: impl IntoIterator<Item = Step>) -> Step {
    steps
        .into_iter()
        .max_by_key(|step| (step.status, !step.retryable))
        .unwrap_or_else(|| Step::because(Status::Blocked, Reason::NoSupportedAgent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_agent_failing_for_good_makes_the_item_not_retryable() {
        for steps in [
            [
                Step::failed("apply_incomplete", false),
                Step::failed("io", true),
            ],
            [
                Step::failed("io", true),
                Step::failed("apply_incomplete", false),
            ],
        ] {
            let step = worst(steps);
            assert_eq!(step.reason.as_deref(), Some("apply_incomplete"));
            assert!(!step.retryable);
        }
    }
}
