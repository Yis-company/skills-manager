//! Apply a saved Project setup to one exact checkout and verify it, in one
//! request.
//!
//! The caller's `operation_id` names a receipt in `resource_state`. The
//! request claims it (version 0 to 1) before its first write, so only one
//! request executes per ID, and records the result (version 1 to 2). The
//! same ID asked again answers from the receipt, for 30 days after it settles;
//! see `docs/operations.md`.
//!
//! Each item is checked and applied by [`super::items`]; decisions and their
//! confirmation are in [`super::confirm`].

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::confirm::{confirmed, decide, propose, save_confirmed, Plan, Proposal, Reason, CONFIRM};
use super::items::{check_items, counts, sorted_agents, ItemResult, Planned, Run, Status, Step};
use super::{check_agents, check_size, find_project, ItemKind, SavedSetup, SetupItem, SETUP_KIND};
use crate::commands::projects::new_project_deploy_mode;
use crate::core::error::AppError;
use crate::core::git_backup_store::device_name;
use crate::core::host::HostCtx;
use crate::core::skill_store::ProjectRecord;

pub(super) const RECEIPT_KIND: &str = "project_setup_receipt";
const MAX_OPERATION_ID: usize = 128;
/// Settled receipts are kept this long, so a retry within it answers from
/// its receipt.
const RECEIPT_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// The most expired receipts one apply deletes.
const RECEIPT_PRUNE_LIMIT: usize = 100;

/// What `project_setup_receipt` records for one `operation_id`: claimed at
/// version 1 with the request's intent and the items it resolved, then
/// settled at version 2 with its result or its error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Receipt {
    pub(super) version: u64,
    pub(super) intent: Value,
    #[serde(default)]
    pub(super) targets: Option<Vec<SetupItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<AppError>,
    /// Epoch milliseconds; see [`RECEIPT_RETENTION_MS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) settled_at: Option<i64>,
}

/// The apply action's arguments.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Args {
    checkout_path: String,
    operation_id: String,
    #[serde(default)]
    evidence: bool,
    /// A first setup's selection, to propose for confirmation.
    #[serde(default)]
    items: Option<Vec<SetupItem>>,
    /// An earlier proposal, sent back with a `choice` on each decision.
    #[serde(default)]
    confirm: Option<Value>,
    /// Agents for this run only, in place of each saved item's agents. The
    /// saved setup is left as it is.
    #[serde(default)]
    agents: Option<Vec<String>>,
}

/// Refuse a malformed request before reading anything for it.
fn check_args(args: &Args) -> Result<(), AppError> {
    let id = &args.operation_id;
    if id.trim().is_empty() || id.chars().count() > MAX_OPERATION_ID {
        return Err(AppError::invalid_input(format!(
            "operation_id must be 1-{MAX_OPERATION_ID} characters"
        )));
    }
    check_agents(args.agents.as_ref().map_or(0, Vec::len))?;
    if let Some(items) = &args.items {
        check_size(items.len())?;
        for item in items {
            check_agents(item.agents.len())?;
        }
    }
    if let Some(confirm) = &args.confirm {
        let decisions = confirm["decisions"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        check_size(decisions.len())?;
        for decision in decisions {
            check_agents(decision["agents"].as_array().map_or(0, Vec::len))?;
        }
    }
    Ok(())
}

/// Whether `args` only ask for a first setup's proposal.
pub(super) fn proposes_only(args: &Args) -> bool {
    args.items.is_some() && args.confirm.is_none()
}

fn no_saved_setup() -> AppError {
    AppError::invalid_input("This checkout has no saved Project setup; pass items to propose one")
}

/// Check `args` on `read_only`, then open the library for writing through
/// `writable` only when they hold something to apply.
pub(super) fn apply<'c>(
    read_only: &'c HostCtx,
    writable: &dyn Fn() -> Result<&'c HostCtx, AppError>,
    args: Args,
) -> Result<Value, AppError> {
    check_args(&args)?;
    let ctx = read_only;
    let (operation_id, evidence) = (args.operation_id.as_str(), args.evidence);
    let (checkout, project) = find_project(ctx, &args.checkout_path)?;
    let host = device_name(&ctx.store);
    // What the caller asked, without what this operation itself changes
    // (the project ID and the setup version), so a retry matches it.
    let mut intent = json!({ "host": host, "checkout_path": checkout });
    if let Some(confirm) = &args.confirm {
        intent["confirm"] = confirm.clone();
    }
    if let Some(agents) = &args.agents {
        intent["agents"] = json!(agents);
    }
    // A retry answers from its receipt before anything is checked again.
    if args.items.is_none() && receipt(ctx, operation_id)?.is_some() {
        return answer_from_receipt(ctx, operation_id, &intent, evidence);
    }
    let saved: Option<SavedSetup> = match &project {
        Some(project) => load_setup(ctx, &project.id)?,
        None => None,
    };

    // Read before the claim: a failure here must not leave a claimed receipt.
    let record = match project {
        Some(project) => project,
        None => unregistered(ctx, checkout.to_string_lossy().into_owned())?,
    };
    let mut proposal = Proposal {
        host: host.clone(),
        checkout_path: checkout.clone(),
        project_id: (!record.id.is_empty()).then(|| record.id.clone()),
        register: record.id.is_empty(),
        deploy_mode: record.deploy_mode.clone(),
        setup_version: saved.as_ref().map_or(0, |setup| setup.version),
        decisions: Vec::new(),
    };
    let mut run = Run::new(ctx, record, true)?;
    let mut scope = json!({
        "host": host,
        "checkout_path": checkout,
        "project_id": proposal.project_id,
        "setup_version": proposal.setup_version,
    });
    if let Some(agents) = &args.agents {
        check_named_agents(&run, agents, saved.is_some() && args.items.is_none())?;
    }
    // Agents named for one run stand in for each saved item's agents, in
    // its decisions too; confirming them never saves.
    let saved = saved.map(|setup| match &args.agents {
        Some(agents) => SavedSetup {
            items: setup
                .items
                .into_iter()
                .map(|item| SetupItem {
                    agents: agents.clone(),
                    ..item
                })
                .collect(),
            ..setup
        },
        None => setup,
    });
    let first = saved.is_none();
    let plan = match (saved, args.items, &args.confirm) {
        (None, Some(items), None) => {
            run.load_skills(items.iter().map(|item| (item.kind, item.id.as_str())))?;
            proposal.decisions = items
                .iter()
                .map(|item| {
                    decide(&run, item, Reason::FirstSetup)?.ok_or_else(|| {
                        AppError::invalid_input(format!("{} is not in the Library", item.id))
                    })
                })
                .collect::<Result<_, _>>()?;
            return Ok(propose(&scope, operation_id, proposal));
        }
        (_, Some(_), _) => return Err(AppError::invalid_input(
            "items propose a first setup only; this request has a saved setup or a confirmation",
        )),
        (None, None, None) => return Err(no_saved_setup()),
        (saved, None, Some(confirm)) => {
            let plan = confirmed(&run, &proposal, saved, confirm)?;
            Plan {
                save: plan.save && args.agents.is_none(),
                ..plan
            }
        }
        (Some(saved), None, None) => Plan {
            items: saved.items.into_iter().map(Planned::new).collect(),
            save: false,
        },
    };
    if first && plan.items.is_empty() {
        let mut result = summarize(&scope, operation_id, &[]);
        result["outcome"] = json!(Outcome::NothingSetUp);
        result["next_action"] = json!(
            "Nothing was set up: no item was approved, so nothing was registered, saved or \
             applied. To set this checkout up, apply again with items for a new proposal."
        );
        return Ok(respond(result, false, evidence));
    }
    let items: Vec<SetupItem> = plan.items.iter().map(|p| p.item.clone()).collect();

    // Checked, and something to apply: only now open the library for writing.
    run.ctx = writable()?;
    let ctx = run.ctx;
    let now = chrono::Utc::now().timestamp_millis();
    ctx.store
        .resource_state_delete_settled_before(
            RECEIPT_KIND,
            now - RECEIPT_RETENTION_MS,
            RECEIPT_PRUNE_LIMIT,
        )
        .map_err(AppError::db)?;
    // The items as resolved for this run, so inspection checks the same ones.
    let claim = Receipt {
        version: 1,
        intent: intent.clone(),
        targets: Some(items.clone()),
        result: None,
        error: None,
        settled_at: None,
    };
    if !ctx
        .store
        .resource_state_put_versioned(RECEIPT_KIND, operation_id, 0, &json!(claim))
        .map_err(AppError::db)?
    {
        return answer_from_receipt(ctx, operation_id, &intent, evidence);
    }
    // Whether the outcome is on record. Once anything may have been
    // written, an outcome that is not is reported as unknown.
    let settle = |outcome: Result<&Value, &AppError>| {
        let receipt = Receipt {
            version: 2,
            result: outcome.ok().cloned(),
            error: outcome.err().cloned(),
            settled_at: Some(chrono::Utc::now().timestamp_millis()),
            ..claim.clone()
        };
        ctx.store
            .resource_state_put_versioned(RECEIPT_KIND, operation_id, 1, &json!(receipt))
            .unwrap_or(false)
    };
    let unrecorded = || {
        let next_action = format!(
            "Outcome unknown: changes may have been made, but their result could not be \
             recorded. Leave the checkout as it is and call {INSPECT_TOOL} with the same \
             operation_id; do not apply under a new operation_id until then."
        );
        outcome_unknown(
            &intent["host"],
            &intent["checkout_path"],
            operation_id,
            &next_action,
        )
    };

    if plan.save {
        match save_confirmed(&mut run, &proposal, items.clone()) {
            Ok(version) => proposal.setup_version = version,
            Err(error) if settle(Err(&error)) => return Err(error),
            Err(_) => return Ok(unrecorded()),
        }
        scope["project_id"] = json!(run.project.id);
        scope["setup_version"] = json!(proposal.setup_version);
    }

    let mut results = check_items(&run, &plan.items);
    proposal.project_id = Some(run.project.id.clone());
    proposal.register = false;
    // A decision that cannot be read fails its item; it never just vanishes.
    for (item, checked) in items.iter().zip(results.iter_mut()) {
        let Some(reason) = checked.asks else { continue };
        match decide(&run, item, reason) {
            Ok(decision) => proposal.decisions.extend(decision),
            Err(error) => checked.fail(Step::error(error)),
        }
    }
    let mut result = summarize(&scope, operation_id, &results);
    // Agents named for this run were checked, not the saved setup's.
    if args.agents.is_some() && result["outcome"] == json!(Outcome::VerifiedNoChange) {
        result["next_action"] = json!(
            "None: the checkout matches the saved items for the agents named for this run; \
             other agents were not checked."
        );
    }
    if !proposal.decisions.is_empty() {
        result["next_action"] = json!(CONFIRM);
        result["proposal"] = json!(proposal);
    }
    result["items"] = json!(results);
    if !settle(Ok(&result)) {
        return Ok(unrecorded());
    }
    Ok(respond(result, false, evidence))
}

/// Agents named for one run: at least one, each able to have some kind of
/// item set up here, and only with a saved setup.
fn check_named_agents(run: &Run, agents: &[String], has_setup: bool) -> Result<(), AppError> {
    if !has_setup {
        return Err(AppError::invalid_input(
            "agents applies a saved setup for this run only; name agents in items instead",
        ));
    }
    if agents.is_empty() {
        return Err(AppError::invalid_input(
            "agents must name at least one agent; omit it to use the saved setup's agents",
        ));
    }
    let kinds = [ItemKind::Skill, ItemKind::Instruction, ItemKind::Mcp];
    match agents
        .iter()
        .find(|agent| !kinds.iter().any(|kind| run.supports(*kind, agent)))
    {
        Some(agent) => Err(AppError::invalid_input(format!(
            "{agent} cannot have a Skill, Instruction bundle or MCP connection set up in this \
             project"
        ))),
        None => Ok(()),
    }
}

pub(super) fn load_setup(ctx: &HostCtx, project_id: &str) -> Result<Option<SavedSetup>, AppError> {
    ctx.store
        .resource_state_get(SETUP_KIND, project_id)
        .map_err(AppError::db)?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| AppError::internal(format!("Saved Project setup is unreadable: {e}")))
}

/// The project a first setup registers: in memory until it is confirmed.
fn unregistered(ctx: &HostCtx, path: String) -> Result<ProjectRecord, AppError> {
    Ok(ProjectRecord {
        id: String::new(),
        name: String::new(),
        path,
        workspace_type: "project".into(),
        linked_agent_key: None,
        linked_agent_name: None,
        disabled_path: None,
        sort_order: 0,
        created_at: 0,
        updated_at: 0,
        agent_keys: None,
        deploy_mode: new_project_deploy_mode(&ctx.store, None)?,
    })
}

pub(super) fn receipt(ctx: &HostCtx, operation_id: &str) -> Result<Option<Receipt>, AppError> {
    ctx.store
        .resource_state_get(RECEIPT_KIND, operation_id)
        .map_err(AppError::db)?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| AppError::internal(format!("This operation_id's receipt is unreadable: {e}")))
}

fn answer_from_receipt(
    ctx: &HostCtx,
    operation_id: &str,
    intent: &Value,
    evidence: bool,
) -> Result<Value, AppError> {
    let receipt = receipt(ctx, operation_id)?.filter(|receipt| receipt.intent == *intent);
    let Some(receipt) = receipt else {
        return Err(AppError::invalid_input(
            "This operation_id was used for a different request; use a new one",
        ));
    };
    if let Some(error) = receipt.error {
        return Err(error);
    }
    if let Some(result) = receipt.result.filter(Value::is_object) {
        return Ok(respond(result, true, evidence));
    }
    let unknown = outcome_unknown(
        &intent["host"],
        &intent["checkout_path"],
        operation_id,
        "Another request with this operation_id has not recorded a result. Ask again later \
         with the same operation_id; do not retry under a new one.",
    );
    Ok(respond(unknown, true, false))
}

/// The tool that reads an apply's outcome back.
const INSPECT_TOOL: &str = "inspect_project_setup_result";

/// An apply result whose outcome is not on record, with no project ID or
/// setup version, and `next_action`.
pub fn outcome_unknown(
    host: &Value,
    checkout_path: &Value,
    operation_id: &str,
    next_action: &str,
) -> Value {
    let scope = json!({ "host": host, "checkout_path": checkout_path });
    let mut unknown = summarize(&scope, operation_id, &[]);
    unknown["outcome"] = json!(Outcome::Unknown);
    unknown["next_action"] = json!(next_action);
    respond(unknown, false, false)
}

pub(super) fn respond(mut result: Value, from_receipt: bool, evidence: bool) -> Value {
    result["from_receipt"] = json!(from_receipt);
    without_evidence(&mut result, evidence);
    result
}

/// Per-item results go out only when the caller asks for evidence.
pub(super) fn without_evidence(result: &mut Value, evidence: bool) {
    if let Some(fields) = result.as_object_mut().filter(|_| !evidence) {
        fields.remove("items");
    }
}

/// An apply's outcome. See `apply_project_setup`'s output schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    VerifiedChanged,
    VerifiedNoChange,
    PartialSuccess,
    NeedsReview,
    RetryableFailure,
    #[serde(rename = "outcome_unknown")]
    Unknown,
    ConfirmationNeeded,
    /// A first setup with nothing approved: nothing registered, saved or applied.
    NothingSetUp,
}

/// The outcome of a run and its next safe action. Changed and unchanged
/// items are settled; any other item needs a decision, a retry or recovery.
/// An item changed for one agent and held for another counts as both.
fn outcome(items: &[ItemResult]) -> (Outcome, &'static str) {
    let changed = items.iter().any(ItemResult::changed);
    let mut unsettled = items
        .iter()
        .filter(|item| !matches!(item.status, Status::Changed | Status::Unchanged))
        .peekable();
    if unsettled.peek().is_none() {
        return if changed {
            (
                Outcome::VerifiedChanged,
                "None: the changes were applied and verified.",
            )
        } else {
            (
                Outcome::VerifiedNoChange,
                "None: the checkout matches its saved setup.",
            )
        };
    }
    if changed {
        return (
            Outcome::PartialSuccess,
            "Independent items were applied and verified. Ask again with the same \
             operation_id and evidence: true to list the items that need a decision, retry \
             or recovery.",
        );
    }
    if unsettled.all(|item| item.retryable == Some(true)) {
        return (
            Outcome::RetryableFailure,
            "No change was verified. Apply again under a new operation_id: apply checks \
             each destination before writing and never overwrites edits.",
        );
    }
    (
        Outcome::NeedsReview,
        "No change was verified. Ask again with the same operation_id and evidence: true \
         to list the items that need a decision, retry or recovery.",
    )
}

pub(super) fn summarize(scope: &Value, operation_id: &str, items: &[ItemResult]) -> Value {
    let agents = sorted_agents(items.iter().map(|item| &item.agents));
    let (outcome, next_action) = outcome(items);
    let mut notes = Vec::new();
    if items
        .iter()
        .any(|item| item.kind == ItemKind::Mcp && item.agents.iter().any(|a| a == "codex"))
    {
        notes.push("Codex loads project MCP configuration only in a trusted workspace.");
    }
    json!({
        "operation_id": operation_id,
        "host": scope["host"],
        "checkout_path": scope["checkout_path"],
        "project_id": scope["project_id"],
        "setup_version": scope["setup_version"],
        "outcome": outcome,
        "agents": agents,
        "counts": counts(items, &[
            Status::Changed, Status::Unchanged, Status::NeedsReview,
            Status::Blocked, Status::Missing, Status::Failed,
        ]),
        "next_action": next_action,
        "notes": notes,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serde_json::{json, Value};

    use crate::core::error::{AppError, ErrorKind};
    use crate::core::host::{HostCtx, NoopEvents};
    use crate::core::host_dispatch::dispatch;
    use crate::core::skill_store::{ProjectRecord, SkillStore};
    use crate::core::test_support::{sample_skill, test_repo, write_skill_dir, TestRepo};

    /// A library with one custom agent, `agent_a` (`.a/skills`), and a
    /// registered project at `work/app`.
    pub(crate) struct Fixture {
        pub repo: TestRepo,
        pub ctx: HostCtx,
        pub checkout: PathBuf,
    }

    impl Fixture {
        pub fn new() -> Self {
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

        pub fn skill(&self, id: &str, name: &str) -> PathBuf {
            let dir = write_skill_dir(name);
            self.ctx
                .store
                .insert_skill(&sample_skill(id, name, &dir))
                .unwrap();
            dir
        }

        pub fn instruction(&self, files: Value) -> String {
            let saved = self.request(
                "instructions_request",
                json!({ "action": "save", "name": "Rules", "files": files }),
            );
            saved["item"]["id"].as_str().unwrap().to_owned()
        }

        pub fn mcp(&self, name: &str) -> Value {
            let saved = self.request(
                "mcps_request",
                json!({ "action": "save", "definition": {
                    "name": name, "transport": "stdio",
                    "server": { "command": "npx", "args": ["-y", name] } } }),
            );
            saved["definition"].clone()
        }

        pub fn request(&self, command: &str, request: Value) -> Value {
            dispatch(&self.ctx, command, &json!({ "request": request })).unwrap()
        }

        pub fn save_setup(&self, items: Value) {
            super::super::save_setup(
                &self.ctx.store,
                "p1",
                0,
                serde_json::from_value(items).unwrap(),
            )
            .unwrap();
        }

        pub fn apply(&self, operation_id: &str) -> Result<Value, AppError> {
            self.apply_with(json!({ "operation_id": operation_id }))
        }

        pub fn apply_with(&self, args: Value) -> Result<Value, AppError> {
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

        pub fn path(&self, relative: &str) -> PathBuf {
            self.checkout.join(relative)
        }
    }

    /// The Library value of `definition`, approved for Claude Code and Codex.
    pub(crate) fn approved(definition: &Value) -> Value {
        let value = json!({ "transport": definition["transport"], "server": definition["server"],
                            "auth": definition.get("auth") });
        json!({ "claude_code": value, "codex": value })
    }

    /// One of each kind: a Skill for agent_a, and an Instruction bundle and an
    /// MCP connection for Claude Code.
    pub(crate) fn one_of_each(fixture: &Fixture) -> (PathBuf, Value) {
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
    pub(crate) fn destinations(fixture: &Fixture) -> Vec<(PathBuf, String)> {
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

    pub(crate) fn item<'a>(result: &'a Value, kind: &str) -> &'a Value {
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

        assert_eq!(result["outcome"], "partial_success");
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

    #[cfg(unix)]
    #[test]
    fn an_unreadable_copy_is_never_counted_as_no_change() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let library = fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let copy = fixture.path(".a/skills/review");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("SKILL.md"), "edited here").unwrap();
        let locked = std::fs::Permissions::from_mode(0o000);
        std::fs::set_permissions(copy.join("SKILL.md"), locked.clone()).unwrap();
        std::fs::set_permissions(library.join("SKILL.md"), locked).unwrap();
        if std::fs::read(copy.join("SKILL.md")).is_ok() {
            return; // Running as root: nothing is unreadable.
        }

        let result = fixture.apply("op-1").unwrap();

        assert_ne!(result["outcome"], "verified_no_change", "{result}");
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

    /// Adds `agent_b`, reading its own folder `.b/skills`.
    pub(crate) fn with_agent_b(fixture: &Fixture) {
        let tmp = fixture.repo._tmp.path();
        let tools = json!([
            { "key": "agent_a", "display_name": "Agent A", "skills_dir": tmp.join("agents/a/skills"),
              "project_relative_skills_dir": ".a/skills" },
            { "key": "agent_b", "display_name": "Agent B", "skills_dir": tmp.join("agents/b/skills"),
              "project_relative_skills_dir": ".b/skills" },
        ]);
        fixture
            .ctx
            .store
            .set_setting("custom_tools", &tools.to_string())
            .unwrap();
        std::fs::create_dir_all(tmp.join("agents/b")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_entry_file_linked_to_a_bundle_file_needs_its_readers_approved() {
        let fixture = Fixture::new();
        let saved = fixture.request(
            "instructions_request",
            json!({ "action": "save", "name": "Rules", "files": { "CLAUDE.md": "rules\n" } }),
        );
        let id = saved["item"]["id"].as_str().unwrap();
        fixture.save_setup(json!([{ "kind": "instruction", "id": id, "agents": ["claude_code"] }]));
        std::os::unix::fs::symlink("CLAUDE.md", fixture.path("AGENTS.md")).unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(
            item(&result, "instruction")["reason"],
            "unapproved_agents",
            "{result}"
        );
        let decision = &result["proposal"]["decisions"][0];
        assert!(
            decision["proposed"]
                .as_array()
                .unwrap()
                .contains(&json!("codex")),
            "{result}"
        );
        assert_eq!(decision["paths"], json!(["AGENTS.md"]));
        assert!(!fixture.path("CLAUDE.md").exists(), "nothing was written");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_another_agents_copy_is_never_counted_as_no_change() {
        let fixture = Fixture::new();
        with_agent_b(&fixture);
        let library = fixture.skill("s1", "review");
        fixture.save_setup(json!([
            { "kind": "skill", "id": "s1", "agents": ["agent_a", "agent_b"] }
        ]));
        let copy = fixture.path(".a/skills/review");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::copy(library.join("SKILL.md"), copy.join("SKILL.md")).unwrap();
        std::fs::create_dir_all(fixture.path(".b/skills")).unwrap();
        std::os::unix::fs::symlink(&copy, fixture.path(".b/skills/review")).unwrap();

        let result = fixture.apply("op-1").unwrap();

        assert_ne!(result["outcome"], "verified_no_change", "{result}");
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_folder_linked_to_another_agents_folder_needs_that_agent_approved() {
        let fixture = Fixture::new();
        with_agent_b(&fixture);
        std::fs::create_dir_all(fixture.path(".a")).unwrap();
        std::os::unix::fs::symlink(fixture.path(".a"), fixture.path(".b")).unwrap();
        fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(
            item(&result, "skill")["reason"],
            "unapproved_agents",
            "{result}"
        );
        assert_eq!(
            result["proposal"]["decisions"][0]["proposed"],
            json!(["agent_a", "agent_b"])
        );
        assert!(!fixture.path(".a/skills").exists(), "nothing was added");

        // Approved for both, the one shared folder gets the Skill once.
        let applied = fixture
            .apply_with(json!({ "operation_id": "op-2", "confirm": confirm(&result, "approve") }))
            .unwrap();
        assert_eq!(applied["outcome"], "verified_changed", "{applied}");
        assert!(fixture.path(".a/skills/review").exists());
    }

    #[test]
    fn agents_named_in_a_request_apply_to_that_run_only() {
        let fixture = Fixture::new();
        with_agent_b(&fixture);
        fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let saved = || {
            fixture
                .ctx
                .store
                .resource_state_get("project_setup", "p1")
                .unwrap()
        };
        let before = saved();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "agents": ["agent_b"] }))
            .unwrap();

        assert_eq!(result["outcome"], "verified_changed", "{result}");
        assert_eq!(result["agents"], json!(["agent_b"]));
        assert!(fixture.path(".b/skills/review").exists());
        assert!(!fixture.path(".a").exists());
        assert_eq!(saved(), before, "the saved setup is unchanged");
        let again = fixture
            .apply_with(json!({ "operation_id": "op-2", "agents": ["agent_b"] }))
            .unwrap();
        assert_eq!(again["outcome"], "verified_no_change");
        let next = again["next_action"].as_str().unwrap();
        assert!(next.contains("agents named for this run"), "{next}");
        let reused = fixture.apply("op-1").unwrap_err();
        assert!(
            reused.message.contains("different request"),
            "{}",
            reused.message
        );
    }

    #[test]
    fn named_agents_must_be_known_and_never_mean_every_agent() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let before = destinations(&fixture);

        for agents in [json!([]), json!(["not_an_agent"])] {
            let error = fixture
                .apply_with(json!({ "operation_id": "op-1", "agents": agents }))
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidInput, "{}", error.message);
        }

        assert_eq!(destinations(&fixture), before);
    }

    #[test]
    fn more_than_500_items_are_refused() {
        let fixture = Fixture::new();
        let items: Vec<Value> = (0..501)
            .map(|n| json!({ "kind": "skill", "id": format!("s{n}"), "agents": ["agent_a"] }))
            .collect();
        let decisions: Vec<Value> = items
            .iter()
            .map(|item| {
                json!({ "kind": "skill", "id": item["id"], "agents": ["agent_a"],
                "reason": "first_setup", "name": null, "proposed": item,
                "choices": ["approve", "skip"], "choice": "approve" })
            })
            .collect();
        let confirm = json!({ "checkout_path": fixture.checkout, "project_id": "p1",
            "register": false, "deploy_mode": "link", "setup_version": 0,
            "decisions": decisions });

        let errors = [
            fixture.apply_with(json!({ "operation_id": "op-1", "items": items })),
            fixture.apply_with(json!({ "operation_id": "op-2", "confirm": confirm })),
            super::super::save_setup(
                &fixture.ctx.store,
                "p1",
                0,
                serde_json::from_value(json!(items)).unwrap(),
            )
            .map(|_| Value::Null),
        ];

        for error in errors {
            let error = error.unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidInput);
            assert!(error.message.contains("500"), "{}", error.message);
        }
    }

    #[test]
    fn an_unsupported_agent_is_reported_per_item_and_not_listed_as_affected() {
        let fixture = Fixture::new();
        let bundle = fixture.instruction(json!({ "CLAUDE.md": "Be brief.\n" }));
        let mcp = fixture.mcp("docs");
        fixture.save_setup(json!([
            { "kind": "instruction", "id": bundle, "agents": ["claude_code", "agent_a"] },
            { "kind": "mcp", "id": mcp["id"], "agents": ["agent_a", "claude_code"],
              "approved": approved(&mcp) },
        ]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(result["outcome"], "verified_changed", "{result}");
        assert_eq!(result["agents"], json!(["claude_code"]));
        for kind in ["instruction", "mcp"] {
            assert_eq!(
                item(&result, kind)["agents"],
                json!(["claude_code"]),
                "{kind}"
            );
            assert_eq!(
                item(&result, kind)["unsupported"],
                json!(["agent_a"]),
                "{kind}"
            );
        }
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
        stale["claude_code"]["server"]["args"] = json!(["-y", "older"]);
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": stale },
        ]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "mcp")["status"], "needs_review");
        assert_eq!(item(&result, "mcp")["reason"], "value_changed");
        assert_eq!(result["outcome"], "needs_review");
        assert!(!fixture.path(".mcp.json").exists());
    }

    #[test]
    fn an_mcp_override_for_one_agent_needs_its_effective_value_approved() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        let target = json!({ "agentKey": "claude_code", "projectId": "p1" });
        let preview = fixture.request(
            "mcps_request",
            json!({ "action": "preview", "target": target, "operations": [{
                "kind": "deploy", "definitionId": mcp["id"],
                "overrideConfig": { "server": { "args": ["-y", "docs", "--beta"] } } }] }),
        );
        fixture.request(
            "mcps_request",
            json!({ "action": "apply", "previewId": preview["previewId"] }),
        );
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": approved(&mcp) },
        ]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "mcp")["reason"], "value_changed", "{result}");
        let proposed = &decision(&result, "mcp")["proposed"];
        assert_eq!(
            proposed["claude_code"]["server"]["args"],
            json!(["-y", "docs", "--beta"]),
            "{proposed}"
        );
        let applied = confirm_with(
            &fixture,
            &fixture.checkout,
            "op-2",
            confirm(&result, "approve"),
        )
        .unwrap();
        assert_eq!(applied["outcome"], "verified_no_change", "{applied}");
    }

    #[test]
    fn a_failed_write_with_nothing_changed_is_a_retryable_failure() {
        let fixture = Fixture::new();
        fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        // A file where the agent's folder belongs makes the write fail.
        std::fs::write(fixture.path(".a"), "not a folder").unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(result["outcome"], "retryable_failure", "{result}");
        assert_eq!(item(&result, "skill")["status"], "failed");
        assert_eq!(item(&result, "skill")["retryable"], true);
        let next = result["next_action"].as_str().unwrap();
        assert!(next.contains("new operation_id"), "{next}");
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
    fn an_mcp_recovery_for_another_project_blocks_instead_of_failing() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let recovery = json!({ "id": "r1",
            "target": { "agentKey": "claude_code", "projectId": "p2" } });
        fixture
            .ctx
            .store
            .resource_state_put("mcps_recovery", "r1", &recovery)
            .unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        let mcp = item(&result, "mcp");
        assert_eq!(mcp["status"], "blocked", "{result}");
        assert_eq!(mcp["reason"], "pending_recovery");
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
        assert_eq!(result["outcome"], "partial_success");
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

        let error = fixture
            .apply_with(json!({ "operation_id": "op-1", "agents": ["claude_code"] }))
            .unwrap_err();
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
    fn apply_prunes_settled_receipts_older_than_30_days_but_never_a_claim() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let store = &fixture.ctx.store;
        let day = 24 * 60 * 60 * 1000;
        let now = chrono::Utc::now().timestamp_millis();
        let intent = json!({ "host": "h", "checkout_path": fixture.checkout });
        let planted = [
            (
                "op-old",
                json!({ "version": 2, "intent": intent, "result": {},
                               "settled_at": now - 31 * day }),
            ),
            (
                "op-recent",
                json!({ "version": 2, "intent": intent, "result": {},
                                  "settled_at": now - 29 * day }),
            ),
            ("op-claimed", json!({ "version": 1, "intent": intent })),
        ];
        for (id, receipt) in &planted {
            store
                .resource_state_put("project_setup_receipt", id, receipt)
                .unwrap();
        }

        fixture.apply("op-1").unwrap();

        let kept = |id| {
            store
                .resource_state_get("project_setup_receipt", id)
                .unwrap()
                .is_some()
        };
        assert!(!kept("op-old"));
        assert!(kept("op-recent") && kept("op-claimed") && kept("op-1"));
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

    fn decision<'a>(result: &'a Value, kind: &str) -> &'a Value {
        result["proposal"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|decision| decision["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} decision in {result}"))
    }

    /// Answer every decision in `result`'s proposal with `choice`.
    fn confirm(result: &Value, choice: &str) -> Value {
        let mut proposal = result["proposal"].clone();
        for decision in proposal["decisions"].as_array_mut().unwrap() {
            decision["choice"] = json!(choice);
        }
        proposal
    }

    /// A Skill for agent_a and an MCP connection for Claude Code, proposed for
    /// the unregistered checkout `work/fresh`.
    fn propose_first_setup(fixture: &Fixture) -> (PathBuf, Value, Value) {
        fixture.skill("s1", "review");
        let mcp = fixture.mcp("docs");
        let fresh = fixture.repo._tmp.path().join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let fresh = fresh.canonicalize().unwrap();
        let proposed = fixture
            .apply_with(
                json!({ "checkout_path": fresh, "operation_id": "op-1", "items": [
                { "kind": "skill", "id": "s1", "agents": ["agent_a"] },
                { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"] },
            ] }),
            )
            .unwrap();
        (fresh, mcp, proposed)
    }

    #[test]
    fn a_first_setup_is_proposed_without_writing_anything() {
        let fixture = Fixture::new();
        let (fresh, _, proposed) = propose_first_setup(&fixture);

        assert_eq!(proposed["outcome"], "confirmation_needed");
        assert_eq!(proposed["proposal"]["register"], true);
        assert_eq!(proposed["proposal"]["checkout_path"], json!(fresh));
        let skill = decision(&proposed, "skill");
        assert_eq!(skill["name"], "review");
        assert_eq!(skill["agents"], json!(["agent_a"]));
        assert_eq!(skill["choices"], json!(["approve", "skip"]));
        let server = &decision(&proposed, "mcp")["proposed"]["approved"]["claude_code"]["server"];
        assert_eq!(server["command"], "npx");
        assert_eq!(server["args"], json!(["-y", "docs"]));
        assert_eq!(fixture.ctx.store.get_all_projects().unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(&fresh).unwrap().count(), 0);
        let name = fixture.ctx.store.get_setting("backup_device_name").unwrap();
        assert_eq!(name, None, "proposing saved a host name");
    }

    fn confirm_with(
        fixture: &Fixture,
        checkout: &Path,
        operation_id: &str,
        confirm: Value,
    ) -> Result<Value, AppError> {
        fixture.apply_with(
            json!({ "checkout_path": checkout, "operation_id": operation_id,
                                    "confirm": confirm, "evidence": true }),
        )
    }

    fn change_args(fixture: &Fixture, mcp: &Value, args: Value) {
        fixture.request(
            "mcps_request",
            json!({ "action": "save", "expectedRevision": mcp["revision"], "definition": {
                "id": mcp["id"], "name": mcp["name"], "transport": "stdio",
                "server": { "command": "npx", "args": args } } }),
        );
    }

    #[test]
    fn a_confirmed_first_setup_registers_saves_applies_and_verifies_in_one_call() {
        let fixture = Fixture::new();
        let (fresh, _, proposed) = propose_first_setup(&fixture);

        let applied =
            confirm_with(&fixture, &fresh, "op-2", confirm(&proposed, "approve")).unwrap();

        assert_eq!(applied["outcome"], "verified_changed", "{applied}");
        assert_eq!(applied["counts"]["changed"], 2);
        let found = fixture.request(
            "project_setup_request",
            json!({ "action": "discover", "checkout_path": fresh }),
        );
        assert_eq!(found["project"]["id"], applied["project_id"]);
        assert_eq!(found["saved_setup"]["version"], 1);
        assert_eq!(applied["setup_version"], 1);
        assert!(fresh.join(".a/skills/review").exists());
        let mcp = std::fs::read_to_string(fresh.join(".mcp.json")).unwrap();
        assert!(mcp.contains("\"docs\""), "{mcp}");

        // The approved, unchanged connection applies later without a prompt.
        let later = fixture
            .apply_with(json!({ "checkout_path": fresh, "operation_id": "op-3" }))
            .unwrap();
        assert_eq!(later["outcome"], "verified_no_change", "{later}");
        assert!(later.get("proposal").is_none());
    }

    #[test]
    fn a_first_setup_reports_agents_that_cannot_take_an_item_here() {
        let fixture = Fixture::new();
        let tmp = fixture.repo._tmp.path();
        // agent_d's own folder does not exist: it is not installed here.
        let tools = json!([
            { "key": "agent_a", "display_name": "Agent A", "skills_dir": tmp.join("agents/a/skills"),
              "project_relative_skills_dir": ".a/skills" },
            { "key": "agent_d", "display_name": "Agent D", "skills_dir": tmp.join("agents/d/skills"),
              "project_relative_skills_dir": ".d/skills" },
        ]);
        fixture
            .ctx
            .store
            .set_setting("custom_tools", &tools.to_string())
            .unwrap();
        fixture.skill("s1", "review");
        let fresh = tmp.join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let propose = |agents: Value| {
            fixture.apply_with(json!({ "checkout_path": fresh, "operation_id": "op-1",
                "items": [{ "kind": "skill", "id": "s1", "agents": agents }] }))
        };

        let proposed = propose(json!(["agent_a", "agent_d"])).unwrap();
        assert_eq!(
            decision(&proposed, "skill")["unsupported"],
            json!(["agent_d"]),
            "{proposed}"
        );

        let refused = propose(json!(["agent_d"])).unwrap_err();
        assert!(refused.message.contains("agent_d"), "{}", refused.message);
        assert_eq!(fixture.ctx.store.get_all_projects().unwrap().len(), 1);
    }

    #[test]
    fn a_first_setup_names_the_shared_folder_that_adds_its_readers() {
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
        let fresh = fixture.repo._tmp.path().join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();

        let proposed = fixture
            .apply_with(
                json!({ "checkout_path": fresh, "operation_id": "op-1", "items": [
                { "kind": "skill", "id": "s1", "agents": ["agent_a"] } ] }),
            )
            .unwrap();

        let skill = decision(&proposed, "skill");
        assert_eq!(skill["proposed"]["agents"], json!(["agent_a", "agent_c"]));
        assert_eq!(skill["paths"], json!([".a/skills"]), "{proposed}");
        assert_eq!(proposed["agents"], json!(["agent_a", "agent_c"]));
    }

    #[test]
    fn a_saved_setup_refuses_a_decision_forged_as_a_first_setup() {
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
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let asked = fixture.apply("op-1").unwrap();
        let mut forged = confirm(&asked, "approve");
        let decision = &mut forged["decisions"][0];
        decision["reason"] = json!("first_setup");
        decision["proposed"] =
            json!({ "kind": "skill", "id": "s1", "agents": ["agent_a", "agent_c"] });

        let refused = fixture
            .apply_with(json!({ "operation_id": "op-2", "confirm": forged }))
            .unwrap_err();

        assert!(
            refused.message.contains("first_setup"),
            "{}",
            refused.message
        );
        let saved = fixture
            .ctx
            .store
            .resource_state_get("project_setup", "p1")
            .unwrap()
            .unwrap();
        assert!(saved["items"][0].get("approved").is_none(), "{saved}");
    }

    #[test]
    fn a_first_setup_proposed_on_one_host_cannot_be_confirmed_on_another() {
        let fixture = Fixture::new();
        let (fresh, _, proposed) = propose_first_setup(&fixture);
        fixture
            .ctx
            .store
            .set_setting("backup_device_name", "other-host")
            .unwrap();

        let error =
            confirm_with(&fixture, &fresh, "op-2", confirm(&proposed, "approve")).unwrap_err();

        assert!(error.message.contains("stale"), "{}", error.message);
        assert_eq!(fixture.ctx.store.get_all_projects().unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(&fresh).unwrap().count(), 0);
    }

    #[test]
    fn a_confirmation_is_stale_when_mcp_arguments_changed_since_the_proposal() {
        let fixture = Fixture::new();
        let (fresh, mcp, proposed) = propose_first_setup(&fixture);
        change_args(&fixture, &mcp, json!(["-y", "docs@2"]));

        let error =
            confirm_with(&fixture, &fresh, "op-2", confirm(&proposed, "approve")).unwrap_err();

        assert!(error.message.contains("stale"), "{}", error.message);
        assert_eq!(fixture.ctx.store.get_all_projects().unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(&fresh).unwrap().count(), 0);
    }

    #[test]
    fn a_changed_mcp_value_is_proposed_and_skipping_it_keeps_the_saved_selection() {
        let fixture = Fixture::new();
        let (_, mcp) = one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        change_args(&fixture, &mcp, json!(["-y", "docs@2"]));

        let proposed = fixture.apply("op-2").unwrap();
        let change = decision(&proposed, "mcp");
        assert_eq!(change["reason"], "value_changed");
        assert_eq!(
            change["proposed"]["claude_code"]["server"]["args"],
            json!(["-y", "docs@2"])
        );
        assert_eq!(change["choices"], json!(["approve", "skip"]));
        let saved = || {
            fixture
                .ctx
                .store
                .resource_state_get("project_setup", "p1")
                .unwrap()
                .unwrap()
        };
        let before = saved();

        let skipped = confirm_with(
            &fixture,
            &fixture.checkout,
            "op-3",
            confirm(&proposed, "skip"),
        )
        .unwrap();
        assert_eq!(item(&skipped, "mcp")["reason"], "skipped");
        assert_eq!(saved(), before);
        assert_eq!(mcp_entry(&fixture, "docs")["args"], json!(["-y", "docs"]));

        let approved = confirm_with(
            &fixture,
            &fixture.checkout,
            "op-4",
            confirm(&proposed, "approve"),
        )
        .unwrap();
        assert_eq!(item(&approved, "mcp")["status"], "changed", "{approved}");
        assert_eq!(mcp_entry(&fixture, "docs")["args"], json!(["-y", "docs@2"]));
        assert_eq!(
            fixture.apply("op-5").unwrap()["outcome"],
            "verified_no_change"
        );
    }

    #[test]
    fn a_confirmed_missing_item_offers_restore_replace_or_revise_while_others_apply() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.ctx.store.delete_skill("s1").unwrap();

        let proposed = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        let gone = decision(&proposed, "skill");
        assert_eq!(gone["reason"], "confirmed_absent");
        assert_eq!(gone["choices"], json!(["restore", "replace", "revise"]));
        assert_eq!(item(&proposed, "instruction")["status"], "changed");
        assert_eq!(item(&proposed, "mcp")["status"], "changed");

        let other = fixture.skill("s2", "lint");
        let mut replace = confirm(&proposed, "replace");
        replace["decisions"][0]["replacement"] = json!("s2");
        let replaced = confirm_with(&fixture, &fixture.checkout, "op-2", replace).unwrap();
        assert_eq!(item(&replaced, "skill")["status"], "changed", "{replaced}");
        assert!(resolves_to(&fixture.path(".a/skills/lint"), &other));

        fixture.ctx.store.delete_skill("s2").unwrap();
        let proposed = fixture.apply("op-3").unwrap();
        let revised = confirm_with(
            &fixture,
            &fixture.checkout,
            "op-4",
            confirm(&proposed, "revise"),
        )
        .unwrap();
        assert_eq!(revised["outcome"], "verified_no_change", "{revised}");
        let saved = fixture
            .ctx
            .store
            .resource_state_get("project_setup", "p1")
            .unwrap()
            .unwrap();
        assert_eq!(saved["items"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn an_approved_instruction_deletion_and_added_agents_apply() {
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
        std::fs::create_dir_all(tmp.join("agents/c")).unwrap();
        fixture.skill("s1", "review");
        let saved = fixture.request(
            "instructions_request",
            json!({ "action": "save", "name": "Rules",
                    "files": { "CLAUDE.md": "a\n", ".claude/rules/extra.md": "b\n" } }),
        );
        let id = saved["item"]["id"].as_str().unwrap();
        fixture.save_setup(json!([
            { "kind": "instruction", "id": id, "agents": ["claude_code"] },
            { "kind": "skill", "id": "s1", "agents": ["agent_a"] },
        ]));
        fixture.apply("op-1").unwrap();
        fixture.request(
            "instructions_request",
            json!({ "action": "save", "id": id, "name": "Rules", "files": { "CLAUDE.md": "a\n" },
                    "expected_revision": saved["item"]["revision"] }),
        );

        let proposed = fixture.apply("op-2").unwrap();
        assert_eq!(
            decision(&proposed, "instruction")["proposed"],
            json!(["claude_code:.claude/rules/extra.md"])
        );
        assert_eq!(
            decision(&proposed, "skill")["proposed"],
            json!(["agent_a", "agent_c"])
        );
        let applied = confirm_with(
            &fixture,
            &fixture.checkout,
            "op-3",
            confirm(&proposed, "approve"),
        )
        .unwrap();

        assert_eq!(applied["outcome"], "verified_changed", "{applied}");
        assert!(!fixture.path(".claude/rules/extra.md").exists());
        assert!(fixture.path(".a/skills/review").exists());
    }

    #[test]
    fn a_hand_edited_managed_mcp_entry_needs_review_instead_of_no_change() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        let path = fixture.path(".mcp.json");
        let mut config: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        config["mcpServers"]["docs"]["args"] = json!(["-y", "docs", "--verbose"]);
        std::fs::write(&path, config.to_string()).unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-2", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "mcp")["status"], "needs_review", "{result}");
        assert_eq!(item(&result, "mcp")["reason"], "edited_here");
        assert_eq!(
            mcp_entry(&fixture, "docs")["args"],
            json!(["-y", "docs", "--verbose"])
        );
    }

    #[test]
    fn a_deleted_instruction_file_needs_review_instead_of_no_change() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        std::fs::remove_file(fixture.path("CLAUDE.md")).unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-2", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "instruction")["status"], "needs_review");
        assert_eq!(item(&result, "instruction")["reason"], "edited_here");
        assert_eq!(result["outcome"], "needs_review");
    }

    #[test]
    fn a_confirmation_replayed_with_its_operation_id_returns_the_recorded_result() {
        let fixture = Fixture::new();
        let (fresh, _, proposed) = propose_first_setup(&fixture);
        let confirmation = confirm(&proposed, "approve");
        let first = confirm_with(&fixture, &fresh, "op-2", confirmation.clone()).unwrap();

        let again = confirm_with(&fixture, &fresh, "op-2", confirmation).unwrap();

        assert_eq!(again["from_receipt"], true, "{again}");
        assert_eq!(again["outcome"], first["outcome"]);
        assert_eq!(again["project_id"], first["project_id"]);
    }

    #[test]
    fn one_agent_changed_and_another_blocked_is_a_partial_success() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code", "codex"],
              "approved": approved(&mcp) },
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

        let docs = &result["items"][0];
        assert_eq!(result["outcome"], "partial_success", "{result}");
        let targets = docs["targets"].as_array().unwrap();
        let status = |agent: &str| {
            targets
                .iter()
                .find(|t| t["agent"] == agent)
                .map(|t| t["status"].clone())
        };
        assert_eq!(status("claude_code"), Some(json!("needs_review")), "{docs}");
        assert_eq!(status("codex"), Some(json!("changed")), "{docs}");
        assert_eq!(mcp_entry(&fixture, "docs"), own);
    }

    #[test]
    fn an_operation_id_of_128_characters_is_accepted_whatever_its_bytes() {
        let fixture = Fixture::new();
        one_of_each(&fixture);

        let accepted = fixture.apply(&"é".repeat(128));
        let refused = fixture.apply(&"é".repeat(129)).unwrap_err();

        assert!(accepted.is_ok(), "{accepted:?}");
        assert!(
            refused.message.contains("characters"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn a_recorded_error_replays_with_its_own_kind() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        let store = &fixture.ctx.store;
        let mut receipt = store
            .resource_state_get("project_setup_receipt", "op-1")
            .unwrap()
            .unwrap();
        let receipt = receipt.as_object_mut().unwrap();
        receipt.remove("result");
        receipt.insert(
            "error".into(),
            json!(AppError::target_conflict("taken", Vec::new())),
        );
        store
            .resource_state_put("project_setup_receipt", "op-1", &json!(receipt))
            .unwrap();

        let error = fixture.apply("op-1").unwrap_err();

        assert_eq!(error.kind, ErrorKind::TargetConflict, "{}", error.message);
        assert_eq!(error.message, "taken");
    }

    #[cfg(unix)]
    #[test]
    fn a_skill_linked_to_another_folder_with_the_same_content_needs_review() {
        let fixture = Fixture::new();
        let library = fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let elsewhere = fixture.repo._tmp.path().join("elsewhere/review");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::copy(library.join("SKILL.md"), elsewhere.join("SKILL.md")).unwrap();
        std::fs::create_dir_all(fixture.path(".a/skills")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, fixture.path(".a/skills/review")).unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "skill")["status"], "needs_review", "{result}");
        assert_eq!(item(&result, "skill")["reason"], "edited_here");
        assert!(resolves_to(&fixture.path(".a/skills/review"), &elsewhere));
    }

    #[test]
    fn an_instruction_written_to_a_shared_file_needs_its_other_readers_approved() {
        let fixture = Fixture::new();
        let bundle = fixture.instruction(json!({ "AGENTS.md": "Be brief.\n" }));
        fixture.save_setup(json!([
            { "kind": "instruction", "id": bundle, "agents": ["claude_code"] },
        ]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(
            item(&result, "instruction")["reason"],
            "unapproved_agents",
            "{result}"
        );
        assert!(!fixture.path("AGENTS.md").exists());
        let readers = decision(&result, "instruction")["proposed"].clone();
        assert!(
            readers.as_array().unwrap().contains(&json!("codex")),
            "{readers}"
        );
        assert_eq!(
            decision(&result, "instruction")["paths"],
            json!(["AGENTS.md"])
        );
        let applied = confirm_with(
            &fixture,
            &fixture.checkout,
            "op-2",
            confirm(&result, "approve"),
        )
        .unwrap();
        assert_eq!(applied["outcome"], "verified_changed", "{applied}");
    }

    #[test]
    fn an_unreadable_mcp_definition_does_not_block_a_skill() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let definitions = crate::core::resource_store::root().join("mcps");
        let file = definitions.join(format!("{}.json", mcp["id"].as_str().unwrap()));
        assert!(file.is_file(), "{file:?}");
        std::fs::write(&file, "not json").unwrap();

        let result = fixture.apply("op-1").unwrap();

        assert_eq!(result["outcome"], "verified_changed", "{result}");
    }

    #[test]
    fn more_than_64_agents_are_refused() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let agents = vec!["agent_a"; 65];

        let named = fixture
            .apply_with(json!({ "operation_id": "op-1", "agents": agents }))
            .unwrap_err();
        let fresh = fixture.repo._tmp.path().join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        let proposed = fixture
            .apply_with(json!({ "checkout_path": fresh, "operation_id": "op-2",
                "items": [{ "kind": "skill", "id": "s1", "agents": agents }] }))
            .unwrap_err();

        for error in [named, proposed] {
            assert_eq!(error.kind, ErrorKind::InvalidInput);
            assert!(error.message.contains("64"), "{}", error.message);
        }
    }

    #[test]
    fn a_deletion_runs_only_when_exactly_those_files_were_approved() {
        use super::super::items::{check_items, Planned, Run};
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
        let project = fixture.ctx.store.get_project_by_id("p1").unwrap().unwrap();
        let run = Run::new(&fixture.ctx, project, true).unwrap();
        let planned = |approved: &[&str]| Planned {
            deletes_approved: approved.iter().map(|file| file.to_string()).collect(),
            ..Planned::new(
                serde_json::from_value(
                    json!({ "kind": "instruction", "id": id, "agents": ["claude_code"] }),
                )
                .unwrap(),
            )
        };

        let other = check_items(&run, &[planned(&["claude_code:.claude/rules/other.md"])]);
        assert_eq!(other[0].reason.as_deref(), Some("would_delete"));
        assert!(fixture.path(".claude/rules/extra.md").exists());

        let exact = check_items(&run, &[planned(&["claude_code:.claude/rules/extra.md"])]);
        assert_eq!(json!(exact[0].status), "changed");
        assert!(!fixture.path(".claude/rules/extra.md").exists());
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

    #[cfg(unix)]
    #[test]
    fn a_skill_folder_linked_outside_the_checkout_is_blocked_and_never_written() {
        let fixture = Fixture::new();
        fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        let shared = fixture.repo._tmp.path().join("shared/skills");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(fixture.path(".a")).unwrap();
        std::os::unix::fs::symlink(&shared, fixture.path(".a/skills")).unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "skill")["status"], "blocked", "{result}");
        assert_eq!(item(&result, "skill")["reason"], "outside_checkout");
        assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 0);
    }

    /// Each agent's status in an item's `targets`.
    fn target_status(item: &Value, agent: &str) -> Value {
        item["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["agent"] == agent)
            .map_or(Value::Null, |t| t["status"].clone())
    }

    #[test]
    fn a_skill_written_for_one_agent_before_another_fails_is_a_partial_success() {
        let fixture = Fixture::new();
        with_agent_b(&fixture);
        fixture.skill("s1", "review");
        fixture.save_setup(json!([
            { "kind": "skill", "id": "s1", "agents": ["agent_a", "agent_b"] },
        ]));
        std::fs::write(fixture.path(".b"), "not a folder").unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        let skill = item(&result, "skill");
        assert_eq!(result["outcome"], "partial_success", "{result}");
        assert_eq!(target_status(skill, "agent_a"), "changed", "{skill}");
        assert_eq!(target_status(skill, "agent_b"), "failed", "{skill}");
        assert!(fixture.path(".a/skills/review").exists());
    }

    #[test]
    fn a_vendored_skill_whose_agent_link_fails_is_a_partial_success() {
        let fixture = Fixture::new();
        let mut project = fixture.ctx.store.get_project_by_id("p1").unwrap().unwrap();
        project.deploy_mode = "copy".into();
        fixture.ctx.store.delete_project("p1").unwrap();
        fixture.ctx.store.insert_project(&project).unwrap();
        fixture.skill("s1", "review");
        let mut agents = vec!["agent_a".to_string()];
        for (config, members) in crate::commands::projects::agent_skill_groups(&fixture.ctx.store) {
            if crate::core::project_deploy::is_vendored_dir(&config.relative_skills_dir) {
                agents.extend(members);
            }
        }
        let reader = agents[1].clone();
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": agents }]));
        std::fs::write(fixture.path(".a"), "not a folder").unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        let skill = item(&result, "skill");
        assert_eq!(result["outcome"], "partial_success", "{result}");
        assert_eq!(target_status(skill, "agent_a"), "failed", "{skill}");
        assert_eq!(target_status(skill, &reader), "changed", "{skill}");
        assert!(fixture.path(".agents/skills/review/SKILL.md").is_file());
    }

    #[test]
    fn an_instruction_written_for_one_agent_before_another_fails_is_a_partial_success() {
        let fixture = Fixture::new();
        // Codex reads none of the bundle's files, so its part fails after
        // Claude Code's was written.
        let bundle = fixture.instruction(json!({ "CLAUDE.md": "Be brief.\n" }));
        fixture.save_setup(json!([
            { "kind": "instruction", "id": bundle, "agents": ["claude_code", "codex"] },
        ]));

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        let instruction = item(&result, "instruction");
        assert_eq!(result["outcome"], "partial_success", "{result}");
        assert_eq!(
            target_status(instruction, "claude_code"),
            "changed",
            "{instruction}"
        );
        assert_eq!(
            target_status(instruction, "codex"),
            "failed",
            "{instruction}"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.path("CLAUDE.md")).unwrap(),
            "Be brief.\n"
        );
    }

    #[test]
    fn a_run_for_a_newly_named_agent_proposes_its_approval_for_that_run_only() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"],
              "approved": { "claude_code": approved(&mcp)["claude_code"] } },
        ]));
        let saved = || {
            fixture
                .ctx
                .store
                .resource_state_get("project_setup", "p1")
                .unwrap()
        };
        let before = saved();

        let asked = fixture
            .apply_with(json!({ "operation_id": "op-1", "agents": ["codex"] }))
            .unwrap();
        assert_eq!(asked["outcome"], "needs_review", "{asked}");
        assert_eq!(
            decision(&asked, "mcp")["reason"],
            "approval_needed",
            "{asked}"
        );
        let applied = fixture
            .apply_with(json!({ "operation_id": "op-2", "agents": ["codex"],
                                "confirm": confirm(&asked, "approve") }))
            .unwrap();

        assert_eq!(applied["outcome"], "verified_changed", "{applied}");
        assert_eq!(applied["agents"], json!(["codex"]));
        assert_eq!(saved(), before, "the saved setup is unchanged");
    }

    #[test]
    fn a_first_setup_with_no_agents_never_expands_to_a_shared_files_readers() {
        let fixture = Fixture::new();
        let bundle = fixture.instruction(json!({ "AGENTS.md": "Be brief.\n" }));
        let fresh = fixture.repo._tmp.path().join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();

        let proposed = fixture
            .apply_with(
                json!({ "checkout_path": fresh, "operation_id": "op-1", "items": [
                { "kind": "instruction", "id": bundle, "agents": [] },
            ] }),
            )
            .unwrap();

        let instruction = decision(&proposed, "instruction");
        assert_eq!(
            instruction["proposed"]["agents"],
            json!([]),
            "{instruction}"
        );
    }

    #[test]
    fn a_skill_folder_shared_with_another_agent_is_named_in_its_decision() {
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
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));

        let result = fixture.apply("op-1").unwrap();

        let skill = decision(&result, "skill");
        assert_eq!(skill["proposed"], json!(["agent_a", "agent_c"]), "{skill}");
        assert_eq!(skill["paths"], json!([".a/skills"]), "{skill}");
    }

    #[test]
    fn an_mcp_connection_the_library_cannot_read_is_missing_and_kept() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": approved(&mcp) },
        ]));
        let saved = fixture
            .ctx
            .store
            .resource_state_get("project_setup", "p1")
            .unwrap();
        let definitions = crate::core::resource_store::root().join("mcps");
        let file = definitions.join(format!("{}.json", mcp["id"].as_str().unwrap()));
        std::fs::write(&file, "not json").unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "mcp")["status"], "missing", "{result}");
        assert_eq!(item(&result, "mcp")["reason"], "unavailable");
        assert!(result.get("proposal").is_none(), "{result}");
        let after = fixture
            .ctx
            .store
            .resource_state_get("project_setup", "p1")
            .unwrap();
        assert_eq!(after, saved, "the selection is kept");
    }

    #[test]
    #[cfg(unix)]
    fn a_first_setup_whose_agent_folder_links_outside_the_checkout_registers_nothing() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        let tmp = fixture.repo._tmp.path();
        // The agent folder itself, its skills folder, or a dangling skills link.
        for (n, link, target_exists) in [
            (1, ".claude", true),
            (2, ".claude/skills", true),
            (3, ".claude/skills", false),
        ] {
            let fresh = tmp.join(format!("work/fresh{n}"));
            std::fs::create_dir_all(fresh.join(".claude")).unwrap();
            if link == ".claude" {
                std::fs::remove_dir(fresh.join(".claude")).unwrap();
            }
            let fresh = fresh.canonicalize().unwrap();
            let shared = tmp.join(format!("shared/claude{n}"));
            if target_exists {
                std::fs::create_dir_all(&shared).unwrap();
            }
            std::os::unix::fs::symlink(&shared, fresh.join(link)).unwrap();
            let proposed = fixture
                .apply_with(
                    json!({ "checkout_path": fresh, "operation_id": format!("op-{n}a"),
                    "items": [{ "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"] }] }),
                )
                .unwrap();

            let error = confirm_with(
                &fixture,
                &fresh,
                &format!("op-{n}b"),
                confirm(&proposed, "approve"),
            )
            .unwrap_err();

            assert!(
                error.message.contains("outside the checkout"),
                "{link}: {}",
                error.message
            );
            assert!(!shared.exists() || std::fs::read_dir(&shared).unwrap().count() == 0);
            assert_eq!(fixture.ctx.store.get_all_projects().unwrap().len(), 1);
            assert!(!fresh.join(".mcp.json").exists());
        }
    }

    #[test]
    fn agents_for_one_run_are_refused_without_a_saved_setup() {
        let fixture = Fixture::new();
        let (fresh, _, proposed) = propose_first_setup(&fixture);
        let refused = fixture.apply_with(json!({ "checkout_path": fresh, "operation_id": "op-2",
            "confirm": confirm(&proposed, "approve"), "agents": ["agent_a"] }));
        assert_eq!(refused.unwrap_err().kind, ErrorKind::InvalidInput);
        assert_eq!(std::fs::read_dir(&fresh).unwrap().count(), 0);

        // A registered checkout with no saved setup.
        let mcp = fixture.mcp("lint");
        let proposed = fixture
            .apply_with(json!({ "operation_id": "op-3",
                "items": [{ "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"] }] }))
            .unwrap();
        let refused = fixture.apply_with(json!({ "operation_id": "op-4",
            "confirm": confirm(&proposed, "approve"), "agents": ["claude_code"] }));
        assert_eq!(refused.unwrap_err().kind, ErrorKind::InvalidInput);
        assert!(!fixture.path(".mcp.json").exists());
    }

    #[test]
    fn a_deleted_instruction_bundle_offers_restore_replace_or_revise() {
        let fixture = Fixture::new();
        let bundle = fixture.instruction(json!({ "CLAUDE.md": "Be brief.\n" }));
        fixture.request(
            "instructions_request",
            json!({ "action": "remove", "id": bundle }),
        );
        fixture.save_setup(
            json!([{ "kind": "instruction", "id": bundle, "agents": ["claude_code"] }]),
        );

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(
            item(&result, "instruction")["status"],
            "missing",
            "{result}"
        );
        let gone = decision(&result, "instruction");
        assert_eq!(gone["reason"], "confirmed_absent");
        assert_eq!(gone["choices"], json!(["restore", "replace", "revise"]));
    }

    #[test]
    fn a_merge_of_a_local_edit_and_a_library_edit_verifies_and_then_stays_in_place() {
        let fixture = Fixture::new();
        let saved = fixture.request(
            "instructions_request",
            json!({ "action": "save", "name": "Rules",
                    "files": { "CLAUDE.md": "alpha\nmiddle\nbeta\n" } }),
        );
        let id = saved["item"]["id"].as_str().unwrap();
        fixture.save_setup(json!([{ "kind": "instruction", "id": id, "agents": ["claude_code"] }]));
        fixture.apply("op-1").unwrap();
        std::fs::write(fixture.path("CLAUDE.md"), "local alpha\nmiddle\nbeta\n").unwrap();
        fixture.request(
            "instructions_request",
            json!({ "action": "save", "id": id, "name": "Rules",
                    "files": { "CLAUDE.md": "alpha\nmiddle\nremote beta\n" },
                    "expected_revision": saved["item"]["revision"] }),
        );

        let result = fixture
            .apply_with(json!({ "operation_id": "op-2", "evidence": true }))
            .unwrap();

        assert_eq!(
            item(&result, "instruction")["status"],
            "changed",
            "{result}"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.path("CLAUDE.md")).unwrap(),
            "local alpha\nmiddle\nremote beta\n"
        );

        // The merge written is in place: no change, not an edit here.
        let later = fixture
            .apply_with(json!({ "operation_id": "op-3", "evidence": true }))
            .unwrap();
        assert_eq!(
            item(&later, "instruction")["status"],
            "unchanged",
            "{later}"
        );

        // An edit made after the merge is still an edit here.
        std::fs::write(
            fixture.path("CLAUDE.md"),
            "local alpha\nedited\nremote beta\n",
        )
        .unwrap();
        let edited = fixture
            .apply_with(json!({ "operation_id": "op-4", "evidence": true }))
            .unwrap();
        assert_eq!(
            item(&edited, "instruction")["reason"],
            "edited_here",
            "{edited}"
        );
    }

    #[test]
    fn a_result_that_cannot_be_recorded_is_outcome_unknown_with_its_operation_id() {
        let fixture = Fixture::new();
        let (library, _) = one_of_each(&fixture);
        let db = rusqlite::Connection::open(fixture.repo._tmp.path().join("repo/test.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER no_result BEFORE UPDATE ON resource_state
             WHEN NEW.kind = 'project_setup_receipt' AND json_extract(NEW.value, '$.version') = 2
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .unwrap();

        let result = fixture.apply("op-1").unwrap();

        assert_eq!(result["outcome"], "outcome_unknown", "{result}");
        assert_eq!(result["operation_id"], "op-1");
        assert!(
            result["next_action"]
                .as_str()
                .unwrap()
                .contains("inspect_project_setup_result"),
            "{result}"
        );
        assert!(resolves_to(&fixture.path(".a/skills/review"), &library));
    }

    #[test]
    fn discovery_skips_an_unreadable_mcp_definition() {
        let fixture = Fixture::new();
        fixture.mcp("docs");
        let definitions = crate::core::resource_store::root().join("mcps");
        std::fs::write(
            definitions.join(format!("{}.json", uuid::Uuid::new_v4())),
            "not json",
        )
        .unwrap();

        let found = dispatch(
            &fixture.ctx,
            "project_setup_request",
            &json!({ "request": { "action": "discover", "checkout_path": fixture.checkout } }),
        )
        .unwrap();

        let names: Vec<&Value> = found["library"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| &item["name"])
            .collect();
        assert_eq!(names, [&json!("docs")], "{found}");
    }

    #[test]
    fn a_first_setup_with_every_decision_skipped_sets_nothing_up() {
        let fixture = Fixture::new();
        let (fresh, _, proposed) = propose_first_setup(&fixture);

        let result = confirm_with(&fixture, &fresh, "op-2", confirm(&proposed, "skip")).unwrap();

        assert!(
            result["next_action"]
                .as_str()
                .unwrap()
                .contains("Nothing was set up"),
            "{result}"
        );
        assert_eq!(result["outcome"], "nothing_set_up");
        assert_eq!(result["project_id"], Value::Null);
        assert_eq!(fixture.ctx.store.get_all_projects().unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(&fresh).unwrap().count(), 0);
    }

    #[test]
    fn discovery_reports_no_sync_check_time_when_this_host_never_synced() {
        let fixture = Fixture::new();

        let found = fixture.request(
            "project_setup_request",
            json!({ "action": "discover", "checkout_path": fixture.checkout }),
        );

        assert_eq!(found["sync"]["checked_at"], Value::Null, "{found}");
        assert_eq!(found["sync"]["verified"], false);
    }

    #[test]
    fn an_unreadable_mcp_definition_does_not_block_another_mcp_connection() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        let other = fixture.mcp("other");
        fixture.save_setup(json!([
            { "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": approved(&mcp) },
        ]));
        let definitions = crate::core::resource_store::root().join("mcps");
        let file = definitions.join(format!("{}.json", other["id"].as_str().unwrap()));
        std::fs::write(&file, "not json").unwrap();

        let result = fixture
            .apply_with(json!({ "operation_id": "op-1", "evidence": true }))
            .unwrap();

        assert_eq!(item(&result, "mcp")["status"], "changed", "{result}");
        assert_eq!(mcp_entry(&fixture, "docs")["command"], "npx");
    }
}
