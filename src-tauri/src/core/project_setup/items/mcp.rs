use serde_json::{json, Value};

use super::{Run, Status, Step};
use crate::core::error::ErrorKind;
use crate::core::host::HostCtx;
use crate::core::mcps;
use crate::core::project_setup::SetupItem;

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

/// The agents to set an MCP connection up for, or why none can be.
pub(super) fn check_mcp(run: &Run, item: &SetupItem) -> Result<Vec<String>, Step> {
    let Some(definition) = run.mcp_definition(&item.id) else {
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
    let ids: Vec<&str> = batch.iter().map(|(_, item)| item.id.as_str()).collect();
    ids.chunks(MCP_BATCH)
        .flat_map(|chunk| mcp_chunk(run.ctx, &target, chunk, run.write))
        .collect()
}

fn mcp_chunk(ctx: &HostCtx, target: &Value, ids: &[&str], write: bool) -> Vec<Step> {
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
                .flat_map(|id| mcp_chunk(ctx, target, &[id], write))
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
                .map_err(Step::error),
            // A partial write leaves a recovery that blocks a retry.
            Ok(_) => Err(Step::failed("apply_incomplete", false)),
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
