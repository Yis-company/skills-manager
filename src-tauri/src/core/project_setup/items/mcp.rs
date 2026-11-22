use serde_json::{json, Value};

use super::{missing, Reason, Run, Status, Step};
use crate::core::error::{AppError, ErrorKind};
use crate::core::host::HostCtx;
use crate::core::mcps;
use crate::core::project_setup::{ItemKind, SetupItem};

/// The most MCP connections one preview may change; see `mcps::adapters::preview`.
const MCP_BATCH: usize = 64;

/// The value an MCP connection's approval covers: what it runs and with
/// which credentials references. Compared verbatim, never normalised.
pub fn mcp_value(definition: &Value) -> Value {
    json!({
        "transport": definition["transport"],
        "server": definition["server"],
        "auth": definition.get("auth"),
    })
}

/// What each of the item's agents would run, by agent key: the Library's
/// value with any override that agent's existing deployment applies here.
pub(in crate::core::project_setup) fn effective_values(
    run: &Run,
    item: &SetupItem,
    definition: &Value,
) -> Result<Value, AppError> {
    let mut values = serde_json::Map::new();
    for agent in item
        .agents
        .iter()
        .filter(|a| run.supports(ItemKind::Mcp, a))
    {
        let value = effective_value(definition, run.mcp_override(agent, &item.id)?);
        values.insert(agent.clone(), value);
    }
    Ok(values.into())
}

/// What a deployment of `definition` runs: the Library's value with the
/// deployment's override, if any.
fn effective_value(definition: &Value, override_config: Option<&Value>) -> Value {
    let mut value = mcp_value(definition);
    if let Some(server) = override_config.map(|o| &o["server"]) {
        for (key, field) in server.as_object().into_iter().flatten() {
            value["server"][key] = field.clone();
        }
    }
    value
}

/// Whether `agent`'s approval of each item still covers what it runs now.
/// Read after its preview is stored: that preview pins the definition
/// revision and the deployment it was built from, and apply refuses it if
/// either changes later.
fn still_approved(
    ctx: &HostCtx,
    target: &Value,
    agent: &str,
    items: &[&SetupItem],
) -> Result<bool, AppError> {
    let definitions = mcps::dispatch(ctx, json!({ "action": "list" }))?;
    let deployments = mcps::dispatch(ctx, json!({ "action": "deployments", "target": target }))?;
    let listed = |reply: &Value, key: &str| reply[key].as_array().cloned().unwrap_or_default();
    let (definitions, deployments) = (
        listed(&definitions, "definitions"),
        listed(&deployments, "deployments"),
    );
    Ok(items.iter().all(|item| {
        let Some(definition) = definitions.iter().find(|d| d["id"] == item.id.as_str()) else {
            return false;
        };
        let override_config = deployments
            .iter()
            .find(|d| d["definitionId"] == item.id.as_str())
            .map(|d| &d["overrideConfig"])
            .filter(|o| !o.is_null());
        item.approved.as_ref().and_then(|a| a.get(agent))
            == Some(&effective_value(definition, override_config))
    }))
}

/// The decision the item's approval needs now, if any: every agent's
/// effective value must be the one approved for it.
pub(in crate::core::project_setup) fn approval(
    item: &SetupItem,
    effective: &Value,
) -> Option<Reason> {
    let Some(approved) = &item.approved else {
        return Some(Reason::ApprovalNeeded);
    };
    let values = effective.as_object()?;
    if values
        .iter()
        .any(|(agent, value)| approved.get(agent).is_some_and(|a| a != value))
    {
        return Some(Reason::ValueChanged);
    }
    values
        .keys()
        .any(|agent| approved.get(agent).is_none())
        .then_some(Reason::ApprovalNeeded)
}

/// The agents to set an MCP connection up for, or why none can be.
pub(super) fn check_mcp(run: &Run, item: &SetupItem) -> Result<Vec<String>, Step> {
    let definition = match run.mcp_definition(&item.id) {
        Ok(Some(definition)) => definition,
        Ok(None) => return Err(Step::asks(Status::Missing, Reason::ConfirmedAbsent)),
        // Unreadable now is not gone: a blocker that keeps the selection.
        Err(error) => return Err(missing(error)),
    };
    let agents: Vec<String> = item
        .agents
        .iter()
        .filter(|agent| run.supports(ItemKind::Mcp, agent))
        .cloned()
        .collect();
    if agents.is_empty() {
        return Err(Step::because(Status::Blocked, "no_supported_agent"));
    }
    let effective = effective_values(run, item, definition).map_err(Step::error)?;
    match approval(item, &effective) {
        Some(reason) => Err(Step::asks(Status::NeedsReview, reason)),
        None => Ok(agents),
    }
}

/// Set up one agent's MCP connections: per batch, one preview to check, one
/// to apply and one to verify. Returns a step per connection, in order.
pub(super) fn apply_mcps(run: &Run, agent: &str, batch: &[(usize, &SetupItem)]) -> Vec<Step> {
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
        Err(error) => return vec![Step::error(error); batch.len()],
    }
    let items: Vec<&SetupItem> = batch.iter().map(|(_, item)| *item).collect();
    items
        .chunks(MCP_BATCH)
        .flat_map(|chunk| mcp_chunk(run, agent, &target, chunk))
        .collect()
}

fn mcp_chunk(run: &Run, agent: &str, target: &Value, items: &[&SetupItem]) -> Vec<Step> {
    let (ctx, write) = (run.ctx, run.write);
    let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
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
    let planned = match preview(&ids, true) {
        Ok(planned) => planned,
        // One connection the preview refuses fails the whole batch; find it.
        Err(_) if ids.len() > 1 => {
            return items
                .iter()
                .flat_map(|item| mcp_chunk(run, agent, target, &[item]))
                .collect()
        }
        Err(error) if error.kind == ErrorKind::InvalidInput => {
            return vec![Step::because(Status::NeedsReview, error.message)]
        }
        Err(error) => return vec![Step::error(error)],
    };
    let mut steps: Vec<Option<Step>> = change_kinds(&planned)
        .into_iter()
        .map(|kind| match kind {
            "unchanged" => Some(Step::of(Status::Unchanged)),
            "add" | "update" => None,
            _ => Some(Step::because(Status::NeedsReview, "edited_here")),
        })
        .collect();
    steps.resize(ids.len(), Some(Step::failed("no_preview", false)));
    let eligible: Vec<usize> = (0..ids.len()).filter(|i| steps[*i].is_none()).collect();
    if !write {
        for index in &eligible {
            steps[*index] = Some(Step::of(Status::NotApplied));
        }
    } else if !eligible.is_empty() {
        let eligible_ids: Vec<&str> = eligible.iter().map(|i| ids[*i]).collect();
        let eligible_items: Vec<&SetupItem> = eligible.iter().map(|i| items[*i]).collect();
        // None when what the stored preview writes is no longer approved.
        let applied = preview(&eligible_ids, false).and_then(|stored| {
            if !still_approved(ctx, target, agent, &eligible_items)? {
                return Ok(None);
            }
            mcps::dispatch(
                ctx,
                json!({ "action": "apply", "previewId": stored["previewId"] }),
            )
            .map(Some)
        });
        let verified = match applied {
            Ok(None) => Err(Step::because(Status::NeedsReview, "changed_since_check")),
            Ok(Some(reply)) if reply["applied"] == true => preview(&eligible_ids, true)
                .map(|checked| {
                    change_kinds(&checked)
                        .iter()
                        .map(|k| *k == "unchanged")
                        .collect::<Vec<_>>()
                })
                .map_err(Step::error),
            // A partial write leaves a recovery that blocks a retry.
            Ok(Some(_)) => Err(Step::failed("apply_incomplete", false)),
            Err(error) => Err(Step::error(error)),
        };
        for (n, index) in eligible.iter().enumerate() {
            steps[*index] = Some(match &verified {
                Ok(checks) if checks.get(n) == Some(&true) => Step::of(Status::Changed),
                Ok(_) => Step::failed("verification_failed", false),
                Err(step) => step.clone(),
            });
        }
    }
    steps.into_iter().flatten().collect()
}

/// One kind per operation, in order. `update` with equal summaries is an
/// entry that already matches, unless it matches only because an edit there
/// was kept.
fn change_kinds(preview: &Value) -> Vec<&str> {
    preview["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|change| match change["kind"].as_str() {
            Some("update") if change["keptEdits"] == true => "kept_edits",
            Some("update") if change["beforeSummary"] == change["afterSummary"] => "unchanged",
            Some(kind) => kind,
            None => "",
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::apply_mcps;
    use crate::core::project_setup::apply::tests::{approved, Fixture};
    use crate::core::project_setup::items::{Run, Status};
    use crate::core::project_setup::SetupItem;

    #[test]
    fn a_value_changed_after_its_approval_was_checked_is_not_applied() {
        let fixture = Fixture::new();
        let mcp = fixture.mcp("docs");
        let item: SetupItem = serde_json::from_value(json!({
            "kind": "mcp", "id": mcp["id"], "agents": ["claude_code"], "approved": approved(&mcp),
        }))
        .unwrap();
        let project = fixture.ctx.store.get_project_by_id("p1").unwrap().unwrap();
        let run = Run::new(&fixture.ctx, project, true).unwrap();
        // The approval was checked; then the Library's arguments changed.
        fixture.request(
            "mcps_request",
            json!({ "action": "save", "expectedRevision": mcp["revision"], "definition": {
                "id": mcp["id"], "name": "docs", "transport": "stdio",
                "server": { "command": "npx", "args": ["-y", "other"] } } }),
        );

        let steps = apply_mcps(&run, "claude_code", &[(0, &item)]);

        assert_eq!(steps[0].status, Status::NeedsReview);
        assert!(!fixture.path(".mcp.json").exists());
    }
}
