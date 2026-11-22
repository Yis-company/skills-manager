//! Inspect a Project setup result: the receipt an `operation_id` names, and
//! the checkout's current state against its saved setup.
//!
//! Read-only. It runs on a store opened read-only, never resubmits, and
//! never repairs a receipt. A receipt with no result, or no receipt at all,
//! is Outcome unknown: a missing receipt is not proof that nothing was
//! written. A destination that matches is reported as verified state, never
//! as this operation's change. The destination is checked for the items and
//! agents the operation ran, as its receipt records them, and otherwise for
//! the saved setup. A checkout that is not registered has no destination.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::apply::{load_setup, receipt as find_receipt, without_evidence, Outcome};
use super::items::{check_setup, counts, Run, Status};
use super::{find_project, SetupItem};
use crate::core::error::{AppError, ErrorKind};
use crate::core::git_backup_store::effective_device_name;
use crate::core::host::HostCtx;

/// What this host recorded for an `operation_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    Missing,
    /// Claimed, with no result recorded yet.
    IntentOnly,
    Result,
    /// Settled with an error before any item was applied.
    Error,
}

pub(super) fn inspect(
    ctx: &HostCtx,
    checkout_path: &str,
    operation_id: &str,
    evidence: bool,
) -> Result<Value, AppError> {
    let (checkout, project) = find_project(ctx, checkout_path)?;
    let host = effective_device_name(&ctx.store);
    let receipt = find_receipt(ctx, operation_id)?;
    if let Some(receipt) = &receipt {
        let intent = &receipt["intent"];
        if intent["host"] != host.as_str() || intent["checkout_path"] != json!(checkout) {
            return Err(AppError::invalid_input(
                "This operation_id belongs to another host or checkout; inspect it there",
            ));
        }
    }
    let receipt_state = match &receipt {
        None => ReceiptState::Missing,
        Some(receipt) if receipt["result"].is_object() => ReceiptState::Result,
        Some(receipt) if !receipt["error"].is_null() => ReceiptState::Error,
        Some(_) => ReceiptState::IntentOnly,
    };
    let error: Option<AppError> = receipt.as_ref().and_then(|receipt| {
        (receipt_state == ReceiptState::Error).then(|| {
            serde_json::from_value(receipt["error"].clone()).unwrap_or_else(|_| {
                AppError::internal("This operation_id recorded an unreadable error")
            })
        })
    });
    let targets: Option<Vec<SetupItem>> = receipt
        .as_ref()
        .and_then(|receipt| serde_json::from_value(receipt["targets"].clone()).ok());
    let mut recorded = receipt.map_or(Value::Null, |receipt| receipt["result"].clone());
    without_evidence(&mut recorded, evidence);

    let project_id = project.as_ref().map(|project| project.id.clone());
    let setup = match &project {
        Some(project) => load_setup(ctx, &project.id)?,
        None => None,
    };
    let (destination, holds, recovery_possible) = match (project, setup) {
        (None, _) | (_, None) => (Value::Null, false, false),
        (Some(project), Some(setup)) => {
            let checked = targets.as_deref().unwrap_or(&setup.items);
            let items = check_setup(&Run::new(ctx, project, false)?, checked);
            let count = |status| items.iter().filter(|item| item.status == status).count();
            let holds = count(Status::Unchanged) == items.len();
            let mut destination = json!({
                "setup_version": setup.version,
                "state": if holds { "matches" } else { "differs" },
                "counts": counts(&items, &[
                    Status::Unchanged, Status::NotApplied, Status::NeedsReview,
                    Status::Blocked, Status::Missing, Status::Failed,
                ]),
            });
            if evidence {
                destination["items"] = json!(items);
            }
            // Only a settled operation leaves work a new apply may take up.
            let settled = receipt_state == ReceiptState::Result;
            (destination, holds, settled && count(Status::NotApplied) > 0)
        }
    };

    let failed = error.as_ref().map(|error| {
        format!(
            "This operation recorded an error and applied no items: {} Resolve it, then apply \
             under a new operation_id.",
            error.message
        )
    });
    let next_action = match (receipt_state, holds, recovery_possible) {
        (ReceiptState::Error, _, _) => failed.as_deref().unwrap_or_default(),
        (ReceiptState::Result, _, _) if destination.is_null() => {
            "Discover this checkout: it has no saved Project setup."
        }
        (ReceiptState::Result, true, _) => "None: the checkout matches its saved setup.",
        (_, true, _) => {
            "None: the checkout matches its saved setup. This operation's own effect is \
             unknown, and no write was repeated."
        }
        (ReceiptState::Missing | ReceiptState::IntentOnly, false, _) => {
            "Outcome unknown: a request may still hold this operation_id, or its result was \
             lost. Leave the checkout as it is. Inspect again later with the same \
             operation_id."
        }
        (_, false, true) => {
            "Apply under a new operation_id to set up the items not applied: apply checks \
             each destination before writing and never overwrites edits."
        }
        (_, false, false) => {
            "Inspect again with evidence: true to list the items that need a decision, \
             retry or recovery."
        }
    };
    let outcome = match (receipt_state, &error) {
        (ReceiptState::Result, _) => recorded["outcome"].clone(),
        (ReceiptState::Error, Some(error))
            if matches!(
                error.kind,
                ErrorKind::Database | ErrorKind::Io | ErrorKind::Network
            ) =>
        {
            json!(Outcome::RetryableFailure)
        }
        (ReceiptState::Error, _) => json!(Outcome::NeedsReview),
        _ => json!(Outcome::Unknown),
    };
    let mut inspected = json!({
        "operation_id": operation_id,
        "host": host,
        "checkout_path": checkout,
        "project_id": project_id,
        "outcome": outcome,
        "receipt": receipt_state,
        "recorded": recorded,
        "destination": destination,
        "recovery_possible": recovery_possible,
        "next_action": next_action,
    });
    if let Some(error) = error {
        inspected["error"] = json!(error);
    }
    Ok(inspected)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{json, Value};

    use super::super::apply::tests::{destinations, item, one_of_each, with_agent_b, Fixture};
    use crate::core::error::{AppError, ErrorKind};
    use crate::core::host::{HostCtx, NoopEvents};
    use crate::core::host_dispatch::dispatch;
    use crate::core::skill_store::SkillStore;

    /// Inspect through a store opened read-only, so any write fails the call.
    fn inspect(fixture: &Fixture, args: Value) -> Result<Value, AppError> {
        let db = fixture.repo._tmp.path().join("repo/test.db");
        let store = SkillStore::open_resource_read_only(&db).unwrap();
        let ctx = HostCtx::for_tests(store, Arc::new(NoopEvents));
        let mut request = json!({ "action": "inspect", "checkout_path": fixture.checkout });
        request
            .as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        dispatch(
            &ctx,
            "project_setup_request",
            &json!({ "request": request }),
        )
    }

    fn receipt(fixture: &Fixture, operation_id: &str) -> Option<Value> {
        fixture
            .ctx
            .store
            .resource_state_get("project_setup_receipt", operation_id)
            .unwrap()
    }

    #[test]
    fn a_completed_operation_returns_its_recorded_result_and_writes_nothing() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        let applied = fixture.apply("op-1").unwrap();
        let before = (destinations(&fixture), receipt(&fixture, "op-1"));

        let inspected = inspect(&fixture, json!({ "operation_id": "op-1" })).unwrap();

        assert_eq!(inspected["outcome"], "verified_changed");
        assert_eq!(inspected["receipt"], "result");
        assert_eq!(inspected["recorded"]["counts"], applied["counts"]);
        assert_eq!(inspected["destination"]["state"], "matches");
        assert_eq!(inspected["destination"]["counts"]["unchanged"], 3);
        assert_eq!(inspected["recovery_possible"], false);
        assert!(inspected["destination"].get("items").is_none());
        assert!(inspected.to_string().len() < 2048);
        assert_eq!((destinations(&fixture), receipt(&fixture, "op-1")), before);
    }

    #[test]
    fn an_intent_only_receipt_is_outcome_unknown_with_current_destination_evidence() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        let mut claimed = receipt(&fixture, "op-1").unwrap();
        claimed["version"] = json!(1);
        claimed.as_object_mut().unwrap().remove("result");
        fixture
            .ctx
            .store
            .resource_state_put("project_setup_receipt", "op-1", &claimed)
            .unwrap();
        std::fs::remove_file(fixture.path(".a/skills/review")).unwrap();
        let before = (destinations(&fixture), receipt(&fixture, "op-1"));

        let inspected = inspect(
            &fixture,
            json!({ "operation_id": "op-1", "evidence": true }),
        )
        .unwrap();

        assert_eq!(inspected["outcome"], "outcome_unknown");
        assert_eq!(inspected["receipt"], "intent_only");
        assert_eq!(inspected["destination"]["state"], "differs");
        assert_eq!(
            item(&inspected["destination"], "skill")["status"],
            "not_applied"
        );
        assert_eq!(
            item(&inspected["destination"], "mcp")["status"],
            "unchanged"
        );
        assert_eq!(inspected["recovery_possible"], false);
        assert_inspect_later(&inspected);
        assert_eq!((destinations(&fixture), receipt(&fixture, "op-1")), before);
    }

    /// Outcome unknown leaves the work alone: the only next step is a later
    /// inspection, never an apply under a new operation_id.
    fn assert_inspect_later(inspected: &Value) {
        let next = inspected["next_action"].as_str().unwrap();
        assert!(next.contains("Inspect again later"), "{next}");
        assert!(!next.contains("new operation_id"), "{next}");
    }

    #[test]
    fn an_unknown_id_on_a_differing_checkout_offers_only_a_later_inspection() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        std::fs::remove_file(fixture.path(".a/skills/review")).unwrap();

        let inspected = inspect(&fixture, json!({ "operation_id": "op-never" })).unwrap();

        assert_eq!(inspected["outcome"], "outcome_unknown");
        assert_eq!(inspected["destination"]["state"], "differs");
        assert_eq!(inspected["recovery_possible"], false);
        assert_inspect_later(&inspected);
    }

    #[test]
    fn a_recorded_result_with_items_not_applied_offers_a_new_apply() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        std::fs::remove_file(fixture.path(".a/skills/review")).unwrap();

        let inspected = inspect(&fixture, json!({ "operation_id": "op-1" })).unwrap();

        assert_eq!(inspected["receipt"], "result");
        assert_eq!(inspected["recovery_possible"], true);
        let next = inspected["next_action"].as_str().unwrap();
        assert!(next.contains("new operation_id"), "{next}");
    }

    #[test]
    fn an_unknown_id_is_outcome_unknown_and_never_claims_the_matching_state() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        let before = destinations(&fixture);

        let inspected = inspect(&fixture, json!({ "operation_id": "op-never" })).unwrap();

        assert_eq!(inspected["outcome"], "outcome_unknown");
        assert_eq!(inspected["receipt"], "missing");
        assert_eq!(inspected["recorded"], Value::Null);
        assert_eq!(inspected["destination"]["state"], "matches");
        let next = inspected["next_action"].as_str().unwrap();
        assert!(next.contains("unknown"), "{next}");
        assert_eq!(destinations(&fixture), before);
        assert_eq!(receipt(&fixture, "op-never"), None);
    }

    #[test]
    fn an_operation_run_for_named_agents_is_checked_against_those_agents() {
        let fixture = Fixture::new();
        with_agent_b(&fixture);
        fixture.skill("s1", "review");
        fixture.save_setup(json!([{ "kind": "skill", "id": "s1", "agents": ["agent_a"] }]));
        fixture
            .apply_with(json!({ "operation_id": "op-1", "agents": ["agent_b"] }))
            .unwrap();

        let inspected = inspect(
            &fixture,
            json!({ "operation_id": "op-1", "evidence": true }),
        )
        .unwrap();

        assert_eq!(inspected["destination"]["state"], "matches", "{inspected}");
        assert_eq!(
            item(&inspected["destination"], "skill")["agents"],
            json!(["agent_b"])
        );
        assert_eq!(inspected["recovery_possible"], false);
    }

    #[test]
    fn an_unregistered_checkout_is_outcome_unknown() {
        let fixture = Fixture::new();
        let fresh = fixture.repo._tmp.path().join("work/fresh");
        std::fs::create_dir_all(&fresh).unwrap();

        let inspected = inspect(
            &fixture,
            json!({ "operation_id": "op-1", "checkout_path": fresh }),
        )
        .unwrap();

        assert_eq!(inspected["outcome"], "outcome_unknown");
        assert_eq!(inspected["receipt"], "missing");
        assert_eq!(inspected["project_id"], Value::Null);
        assert_eq!(inspected["destination"], Value::Null);
        assert_inspect_later(&inspected);
    }

    #[test]
    fn an_operation_from_another_host_or_checkout_is_rejected() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        let mut moved = receipt(&fixture, "op-1").unwrap();
        moved["intent"]["host"] = json!("other-host");
        fixture
            .ctx
            .store
            .resource_state_put("project_setup_receipt", "op-2", &moved)
            .unwrap();
        let other = fixture.repo._tmp.path().join("work/other");
        std::fs::create_dir_all(&other).unwrap();
        let mut project = fixture.ctx.store.get_project_by_id("p1").unwrap().unwrap();
        project.id = "p2".into();
        project.path = other.to_string_lossy().into_owned();
        fixture.ctx.store.insert_project(&project).unwrap();

        let wrong_host = inspect(&fixture, json!({ "operation_id": "op-2" })).unwrap_err();
        let wrong_checkout = inspect(
            &fixture,
            json!({ "operation_id": "op-1", "checkout_path": other }),
        )
        .unwrap_err();

        for error in [wrong_host, wrong_checkout] {
            assert_eq!(error.kind, ErrorKind::InvalidInput);
            assert!(
                error.message.contains("another host or checkout"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn a_recorded_error_is_a_settled_outcome_with_its_kind() {
        let fixture = Fixture::new();
        one_of_each(&fixture);
        fixture.apply("op-1").unwrap();
        let mut settled = receipt(&fixture, "op-1").unwrap();
        settled.as_object_mut().unwrap().remove("result");
        settled["error"] = json!(AppError::target_conflict("taken", Vec::new()));
        fixture
            .ctx
            .store
            .resource_state_put("project_setup_receipt", "op-1", &settled)
            .unwrap();

        let inspected = inspect(&fixture, json!({ "operation_id": "op-1" })).unwrap();

        assert_eq!(inspected["receipt"], "error", "{inspected}");
        assert_eq!(inspected["outcome"], "needs_review");
        assert_eq!(inspected["error"]["kind"], "target_conflict");
        assert_eq!(inspected["error"]["message"], "taken");
        let next = inspected["next_action"].as_str().unwrap();
        assert!(next.contains("new operation_id"), "{next}");
    }
}
