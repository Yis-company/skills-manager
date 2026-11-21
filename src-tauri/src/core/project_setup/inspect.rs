//! Inspect a Project setup result: the receipt an `operation_id` names, and
//! the checkout's current state against its saved setup.
//!
//! Read-only. It runs on a store opened read-only, never resubmits, and
//! never repairs a receipt. A receipt with no result, or no receipt at all,
//! is Outcome unknown: a missing receipt is not proof that nothing was
//! written. A destination that matches is reported as verified state, never
//! as this operation's change.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::apply::{load_setup, without_evidence, RECEIPT_KIND};
use super::find_project;
use super::items::{check_saved, counts, Run, Status};
use crate::core::error::AppError;
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
}

pub(super) fn inspect(
    ctx: &HostCtx,
    checkout_path: &str,
    operation_id: &str,
    evidence: bool,
) -> Result<Value, AppError> {
    let (checkout, project) = find_project(ctx, checkout_path)?;
    let project = project.ok_or_else(|| {
        AppError::invalid_input("This checkout is not a registered project; discover it first")
    })?;
    let host = effective_device_name(&ctx.store);
    let receipt = ctx
        .store
        .resource_state_get(RECEIPT_KIND, operation_id)
        .map_err(AppError::db)?;
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
        Some(_) => ReceiptState::IntentOnly,
    };
    let mut recorded = receipt.map_or(Value::Null, |receipt| receipt["result"].clone());
    without_evidence(&mut recorded, evidence);

    let project_id = project.id.clone();
    let (destination, holds, recovery_possible) = match load_setup(ctx, &project.id)? {
        None => (Value::Null, false, false),
        Some(setup) => {
            let items = check_saved(&Run::new(ctx, project, false)?, &setup);
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
            (destination, holds, count(Status::NotApplied) > 0)
        }
    };

    let next_action = match (receipt_state, holds, recovery_possible) {
        _ if destination.is_null() => "Discover this checkout: it has no saved Project setup.",
        (ReceiptState::Result, true, _) => "None: the checkout matches its saved setup.",
        (_, true, _) => {
            "None: the checkout matches its saved setup. This operation's own effect is \
             unknown, and no write was repeated."
        }
        (ReceiptState::IntentOnly, false, _) => {
            "Another request may still hold this operation_id. Inspect again later with the \
             same operation_id. If it stays unknown, apply under a new operation_id: apply \
             checks each destination before writing and never overwrites edits."
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
    let outcome = match receipt_state {
        ReceiptState::Result => recorded["outcome"].clone(),
        _ => json!("outcome_unknown"),
    };
    Ok(json!({
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
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{json, Value};

    use super::super::apply::tests::{destinations, item, one_of_each, Fixture};
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
        assert_eq!(inspected["recovery_possible"], true);
        assert_eq!((destinations(&fixture), receipt(&fixture, "op-1")), before);
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
}
