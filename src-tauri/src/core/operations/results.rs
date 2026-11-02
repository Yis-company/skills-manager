//! Desktop-local unresolved summaries. These are never worker or confirmation authority.
use super::*;

const KIND: &str = "operation_results";
const ID: &str = "desktop";

#[derive(Clone, Serialize, Deserialize)]
pub struct Origin {
    pub host_name: String,
    pub ssh_target: Option<String>,
    pub host_created_at: Option<i64>,
    pub project_name: Option<String>,
}

#[derive(Clone, Deserialize)]
pub struct Admission {
    action: String,
    expected_attempt: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RetainedResult {
    #[serde(flatten)]
    pub snapshot: OperationSnapshot,
    pub origin: Option<Origin>,
    pub recovery: Option<String>,
    pub recovery_attempt: Option<String>,
    #[serde(default, skip_serializing)]
    pub admission: Option<Admission>,
}

#[derive(Default, Serialize, Deserialize)]
struct SavedResults {
    version: u64,
    results: Vec<RetainedResult>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Request {
    Read,
    Write {
        version: u64,
        results: Vec<RetainedResult>,
    },
}

pub fn dispatch(ctx: &HostCtx, request: Value) -> Result<Value, AppError> {
    let request: Request = serde_json::from_value(request)
        .map_err(|_| AppError::invalid_input("Invalid operation results"))?;
    match request {
        Request::Read => {
            let saved = ctx
                .store
                .resource_state_get(KIND, ID)
                .map_err(|_| AppError::db("Could not load saved operation results"))?;
            Ok(saved.unwrap_or_else(|| json!(SavedResults::default())))
        }
        Request::Write { version, results } => {
            let mut retained = vec![];
            for mut result in results {
                let snapshot = &mut result.snapshot;
                if snapshot.settled
                    && result.admission.is_none()
                    && result.recovery.is_none()
                    && !snapshot.items.iter().any(|i| {
                        matches!(
                            i.phase,
                            Phase::Failed | Phase::NeedsReview | Phase::OutcomeUnknown
                        )
                    })
                {
                    continue;
                }
                if let Some(admission) = &result.admission {
                    result.recovery = Some("unknown".into());
                    result.recovery_attempt = admission.expected_attempt.clone();
                    for item in &mut snapshot.items {
                        if !item.phase.settled()
                            || (admission.action == "retry_failed" && item.phase == Phase::Failed)
                            || (admission.action == "confirm_install"
                                && item.phase == Phase::NeedsReview)
                        {
                            item.phase = Phase::OutcomeUnknown;
                        }
                    }
                }
                if !snapshot.settled
                    || result.recovery.as_deref() == Some("checking")
                    || snapshot
                        .items
                        .iter()
                        .any(|i| i.phase == Phase::OutcomeUnknown)
                {
                    result.recovery = Some("unknown".into());
                }
                if result.recovery.is_some()
                    && result.recovery_attempt.is_none()
                    && !snapshot.attempt_id.is_empty()
                {
                    result.recovery_attempt = Some(snapshot.attempt_id.clone());
                }
                *snapshot = unowned_snapshot(snapshot.clone());
                for source in &mut snapshot.sources {
                    installation::sanitize(source)?;
                }
                for item in &mut snapshot.items {
                    if let Some(source) = &mut item.source {
                        let old = match source {
                            InstallSource::Git { url } => Some(url.clone()),
                            _ => None,
                        };
                        installation::sanitize(source)?;
                        if old.as_deref() == Some(&item.relative_path) {
                            if let InstallSource::Git { url } = source {
                                item.relative_path = url.clone();
                            }
                        }
                    }
                    // Error strings can contain remote stderr, credentials and query tokens.
                    // The phase and original source/destination provide recoverable context.
                    item.message = None;
                }
                retained.push(result);
            }
            let saved = json!(SavedResults {
                version: version + 1,
                results: retained
            });
            if !ctx
                .store
                .resource_state_put_versioned(KIND, ID, version, &saved)
                .map_err(|_| AppError::db("Could not save operation results; retry saving"))?
            {
                return Err(AppError::invalid_input(
                    "Operation results changed in another session; reload before saving",
                ));
            }
            Ok(saved)
        }
    }
}
