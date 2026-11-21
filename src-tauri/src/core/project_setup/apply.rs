//! Apply a saved Project setup to one exact checkout and verify it, in one
//! request.
//!
//! The caller's `operation_id` names a receipt in `resource_state`. The
//! request claims it (version 0 to 1) before its first write, so only one
//! request executes per ID, and records the result (version 1 to 2). The
//! same ID asked again answers from the receipt.
//!
//! Each item is checked against its destination first. Only an absent or
//! cleanly replaceable destination is written, then checked again; anything
//! edited, unapproved or ambiguous is reported and left untouched, while
//! independent items carry on.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{find_project, ItemKind, SavedSetup, SetupItem, SETUP_KIND};
use crate::commands::projects::{
    agent_skill_configs, agent_skill_groups, available_agent_keys, ensure_safe_skill_relative_path,
    is_copy_project,
};
use crate::core::error::{AppError, ErrorKind};
use crate::core::git_backup_store::effective_device_name;
use crate::core::host::HostCtx;
use crate::core::project_scanner::AgentSkillConfig;
use crate::core::skill_store::{ProjectRecord, SkillRecord};
use crate::core::{content_hash, instructions, mcps, project_deploy, sync_engine};

const RECEIPT_KIND: &str = "project_setup_receipt";
const MAX_OPERATION_ID: usize = 128;
/// The most MCP connections one preview may change; see `mcps::adapters::preview`.
const MCP_BATCH: usize = 64;
/// Agents with a project instruction root; see `instructions::resolve_target`.
const INSTRUCTION_AGENTS: &[&str] = &["claude_code", "codex", "antigravity", "hermes", "cursor"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    // Declared from least to most in need of attention.
    Unchanged,
    Changed,
    Missing,
    Blocked,
    NeedsReview,
    Failed,
}

/// One item's verified result. `reason` says why it was not applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ItemResult {
    kind: ItemKind,
    id: String,
    agents: Vec<String>,
    status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Clone)]
struct Step(Status, Option<String>);

impl Step {
    fn of(status: Status) -> Self {
        Self(status, None)
    }
    fn because(status: Status, reason: impl Into<String>) -> Self {
        Self(status, Some(reason.into()))
    }
}

/// The value an MCP connection's approval covers: what it runs and with
/// which credentials references. Compared verbatim, never normalised.
pub fn mcp_value(definition: &Value) -> Value {
    json!({
        "transport": definition["transport"],
        "server": definition["server"],
        "auth": definition.get("auth"),
    })
}

pub(super) fn apply(
    ctx: &HostCtx,
    checkout_path: &str,
    operation_id: &str,
    evidence: bool,
) -> Result<Value, AppError> {
    if operation_id.trim().is_empty() || operation_id.len() > MAX_OPERATION_ID {
        return Err(AppError::invalid_input(format!(
            "operation_id must be 1-{MAX_OPERATION_ID} characters"
        )));
    }
    let (checkout, project) = find_project(ctx, checkout_path)?;
    let project = project.ok_or_else(|| {
        AppError::invalid_input("This checkout is not a registered project; discover it first")
    })?;
    let setup: SavedSetup = ctx
        .store
        .resource_state_get(SETUP_KIND, &project.id)
        .map_err(AppError::db)?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| AppError::internal(format!("Saved Project setup is unreadable: {e}")))?
        .ok_or_else(|| AppError::invalid_input("This project has no saved Project setup"))?;
    let host = effective_device_name(&ctx.store);
    let intent = json!({
        "host": host,
        "checkout_path": checkout,
        "project_id": project.id,
        "setup_version": setup.version,
    });

    // `checkout` is canonical and the project's exact path, so it is the root.
    // Read before the claim: a failure here must not leave a claimed receipt.
    let project = ProjectRecord {
        path: checkout.to_string_lossy().into_owned(),
        ..project
    };
    let run = Run::new(ctx, project)?;

    let claim = json!({ "version": 1, "intent": intent });
    if !ctx
        .store
        .resource_state_put_versioned(RECEIPT_KIND, operation_id, 0, &claim)
        .map_err(AppError::db)?
    {
        return answer_from_receipt(ctx, operation_id, &intent, evidence);
    }

    // Each item's steps, one per agent where it varies by agent. MCP
    // connections are queued and applied in batches per agent config.
    let mut steps: Vec<Vec<Step>> = Vec::with_capacity(setup.items.len());
    let mut queued: Vec<(String, usize)> = Vec::new();
    for (index, item) in setup.items.iter().enumerate() {
        steps.push(Vec::new());
        let step = match item.kind {
            _ if item.agents.is_empty() => Ok(Step::because(Status::Blocked, "no_agents")),
            ItemKind::Skill => apply_skill(&run, item),
            ItemKind::Instruction => apply_instruction(&run, item),
            ItemKind::Mcp => match check_mcp(&run, item) {
                Ok(agents) => {
                    queued.extend(agents.into_iter().map(|agent| (agent, index)));
                    continue;
                }
                Err(step) => Ok(step),
            },
        };
        steps[index]
            .push(step.unwrap_or_else(|error| Step::because(Status::Failed, error.message)));
    }
    queued.sort();
    for batch in queued.chunk_by(|a, b| a.0 == b.0) {
        let agent = &batch[0].0;
        let batch: Vec<(usize, &SetupItem)> = batch
            .iter()
            .map(|(_, index)| (*index, &setup.items[*index]))
            .collect();
        for ((index, _), step) in batch.iter().zip(apply_mcps(&run, agent, &batch)) {
            steps[*index].push(step);
        }
    }
    let items: Vec<ItemResult> = setup
        .items
        .iter()
        .zip(steps)
        .map(|(item, steps)| {
            let Step(status, reason) = worst(steps);
            ItemResult {
                kind: item.kind,
                id: item.id.clone(),
                agents: item.agents.clone(),
                status,
                reason,
            }
        })
        .collect();
    let mut result = summarize(&intent, operation_id, &items);
    result["items"] = json!(items);
    let receipt = json!({ "version": 2, "intent": intent, "result": result });
    ctx.store
        .resource_state_put_versioned(RECEIPT_KIND, operation_id, 1, &receipt)
        .map_err(AppError::db)?;
    Ok(respond(result, false, evidence))
}

fn answer_from_receipt(
    ctx: &HostCtx,
    operation_id: &str,
    intent: &Value,
    evidence: bool,
) -> Result<Value, AppError> {
    let receipt = ctx
        .store
        .resource_state_get(RECEIPT_KIND, operation_id)
        .map_err(AppError::db)?
        .unwrap_or_default();
    if receipt["intent"] != *intent {
        return Err(AppError::invalid_input(
            "This operation_id was used for a different request; use a new one",
        ));
    }
    if receipt["result"].is_object() {
        return Ok(respond(receipt["result"].clone(), true, evidence));
    }
    let mut unknown = summarize(intent, operation_id, &[]);
    unknown["outcome"] = json!("outcome_unknown");
    unknown["next_action"] = json!(
        "Another request with this operation_id has not recorded a result. \
         Ask again later with the same operation_id; do not retry under a new one."
    );
    Ok(respond(unknown, true, false))
}

fn respond(mut result: Value, from_receipt: bool, evidence: bool) -> Value {
    result["from_receipt"] = json!(from_receipt);
    if !evidence {
        if let Some(fields) = result.as_object_mut() {
            fields.remove("items");
        }
    }
    result
}

fn summarize(intent: &Value, operation_id: &str, items: &[ItemResult]) -> Value {
    let count = |status| items.iter().filter(|item| item.status == status).count();
    let mut agents: Vec<&str> = items
        .iter()
        .flat_map(|item| item.agents.iter().map(String::as_str))
        .collect();
    agents.sort_unstable();
    agents.dedup();
    let worst = items.iter().map(|item| item.status).max();
    let (outcome, next_action) = match worst {
        None | Some(Status::Unchanged) => (
            "verified_no_change",
            "None: the checkout matches its saved setup.",
        ),
        Some(Status::Changed) => (
            "verified_changed",
            "None: the changes were applied and verified.",
        ),
        Some(_) => (
            "attention_needed",
            "Independent items were applied. Ask again with the same operation_id and \
             evidence: true to list the items that need a decision, retry or recovery.",
        ),
    };
    let mut notes = Vec::new();
    if items
        .iter()
        .any(|item| item.kind == ItemKind::Mcp && item.agents.iter().any(|a| a == "codex"))
    {
        notes.push("Codex loads project MCP configuration only in a trusted workspace.");
    }
    json!({
        "operation_id": operation_id,
        "host": intent["host"],
        "checkout_path": intent["checkout_path"],
        "project_id": intent["project_id"],
        "setup_version": intent["setup_version"],
        "outcome": outcome,
        "agents": agents,
        "counts": {
            "changed": count(Status::Changed),
            "unchanged": count(Status::Unchanged),
            "needs_review": count(Status::NeedsReview),
            "blocked": count(Status::Blocked),
            "missing": count(Status::Missing),
            "failed": count(Status::Failed),
        },
        "next_action": next_action,
        "notes": notes,
    })
}

/// Lookups made once per request rather than once per item.
struct Run<'a> {
    ctx: &'a HostCtx,
    project: ProjectRecord,
    skill_groups: Vec<(AgentSkillConfig, Vec<String>)>,
    available_skill_groups: HashSet<String>,
    instruction_recovery_pending: bool,
    mcp_definitions: Vec<Value>,
    /// Agents with project-scoped MCP configuration.
    mcp_agents: Vec<String>,
}

impl<'a> Run<'a> {
    fn new(ctx: &'a HostCtx, project: ProjectRecord) -> Result<Self, AppError> {
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
        })
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
        .max_by_key(|step| step.0)
        .unwrap_or_else(|| Step::because(Status::Blocked, "no_supported_agent"))
}

// ── Skills ──

/// What sits at a deployment slot, compared with what belongs there.
#[derive(PartialEq)]
enum Slot {
    Absent,
    Same,
    Different,
}

/// A link to `expected`, or content equal to it, is the same. An unedited
/// copy behind the Library reads as different: the accepted v1 limit.
fn slot(path: &Path, expected: &Path) -> Slot {
    if std::fs::symlink_metadata(path).is_err() {
        return Slot::Absent;
    }
    let same_target = path.canonicalize().ok().is_some_and(|target| {
        expected
            .canonicalize()
            .is_ok_and(|expected| expected == target)
    });
    let same_content = || {
        path.is_dir()
            && content_hash::hash_directory(path).ok()
                == content_hash::hash_directory(expected).ok()
    };
    if same_target || same_content() {
        Slot::Same
    } else {
        Slot::Different
    }
}

fn apply_skill(run: &Run, item: &SetupItem) -> Result<Step, AppError> {
    let (ctx, project) = (run.ctx, &run.project);
    let skill: SkillRecord = match ctx.store.get_skill_by_id(&item.id).map_err(AppError::db)? {
        Some(skill) => skill,
        None => return Ok(Step::because(Status::Missing, "confirmed_absent")),
    };
    let source = PathBuf::from(&skill.central_path);
    if !source.is_dir() {
        return Ok(Step::because(Status::Missing, "unavailable"));
    }
    let dir_name = sync_engine::target_dir_name(&source, &skill.name);
    ensure_safe_skill_relative_path(&dir_name)?;
    let root = Path::new(&project.path);
    let copy_mode = is_copy_project(project);

    let groups: Vec<&(AgentSkillConfig, Vec<String>)> = run
        .skill_groups
        .iter()
        .filter(|(config, members)| {
            run.available_skill_groups.contains(&config.key)
                && members.iter().any(|m| item.agents.contains(m))
        })
        .collect();
    if groups.is_empty() {
        return Ok(Step::because(Status::Blocked, "no_supported_agent"));
    }
    // Every agent that reads a folder this writes to sees the skill, so each
    // must be approved. A copy-mode project also writes `.agents/skills`.
    let mut readers = run.skill_groups.iter().filter(|(config, _)| {
        groups.iter().any(|(group, _)| group.key == config.key)
            || (copy_mode && project_deploy::is_vendored_dir(&config.relative_skills_dir))
    });
    if readers.any(|(_, members)| members.iter().any(|member| !item.agents.contains(member))) {
        return Ok(Step::because(Status::NeedsReview, "unapproved_agents"));
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

    if copy_mode {
        let keys: Vec<String> = groups
            .iter()
            .map(|(config, _)| config.key.clone())
            .collect();
        let configs = agent_skill_configs(&ctx.store);
        let failed = project_deploy::deploy_copy_mode(root, &configs, &source, &dir_name, &keys)
            .map_err(AppError::io)?;
        if let Some(failure) = failed.first() {
            return Ok(Step::because(Status::Failed, failure.error.clone()));
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
        Step::because(Status::Failed, "verification_failed")
    })
}

// ── Instruction bundles ──

fn apply_instruction(run: &Run, item: &SetupItem) -> Result<Step, AppError> {
    let (ctx, project) = (run.ctx, &run.project);
    if let Err(error) = instructions::dispatch(ctx, json!({ "action": "get", "id": item.id })) {
        return Ok(missing(error));
    }
    let agents: Vec<&String> = item
        .agents
        .iter()
        .filter(|agent| INSTRUCTION_AGENTS.contains(&agent.as_str()))
        .collect();
    if agents.is_empty() {
        return Ok(Step::because(Status::Blocked, "no_supported_agent"));
    }
    if run.instruction_recovery_pending {
        return Ok(Step::because(Status::Blocked, "pending_recovery"));
    }
    agents
        .into_iter()
        .map(|agent| {
            let preview = |dry_run: bool| {
                instructions::dispatch(
                    ctx,
                    json!({ "action": "preview", "instruction_id": item.id, "dry_run": dry_run,
                            "target": { "agent_key": agent, "project_id": project.id } }),
                )
            };
            let has = |preview: &Value, status: &str| {
                preview["changes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|change| change["status"] == status)
            };
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
            if has(&planned, "delete") {
                return Ok(Step::because(Status::NeedsReview, "would_delete"));
            }
            if unchanged(&planned) {
                return Ok(Step::of(Status::Unchanged));
            }
            let stored = preview(false)?;
            let applied = instructions::dispatch(
                ctx,
                json!({ "action": "apply", "preview_id": stored["preview_id"] }),
            )?;
            if applied["status"] != "complete" {
                return Ok(Step::because(Status::Failed, "apply_incomplete"));
            }
            Ok(if unchanged(&preview(true)?) {
                Step::of(Status::Changed)
            } else {
                Step::because(Status::Failed, "verification_failed")
            })
        })
        .collect::<Result<Vec<_>, AppError>>()
        .map(worst)
}

// ── MCP connections ──

/// The agents to set an MCP connection up for, or why none can be.
fn check_mcp(run: &Run, item: &SetupItem) -> Result<Vec<String>, Step> {
    let Some(definition) = run
        .mcp_definitions
        .iter()
        .find(|d| d["id"] == item.id.as_str())
    else {
        return Err(Step::because(Status::Missing, "confirmed_absent"));
    };
    let agents: Vec<String> = item
        .agents
        .iter()
        .filter(|agent| run.mcp_agents.contains(agent))
        .cloned()
        .collect();
    if agents.is_empty() {
        return Err(Step::because(Status::Blocked, "no_supported_agent"));
    }
    match &item.approved {
        None => Err(Step::because(Status::NeedsReview, "approval_needed")),
        Some(approved) if *approved != mcp_value(definition) => {
            Err(Step::because(Status::NeedsReview, "value_changed"))
        }
        Some(_) => Ok(agents),
    }
}

/// Set up one agent's MCP connections: per batch, one preview to check, one
/// to apply and one to verify. Returns a step per connection, in order.
fn apply_mcps(run: &Run, agent: &str, batch: &[(usize, &SetupItem)]) -> Vec<Step> {
    let target = json!({ "agentKey": agent, "projectId": run.project.id });
    let pending = mcps::dispatch(
        run.ctx,
        json!({ "action": "deployments", "target": target }),
    )
    .map(|reply| {
        reply["recoveries"]
            .as_array()
            .is_some_and(|r| !r.is_empty())
    });
    match pending {
        Ok(false) => {}
        Ok(true) => return vec![Step::because(Status::Blocked, "pending_recovery"); batch.len()],
        Err(error) => return vec![Step::because(Status::Failed, error.message); batch.len()],
    }
    let ids: Vec<&str> = batch.iter().map(|(_, item)| item.id.as_str()).collect();
    ids.chunks(MCP_BATCH)
        .flat_map(|chunk| mcp_chunk(run.ctx, &target, chunk))
        .collect()
}

fn mcp_chunk(ctx: &HostCtx, target: &Value, ids: &[&str]) -> Vec<Step> {
    let preview = |ids: &[&str], dry_run: bool| {
        let operations: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "kind": "deploy", "definitionId": id, "conflict": "keep" }))
            .collect();
        mcps::dispatch(
            ctx,
            json!({ "action": "preview", "target": target, "dryRun": dry_run,
                    "operations": operations }),
        )
    };
    let planned = match preview(ids, true) {
        Ok(planned) => planned,
        // One connection the preview refuses fails the whole batch; find it.
        Err(_) if ids.len() > 1 => {
            return ids
                .iter()
                .flat_map(|id| mcp_chunk(ctx, target, &[id]))
                .collect()
        }
        Err(error) if error.kind == ErrorKind::InvalidInput => {
            return vec![Step::because(Status::NeedsReview, error.message)]
        }
        Err(error) => return vec![Step::because(Status::Failed, error.message)],
    };
    let mut steps: Vec<Option<Step>> = change_kinds(&planned)
        .into_iter()
        .map(|kind| match kind {
            "unchanged" => Some(Step::of(Status::Unchanged)),
            "add" | "update" => None,
            _ => Some(Step::because(Status::NeedsReview, "edited_here")),
        })
        .collect();
    steps.resize(ids.len(), Some(Step::because(Status::Failed, "no_preview")));
    let eligible: Vec<usize> = (0..ids.len()).filter(|i| steps[*i].is_none()).collect();
    if !eligible.is_empty() {
        let eligible_ids: Vec<&str> = eligible.iter().map(|i| ids[*i]).collect();
        let applied = preview(&eligible_ids, false).and_then(|stored| {
            mcps::dispatch(
                ctx,
                json!({ "action": "apply", "previewId": stored["previewId"] }),
            )
        });
        let verified = match applied {
            Ok(reply) if reply["applied"] == true => preview(&eligible_ids, true)
                .map(|checked| {
                    change_kinds(&checked)
                        .iter()
                        .map(|k| *k == "unchanged")
                        .collect::<Vec<_>>()
                })
                .map_err(|error| error.message),
            Ok(_) => Err("apply_incomplete".to_owned()),
            Err(error) => Err(error.message),
        };
        for (n, index) in eligible.iter().enumerate() {
            steps[*index] = Some(match &verified {
                Ok(checks) if checks.get(n) == Some(&true) => Step::of(Status::Changed),
                Ok(_) => Step::because(Status::Failed, "verification_failed"),
                Err(message) => Step::because(Status::Failed, message.clone()),
            });
        }
    }
    steps.into_iter().flatten().collect()
}

/// One kind per operation, in order. `update` with equal summaries is an
/// entry that already matches.
fn change_kinds(preview: &Value) -> Vec<&str> {
    preview["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|change| match change["kind"].as_str() {
            Some("update") if change["beforeSummary"] == change["afterSummary"] => "unchanged",
            Some(kind) => kind,
            None => "",
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde_json::{json, Value};

    use crate::core::error::AppError;
    use crate::core::host::{HostCtx, NoopEvents};
    use crate::core::host_dispatch::dispatch;
    use crate::core::skill_store::{ProjectRecord, SkillStore};
    use crate::core::test_support::{sample_skill, test_repo, write_skill_dir, TestRepo};

    /// A library with one custom agent, `agent_a` (`.a/skills`), and a
    /// registered project at `work/app`.
    struct Fixture {
        repo: TestRepo,
        ctx: HostCtx,
        checkout: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let repo = test_repo();
            let tmp = repo._tmp.path();
            let store = SkillStore::new(&tmp.join("repo/test.db")).unwrap();
            let tools = json!([{ "key": "agent_a", "display_name": "Agent A",
                "skills_dir": tmp.join("agents/a/skills"),
                "project_relative_skills_dir": ".a/skills" }]);
            std::fs::create_dir_all(tmp.join("agents/a")).unwrap();
            store
                .set_setting("custom_tools", &tools.to_string())
                .unwrap();
            let checkout = tmp.join("work/app");
            std::fs::create_dir_all(&checkout).unwrap();
            let checkout = checkout.canonicalize().unwrap();
            store
                .insert_project(&ProjectRecord {
                    id: "p1".into(),
                    name: "app".into(),
                    path: checkout.to_string_lossy().into_owned(),
                    workspace_type: "project".into(),
                    linked_agent_key: None,
                    linked_agent_name: None,
                    disabled_path: None,
                    sort_order: 0,
                    created_at: 0,
                    updated_at: 0,
                    agent_keys: None,
                    deploy_mode: "link".into(),
                })
                .unwrap();
            let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
            Self {
                repo,
                ctx,
                checkout,
            }
        }

        fn skill(&self, id: &str, name: &str) -> PathBuf {
            let dir = write_skill_dir(name);
            self.ctx
                .store
                .insert_skill(&sample_skill(id, name, &dir))
                .unwrap();
            dir
        }

        fn instruction(&self, files: Value) -> String {
            let saved = self.request(
                "instructions_request",
                json!({ "action": "save", "name": "Rules", "files": files }),
            );
            saved["item"]["id"].as_str().unwrap().to_owned()
        }

        fn mcp(&self, name: &str) -> Value {
            let saved = self.request(
                "mcps_request",
                json!({ "action": "save", "definition": {
                    "name": name, "transport": "stdio",
                    "server": { "command": "npx", "args": ["-y", name] } } }),
            );
            saved["definition"].clone()
        }

        fn request(&self, command: &str, request: Value) -> Value {
            dispatch(&self.ctx, command, &json!({ "request": request })).unwrap()
        }

        fn save_setup(&self, items: Value) {
            super::super::save_setup(
                &self.ctx.store,
                "p1",
                0,
                serde_json::from_value(items).unwrap(),
            )
            .unwrap();
        }

        fn apply(&self, operation_id: &str) -> Result<Value, AppError> {
            self.apply_with(json!({ "operation_id": operation_id }))
        }

        fn apply_with(&self, args: Value) -> Result<Value, AppError> {
            let mut request = json!({ "action": "apply", "checkout_path": self.checkout });
            request
                .as_object_mut()
                .unwrap()
                .extend(args.as_object().unwrap().clone());
            dispatch(
                &self.ctx,
                "project_setup_request",
                &json!({ "request": request }),
            )
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.checkout.join(relative)
        }
    }

    fn approved(definition: &Value) -> Value {
        json!({ "transport": definition["transport"], "server": definition["server"],
                "auth": definition.get("auth") })
    }

    /// One of each kind: a Skill for agent_a, and an Instruction bundle and an
    /// MCP connection for Claude Code.
    fn one_of_each(fixture: &Fixture) -> (PathBuf, Value) {
        let library = fixture.skill("s1", "review");
        let bundle = fixture.instruction(json!({ "CLAUDE.md": "Be brief.\n" }));
        let mcp = fixture.mcp("docs");
        fixture.save_setup(json!([
            { "kind": "skill", "id": "s1", "agents": ["agent_a"] },
            { "kind": "instruction", "id": bundle, "agents": ["claude_code"] },
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": approved(&mcp) },
        ]));
        (library, mcp)
    }

    fn mcp_entry(fixture: &Fixture, name: &str) -> Value {
        let text = std::fs::read_to_string(fixture.path(".mcp.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["mcpServers"][name].clone()
    }

    fn resolves_to(link: &Path, target: &Path) -> bool {
        link.canonicalize().ok() == target.canonicalize().ok()
    }

    #[test]
    fn one_call_applies_and_verifies_a_skill_an_instruction_and_an_mcp_connection() {
        let fixture = Fixture::new();
        let (library, _) = one_of_each(&fixture);

        let result = fixture.apply("op-1").unwrap();

        assert_eq!(result["outcome"], "verified_changed");
        assert_eq!(result["counts"]["changed"], 3);
        assert_eq!(result["project_id"], "p1");
        assert_eq!(result["checkout_path"], json!(fixture.checkout));
        assert_eq!(result["agents"], json!(["agent_a", "claude_code"]));
        assert!(result.get("items").is_none(), "evidence only on request");
        assert!(result.to_string().len() < 2048);
        assert!(resolves_to(&fixture.path(".a/skills/review"), &library));
        assert_eq!(
            std::fs::read_to_string(fixture.path("CLAUDE.md")).unwrap(),
            "Be brief.\n"
        );
        assert_eq!(mcp_entry(&fixture, "docs")["command"], "npx");
    }

    /// Every file and link under the checkout, plus the store's deployment
    /// records, to prove a call wrote nothing.
    fn destinations(fixture: &Fixture) -> Vec<(PathBuf, String)> {
        let mut found = Vec::new();
        let mut pending = vec![fixture.checkout.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                let content = if meta.file_type().is_symlink() {
                    format!("-> {:?}", std::fs::read_link(&path).unwrap())
                } else if meta.is_dir() {
                    pending.push(path.clone());
                    String::new()
                } else {
                    std::fs::read_to_string(&path).unwrap()
                };
                found.push((path, content));
            }
        }
        for kind in ["instructions_deployment", "mcps_deployment"] {
            let records = fixture.ctx.store.resource_state_list(kind).unwrap();
            found.push((kind.into(), json!(records).to_string()));
        }
        found.sort();
        found
    }

    fn item<'a>(result: &'a Value, kind: &str) -> &'a Value {
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} in {result}"))
    }

    #[test]
    fn a_rerun_writes_nothing_and_reports_verified_no_change() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        let before = destinations(&fixture);

        let rerun = fixture.apply("op-2").unwrap();

        assert_eq!(rerun["outcome"], "verified_no_change");
        assert_eq!(rerun["counts"]["unchanged"], 3);
        assert_eq!(rerun["from_receipt"], false);
        assert_eq!(destinations(&fixture), before);
    }

    #[test]
    fn an_edited_copy_survives_while_an_independent_addition_succeeds() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let skill = fixture.path(".a/skills/review");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: review\n---\nmine\n").unwrap();
        std::fs::write(fixture.path("CLAUDE.md"), "My own rules.\n").unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(result["outcome"], "attention_needed");
        assert_eq!(item(&result, "skill")["status"], "needs_review");
        assert_eq!(item(&result, "skill")["reason"], "edited_here");
        assert_eq!(item(&result, "instruction")["status"], "needs_review");
        assert_eq!(item(&result, "mcp")["status"], "changed");
        assert_eq!(
            std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            "---\nname: review\n---\nmine\n"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.path("CLAUDE.md")).unwrap(),
            "My own rules.\n"
        );
        assert_eq!(mcp_entry(&fixture, "docs")["command"], "npx");
    }

    #[test]
    fn a_copy_with_the_library_content_counts_as_no_change() {
        let fixture = Fixture::new();
        let library = fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let copy = fixture.path(".a/skills/review");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::copy(library.join("SKILL.md"), copy.join("SKILL.md")).unwrap();

        let result = fixture.apply("op-1").unwrap();

        assert_eq!(result["outcome"], "verified_no_change");
    }

    #[test]
    fn a_renamed_skill_still_resolves_but_a_new_skill_reusing_its_name_does_not() {
        let fixture = Fixture::new();
        let library = fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        fixture.apply("op-1").unwrap();
        let deployed = fixture.path(".a/skills/review");

        fixture.ctx.store.delete_skill("s1").unwrap();
        fixture
            .ctx
            .store
            .insert_skill(&sample_skill("s1", "Code review", &library))
            .unwrap();
        let renamed = fixture.apply("op-2").unwrap();
        assert_eq!(renamed["outcome"], "verified_no_change");

        fixture.ctx.store.delete_skill("s1").unwrap();
        let other = write_skill_dir("other/review");
        fixture
            .ctx
            .store
            .insert_skill(&sample_skill("s2", "review", &other))
            .unwrap();
        let replaced = fixture
            .apply_with(json!({ "operation_id": "op-3", "evidence": true }))
            .unwrap();

        assert_eq!(item(&replaced, "skill")["status"], "missing");
        assert_eq!(item(&replaced, "skill")["reason"], "confirmed_absent");
        assert!(resolves_to(&deployed, &library), "the deployment is kept");
    }

    #[test]
    fn agents_are_explicit_and_a_shared_folder_needs_every_reader_approved() {
        let fixture = Fixture::new();
        let tmp = fixture.repo._tmp.path();
        let tools = json!([
            { "key": "agent_a", "display_name": "Agent A", "skills_dir": tmp.join("agents/a/skills"),
              "project_relative_skills_dir": ".a/skills" },
            { "key": "agent_c", "display_name": "Agent C", "skills_dir": tmp.join("agents/c/skills"),
              "project_relative_skills_dir": ".a/skills" },
        ]);
        fixture
            .ctx
            .store
            .set_setting("custom_tools", &tools.to_string())
            .unwrap();
        fixture.skill("s1", "review");
        fixture.save_setup(json!([
            { "kind": "skill", "id": "s1", "agents": [] },
            { "kind": "skill", "id": "s1", "agents": ["not_an_agent"] },
            { "kind": "skill", "id": "s1", "agents": ["agent_a"] },
        ]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        let reasons: Vec<&Value> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["reason"])
            .collect();
        assert_eq!(
            reasons,
            [
                &json!("no_agents"),
                &json!("no_supported_agent"),
                &json!("unapproved_agents")
            ]
        );
        assert!(!fixture.path(".a").exists(), "nothing was deployed");
    }

    #[test]
    fn an_unmanaged_mcp_entry_is_kept_while_others_in_its_config_are_added() {
        let fixture = Fixture::new();
        let docs = fixture.mcp("docs");
        let api = fixture.mcp("api");
        fixture.save_setup(json!([
            { "kind": "mcp", "id": docs["id"], "agents": ["claude_code"], "approved": approved(&docs) },
            { "kind": "mcp", "id": api["id"], "agents": ["claude_code"], "approved": approved(&api) },
        ]));
        let own = json!({ "type": "stdio", "command": "my-docs" });
        std::fs::write(
            fixture.path(".mcp.json"),
            json!({ "mcpServers": { "docs": own } }).to_string(),
        )
        .unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        let statuses: Vec<&Value> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["status"])
            .collect();
        assert_eq!(statuses, [&json!("needs_review"), &json!("changed")]);
        assert_eq!(mcp_entry(&fixture, "docs"), own);
        assert_eq!(mcp_entry(&fixture, "api")["command"], "npx");
    }

    #[test]
    fn a_changed_mcp_value_waits_for_approval() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        let mut stale = approved(&mcp);
        stale["server"]["args"] = json!(["-y", "older"]);
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": stale },
        ]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "mcp")["status"], "needs_review");
        assert_eq!(item(&result, "mcp")["reason"], "value_changed");
        assert!(!fixture.path(".mcp.json").exists());
    }

    #[test]
    fn an_instruction_update_that_would_delete_a_file_waits_for_approval() {
        let fixture = Fixture::new();
        let saved = fixture.request(
            "instructions_request",
            json!({ "action": "save", "name": "Rules",
                    "files": { "CLAUDE.md": "a\n", ".claude/rules/extra.md": "b\n" } }),
        );
        let id = saved["item"]["id"].as_str().unwrap();
        fixture.save_setup(json!([{ "kind": "instruction", "id": id, "agents": ["claude_code"] }]));
        fixture.apply("op-1").unwrap();
        fixture.request(
            "instructions_request",
            json!({ "action": "save", "id": id, "name": "Rules", "files": { "CLAUDE.md": "a\n" },
                    "expected_revision": saved["item"]["revision"] }),
        );

        let result = fixture
            .apply_with(json!({ "operation_id": "op-2", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "instruction")["reason"], "would_delete");
        assert!(fixture.path(".claude/rules/extra.md").exists());
    }

    #[test]
    fn pending_recovery_blocks_its_items_and_is_never_run() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let store = &fixture.ctx.store;
        let journal = json!({ "id": "j1", "kind": "deployment", "status": "applying" });
        store
            .resource_state_put("instructions_journal", "j1", &journal)
            .unwrap();
        let recovery = json!({ "id": "r1",
            "target": { "agentKey": "claude_code", "projectId": "p1" } });
        store
            .resource_state_put("mcps_recovery", "r1", &recovery)
            .unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "instruction")["reason"], "pending_recovery");
        assert_eq!(item(&result, "mcp")["reason"], "pending_recovery");
        assert_eq!(item(&result, "skill")["status"], "changed");
        assert_eq!(
            store
                .resource_state_get("instructions_journal", "j1")
                .unwrap(),
            Some(journal)
        );
        assert_eq!(
            store.resource_state_get("mcps_recovery", "r1").unwrap(),
            Some(recovery)
        );
    }

    #[test]
    fn a_copy_mode_project_vendors_the_skill_and_links_its_agents() {
        let fixture = Fixture::new();
        let mut project = fixture.ctx.store.get_project_by_id("p1").unwrap().unwrap();
        project.deploy_mode = "copy".into();
        fixture.ctx.store.delete_project("p1").unwrap();
        fixture.ctx.store.insert_project(&project).unwrap();
        fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let unapproved = fixture
            .apply_with(json!({ "operation_id": "op-0", "evidence": true }))
            .unwrap();
        assert_eq!(item(&unapproved, "skill")["reason"], "unapproved_agents");
        assert!(!fixture.path(".agents").exists());

        // Every agent reading `.agents/skills` sees the vendored copy.
        let mut agents = vec!["agent_a".to_string()];
        for (config, members) in crate::commands::projects::agent_skill_groups(&fixture.ctx.store) {
            if crate::core::project_deploy::is_vendored_dir(&config.relative_skills_dir) {
                agents.extend(members);
            }
        }
        super::super::save_setup(
            &fixture.ctx.store,
            "p1",
            1,
            serde_json::from_value(json!([{ "kind": "skill", "id": "s1", "agents": agents }]))
                .unwrap(),
        )
        .unwrap();
        let first = fixture.apply("op-1").unwrap();
        let rerun = fixture.apply("op-2").unwrap();

        assert_eq!(first["outcome"], "verified_changed");
        let vendored = fixture.path(".agents/skills/review");
        assert!(vendored.join("SKILL.md").is_file());
        assert!(resolves_to(&fixture.path(".a/skills/review"), &vendored));
        assert_eq!(rerun["outcome"], "verified_no_change");
    }

    #[test]
    fn the_same_operation_id_answers_from_its_receipt() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let first = fixture.apply("op-1").unwrap();

        let again = fixture.apply("op-1").unwrap();
        assert_eq!(again["from_receipt"], true);
        assert_eq!(again["outcome"], first["outcome"]);
        assert_eq!(again["counts"], first["counts"]);

        let saved = fixture
            .ctx
            .store
            .resource_state_get("project_setup", "p1")
            .unwrap();
        let items = saved.unwrap()["items"].clone();
        super::super::save_setup(
            &fixture.ctx.store,
            "p1",
            1,
            serde_json::from_value(items).unwrap(),
        )
        .unwrap();
        let error = fixture.apply("op-1").unwrap_err();
        assert!(
            error.message.contains("different request"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_claimed_operation_without_a_result_is_outcome_unknown() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let first = fixture.apply("op-1").unwrap();
        let mut receipt = fixture
            .ctx
            .store
            .resource_state_get("project_setup_receipt", "op-1")
            .unwrap()
            .unwrap();
        receipt["version"] = json!(1);
        receipt.as_object_mut().unwrap().remove("result");
        fixture
            .ctx
            .store
            .resource_state_put("project_setup_receipt", "op-1", &receipt)
            .unwrap();

        let unknown = fixture.apply("op-1").unwrap();

        assert_eq!(first["outcome"], "verified_changed");
        assert_eq!(unknown["outcome"], "outcome_unknown");
        assert_eq!(unknown["from_receipt"], true);
    }

    #[test]
    fn two_concurrent_calls_with_the_same_id_apply_once() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let db = fixture.repo._tmp.path().join("repo/test.db");
        let calls: Vec<Value> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let db = db.clone();
                    let checkout = fixture.checkout.clone();
                    scope.spawn(move || {
                        let store = SkillStore::new(&db).unwrap();
                        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
                        let request = json!({ "action": "apply", "checkout_path": checkout,
                                              "operation_id": "op-1" });
                        dispatch(
                            &ctx,
                            "project_setup_request",
                            &json!({ "request": request }),
                        )
                        .unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let executed: Vec<&Value> = calls
            .iter()
            .filter(|c| c["from_receipt"] == false)
            .collect();
        assert_eq!(executed.len(), 1, "{calls:?}");
        assert_eq!(executed[0]["counts"]["changed"], 3);
    }

    /// Run with `--ignored --nocapture` to measure; the timing goes in the PR.
    #[test]
    #[ignore]
    fn measure_a_300_item_setup() {
        let fixture = Fixture::new();
        let mut items = Vec::new();
        for n in 0..100 {
            let id = format!("s{n:03}");
            fixture.skill(&id, &format!("skill-{n:03}"));
            items.push(json!({ "kind": "skill", "id": id, "agents": ["agent_a"] }));
            let file = format!(".claude/rules/rule-{n:03}.md");
            let bundle = fixture.instruction(json!({ file: format!("Rule {n}.\n") }));
            items.push(json!({ "kind": "instruction", "id": bundle, "agents": ["claude_code"] }));
            let mcp = fixture.mcp(&format!("server-{n:03}"));
            items.push(
                json!({ "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"],
                               "approved": approved(&mcp) }),
            );
        }
        fixture.save_setup(json!(items));

        let started = std::time::Instant::now();
        let first = fixture.apply("op-1").unwrap();
        let applied = started.elapsed();
        let started = std::time::Instant::now();
        let rerun = fixture.apply("op-2").unwrap();
        let verified = started.elapsed();

        assert_eq!(first["counts"]["changed"], 300, "{first}");
        assert_eq!(rerun["counts"]["unchanged"], 300, "{rerun}");
        assert!(first.to_string().len() < 2048);
        eprintln!("300 items: apply {applied:?}, verified rerun {verified:?}");
    }
}
