//! Backend-owned interactive work. See docs/operations.md for the wire contract.
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{error::AppError, host::HostCtx};
use crate::commands::projects;

mod installation;
use installation::InstallWork;
pub use installation::{InstallSource, LocalOrigin};

pub const MAX_ACTIVE_ITEMS: usize = 2;
const RECEIPT: &str = "operation_receipt";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    Preparing,
    Waiting,
    Applying,
    Stopping,
    CheckingResult,
    Succeeded,
    Unchanged,
    Failed,
    Canceled,
    NeedsReview,
    OutcomeUnknown,
}
impl Phase {
    pub fn settled(&self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Unchanged
                | Self::Failed
                | Self::Canceled
                | Self::NeedsReview
                | Self::OutcomeUnknown
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationItem {
    pub id: String,
    pub attempt_id: String,
    pub relative_path: String,
    pub agent: Option<String>,
    #[serde(default)]
    pub source: Option<InstallSource>,
    pub phase: Phase,
    pub message: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationSnapshot {
    pub operation_id: String,
    pub attempt_id: String,
    pub host_id: Option<String>,
    #[serde(default)]
    pub work: OperationWork,
    pub project_id: Option<String>,
    pub project_path: Option<String>,
    #[serde(default)]
    pub destination: Option<Destination>,
    #[serde(default)]
    pub preview: Option<Vec<crate::commands::skills::GitSkillPreview>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<InstallSource>,
    pub revision: u64,
    pub settled: bool,
    pub cancel_requested: bool,
    pub items: Vec<OperationItem>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationWork {
    #[default]
    ProjectUpdate,
    Installation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Destination {
    Project { id: String, path: String },
    Library { path: String },
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    snapshot: OperationSnapshot,
    updates: Vec<Option<projects::CapturedProjectUpdate>>,
    attempts: Vec<String>,
    #[serde(default)]
    installs: Vec<InstallWork>,
}
#[derive(Default)]
struct State {
    records: Vec<Record>,
    running: usize,
    driving: bool,
    next_record: usize,
}
#[derive(Default)]
pub struct OperationService(Mutex<State>);
impl OperationService {
    pub(crate) fn cancel_requested(&self, id: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .records
            .iter()
            .find(|r| r.snapshot.operation_id == id)
            .is_some_and(|r| r.snapshot.cancel_requested)
    }
}

impl Drop for OperationService {
    fn drop(&mut self) {
        if let Ok(state) = self.0.get_mut() {
            for record in &state.records {
                installation::cleanup(record);
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OperationRequest {
    ConfirmInstall {
        operation_id: String,
        attempt_id: String,
        items: Vec<crate::commands::skills::SkillInstallItem>,
    },
    SubmitInstall {
        operation_id: String,
        host_id: Option<String>,
        sources: Vec<InstallSource>,
    },
    SubmitProjectUpdate {
        operation_id: String,
        host_id: Option<String>,
        project_id: String,
        skill_paths: Option<Vec<String>>,
    },
    Read {
        operation_id: String,
    },
    List,
    Cancel {
        operation_id: String,
    },
    RetryFailed {
        operation_id: String,
        attempt_id: String,
    },
    Reconcile {
        operation_id: String,
    },
}

pub fn dispatch(ctx: &HostCtx, request: Value) -> Result<Value, AppError> {
    let request: OperationRequest =
        serde_json::from_value(request).map_err(|e| AppError::invalid_input(e.to_string()))?;
    match request {
        OperationRequest::ConfirmInstall {
            operation_id,
            attempt_id,
            items,
        } => installation::confirm(ctx, &operation_id, attempt_id, items),
        OperationRequest::SubmitInstall {
            operation_id,
            host_id,
            sources,
        } => installation::submit(ctx, operation_id, host_id, sources),
        OperationRequest::SubmitProjectUpdate {
            operation_id,
            host_id,
            project_id,
            skill_paths,
        } => {
            let mut state = ctx.operations.0.lock().unwrap();
            if let Some(record) = state
                .records
                .iter()
                .find(|r| r.snapshot.operation_id == operation_id)
            {
                return Ok(json!(record.snapshot));
            }
            if let Some(value) = ctx
                .store
                .resource_state_get(RECEIPT, &operation_id)
                .map_err(AppError::db)?
            {
                let record: Record = serde_json::from_value(value).map_err(AppError::db)?;
                return Ok(json!(unowned_snapshot(record.snapshot)));
            }
            let (project_path, captured) =
                projects::capture_project_updates(ctx, &project_id, skill_paths.as_deref())?;
            if !captured.is_empty() {
                if let Some(existing) = state.records.iter().find(|r| {
                    r.snapshot.project_id.as_deref() == Some(project_id.as_str())
                        && !r.snapshot.settled
                        && captured.iter().all(|(update, path, agent, _)| {
                            r.snapshot.items.iter().zip(&r.updates).any(|(item, old)| {
                                !item.phase.settled()
                                    && item.relative_path == *path
                                    && item.agent.as_deref() == Some(agent.as_str())
                                    && old.as_ref().zip(update.as_ref()).is_some_and(|(a, b)| {
                                        a.target == b.target && a.source_content == b.source_content
                                    })
                            })
                        })
                }) {
                    return Ok(json!(existing.snapshot));
                }
            }
            let attempt_id = uuid::Uuid::new_v4().to_string();
            let mut record = Record {
                snapshot: OperationSnapshot {
                    operation_id,
                    attempt_id: attempt_id.clone(),
                    host_id,
                    destination: Some(Destination::Project {
                        id: project_id.clone(),
                        path: project_path.clone(),
                    }),
                    work: OperationWork::ProjectUpdate,
                    project_id: Some(project_id),
                    project_path: Some(project_path),
                    preview: None,
                    sources: vec![],
                    revision: 0,
                    settled: false,
                    cancel_requested: false,
                    items: vec![],
                },
                updates: vec![],
                installs: vec![],
                attempts: vec![attempt_id.clone()],
            };
            for (update, relative_path, agent, phase) in captured {
                record.snapshot.items.push(OperationItem {
                    id: uuid::Uuid::new_v4().to_string(),
                    attempt_id: attempt_id.clone(),
                    relative_path,
                    agent: Some(agent),
                    source: None,
                    phase,
                    message: None,
                });
                record.updates.push(update);
            }
            save(ctx, &mut record)?;
            let snapshot = record.snapshot.clone();
            state.records.push(record);
            if !state.driving && !snapshot.settled {
                state.driving = true;
                let ctx = ctx.clone();
                std::thread::spawn(move || drive(ctx));
            }
            Ok(json!(snapshot))
        }
        OperationRequest::Read { operation_id } | OperationRequest::Reconcile { operation_id } => {
            let state = ctx.operations.0.lock().unwrap();
            let record = state
                .records
                .iter()
                .find(|r| r.snapshot.operation_id == operation_id)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| {
                    ctx.store
                        .resource_state_get(RECEIPT, &operation_id)
                        .map_err(AppError::db)?
                        .map(|v| serde_json::from_value::<Record>(v).map_err(AppError::db))
                        .unwrap_or_else(|| Err(AppError::not_found("Operation receipt not found")))
                })?;
            let mut snapshot = record.snapshot;
            // A receipt proves recorded outcomes, not ownership of a worker
            // in another process/session. Never replay an unowned apply.
            if !state
                .records
                .iter()
                .any(|r| r.snapshot.operation_id == operation_id)
            {
                snapshot = unowned_snapshot(snapshot);
            }
            Ok(json!(snapshot))
        }
        OperationRequest::Cancel { operation_id } => {
            let mut state = ctx.operations.0.lock().unwrap();
            let record = state
                .records
                .iter_mut()
                .find(|r| r.snapshot.operation_id == operation_id)
                .ok_or_else(|| AppError::not_found("Operation not found"))?;
            record.snapshot.cancel_requested = true;
            ctx.cancel.cancel(&operation_id);
            for item in &mut record.snapshot.items {
                match item.phase {
                    Phase::Queued | Phase::Waiting => {
                        item.phase = Phase::Canceled;
                        item.message = None;
                    }
                    Phase::Applying | Phase::Preparing => item.phase = Phase::Stopping,
                    _ => {}
                }
            }
            save(ctx, record)?;
            let snapshot = record.snapshot.clone();
            drop(state);
            notify(ctx, &snapshot);
            Ok(json!(snapshot))
        }
        OperationRequest::RetryFailed {
            operation_id,
            attempt_id,
        } => {
            let mut state = ctx.operations.0.lock().unwrap();
            let record = state
                .records
                .iter_mut()
                .find(|r| r.snapshot.operation_id == operation_id)
                .ok_or_else(|| AppError::not_found("Operation not found"))?;
            if record.attempts.contains(&attempt_id) {
                return Ok(json!(record.snapshot));
            }
            if !record.snapshot.settled {
                return Err(AppError::invalid_input(
                    "Wait for this operation to finish before retrying",
                ));
            }
            if !record
                .snapshot
                .items
                .iter()
                .any(|item| item.phase == Phase::Failed)
            {
                return Ok(json!(record.snapshot));
            }
            let original = record.clone();
            ctx.store
                .resource_state_put(
                    "operation_attempt",
                    &format!("{}/{}", operation_id, record.snapshot.attempt_id),
                    &json!(record),
                )
                .map_err(AppError::db)?;
            record.attempts.push(attempt_id.clone());
            record.snapshot.attempt_id = attempt_id.clone();
            record.snapshot.cancel_requested = false;
            for item in &mut record.snapshot.items {
                if item.phase == Phase::Failed {
                    item.phase = Phase::Queued;
                    item.attempt_id = attempt_id.clone();
                    item.message = None;
                }
            }
            if let Err(error) = save(ctx, record) {
                *record = original;
                return Err(error);
            }
            let snapshot = record.snapshot.clone();
            if !state.driving {
                state.driving = true;
                let ctx = ctx.clone();
                std::thread::spawn(move || drive(ctx));
            }
            drop(state);
            notify(ctx, &snapshot);
            Ok(json!(snapshot))
        }
        OperationRequest::List => Ok(json!(ctx
            .operations
            .0
            .lock()
            .unwrap()
            .records
            .iter()
            .map(|r| &r.snapshot)
            .collect::<Vec<_>>())),
    }
}

fn unowned_snapshot(mut snapshot: OperationSnapshot) -> OperationSnapshot {
    for item in &mut snapshot.items {
        if !item.phase.settled() {
            item.phase = Phase::OutcomeUnknown;
            item.message =
                Some("No live operation owner; the recorded result needs reconciliation".into());
        }
    }
    snapshot.settled = true;
    snapshot
}

fn save(ctx: &HostCtx, record: &mut Record) -> Result<(), AppError> {
    record.snapshot.revision += 1;
    record.snapshot.settled = record.snapshot.items.iter().all(|i| i.phase.settled());
    ctx.store
        .resource_state_put(RECEIPT, &record.snapshot.operation_id, &json!(record))
        .map_err(AppError::db)
}
fn notify(ctx: &HostCtx, snapshot: &OperationSnapshot) {
    ctx.events.emit("operation-changed", json!(snapshot));
}
#[allow(dead_code)]
enum Guard {
    Project(super::project_update_lock::ProjectUpdateLock),
    Library(super::repo_lock::RepoLock),
    Preparation,
}
fn drive(ctx: HostCtx) {
    loop {
        let task = {
            let mut state = ctx.operations.0.lock().unwrap();
            if state.running < MAX_ACTIVE_ITEMS {
                let mut candidate = None;
                let count = state.records.len();
                let first = state.next_record;
                'records: for offset in 0..count {
                    let r = (first + offset) % count;
                    let record = &mut state.records[r];
                    for i in 0..record.snapshot.items.len() {
                        if !matches!(
                            record.snapshot.items[i].phase,
                            Phase::Queued | Phase::Waiting
                        ) {
                            continue;
                        }
                        let previous = record.snapshot.items[i].phase.clone();
                        if !record.installs.is_empty() {
                            if record.installs[i].prepared.is_none() {
                                candidate = Some((r, i, Guard::Preparation));
                                break 'records;
                            }
                            match super::repo_lock::RepoLock::try_exclusive("operation install")
                                .map_err(AppError::io)
                            {
                                Ok(Some(guard)) => {
                                    candidate = Some((r, i, Guard::Library(guard)));
                                    break 'records;
                                }
                                Ok(None) => {
                                    record.snapshot.items[i].phase = Phase::Waiting;
                                    record.snapshot.items[i].message =
                                        Some("Waiting for another change to the library".into());
                                }
                                Err(e) => {
                                    record.snapshot.items[i].phase = Phase::Failed;
                                    record.snapshot.items[i].message = Some(e.message);
                                }
                            }
                            if record.snapshot.items[i].phase != previous {
                                let _ = save(&ctx, record);
                            }
                            continue;
                        }
                        let update = record.updates[i].as_ref().unwrap();
                        let previous = record.snapshot.items[i].phase.clone();
                        match super::project_update_lock::ProjectUpdateLock::try_acquire(
                            &update.target,
                        ) {
                            Ok(Some(guard)) => {
                                candidate = Some((r, i, Guard::Project(guard)));
                                break 'records;
                            }
                            Ok(None) => {
                                record.snapshot.items[i].phase = Phase::Waiting;
                                record.snapshot.items[i].message = Some(
                                    "Waiting for another change to this skill or library".into(),
                                );
                            }
                            Err(error) => {
                                record.snapshot.items[i].phase = Phase::Failed;
                                record.snapshot.items[i].message = Some(error.message);
                            }
                        }
                        if record.snapshot.items[i].phase != previous {
                            if let Err(error) = save(&ctx, record) {
                                record.snapshot.items[i].phase = Phase::Failed;
                                record.snapshot.items[i].message = Some(error.message);
                                record.snapshot.settled =
                                    record.snapshot.items.iter().all(|i| i.phase.settled());
                            }
                        }
                    }
                }
                if let Some((r, i, guard)) = candidate {
                    let record = &mut state.records[r];
                    record.snapshot.items[i].phase = if matches!(guard, Guard::Preparation) {
                        Phase::Preparing
                    } else {
                        Phase::Applying
                    };
                    record.snapshot.items[i].message = None;
                    if let Err(error) = save(&ctx, record) {
                        record.snapshot.items[i].phase = Phase::Failed;
                        record.snapshot.items[i].message = Some(error.message);
                        record.snapshot.settled =
                            record.snapshot.items.iter().all(|i| i.phase.settled());
                        continue;
                    }
                    let snapshot = record.snapshot.clone();
                    let update = record.updates.get(i).cloned().flatten();
                    let install = record.installs.get(i).cloned();
                    state.next_record = (r + 1) % state.records.len();
                    state.running += 1;
                    Some((r, i, snapshot, update, install, guard))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((r, i, snapshot, update, install, guard)) = task {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let guard = guard;
                notify(&ctx, &snapshot);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if let Some(install) = install {
                        return installation::execute(&ctx, &snapshot.operation_id, i, install);
                    }
                    projects::apply_captured_project_update(
                        &ctx,
                        &snapshot.operation_id,
                        snapshot.project_id.as_deref().unwrap(),
                        update.as_ref().unwrap(),
                    )
                }));
                let result = result.unwrap_or_else(|_| Ok(Phase::OutcomeUnknown));
                drop(guard);
                let snapshot = {
                    let mut state = ctx.operations.0.lock().unwrap();
                    let record = &mut state.records[r];
                    let item = &mut record.snapshot.items[i];
                    match result {
                        Ok(phase) => {
                            item.message = match phase {
                                Phase::NeedsReview => {
                                    let reason = if record.snapshot.preview.is_some() {
                                        "Select skills to install; no work is running while you review"
                                    } else {
                                        "Source, destination or deployment choice changed; review before continuing"
                                    };
                                    Some(reason.into())
                                }
                                Phase::Waiting => {
                                    Some("Waiting for another change to the library".into())
                                }
                                _ => None,
                            };
                            item.phase = phase;
                        }
                        Err(error) if error.message == super::git_fetcher::CACHE_BUSY => {
                            item.phase = if record.snapshot.cancel_requested {
                                Phase::Canceled
                            } else {
                                Phase::Waiting
                            };
                            item.message = Some(error.message);
                        }
                        Err(error) => {
                            item.phase = if error.kind == super::error::ErrorKind::Cancelled {
                                Phase::Canceled
                            } else if matches!(
                                error.kind,
                                super::error::ErrorKind::InvalidInput
                                    | super::error::ErrorKind::TargetConflict
                            ) {
                                Phase::NeedsReview
                            } else {
                                Phase::Failed
                            };
                            item.message = Some(error.message);
                        }
                    }
                    if let Err(error) = save(&ctx, record) {
                        record.snapshot.items[i].phase = Phase::OutcomeUnknown;
                        record.snapshot.items[i].message =
                            Some(format!("Could not record result: {error}"));
                        record.snapshot.settled = record
                            .snapshot
                            .items
                            .iter()
                            .all(|item| item.phase.settled());
                    }
                    let snapshot = record.snapshot.clone();
                    state.running -= 1;
                    snapshot
                };
                notify(&ctx, &snapshot);
                let completed = {
                    let state = ctx.operations.0.lock().unwrap();
                    let record = &state.records[r];
                    (record.snapshot.settled
                        && record.snapshot.items.iter().all(|item| {
                            matches!(
                                item.phase,
                                Phase::Succeeded | Phase::Unchanged | Phase::Canceled
                            )
                        }))
                    .then(|| record.clone())
                };
                if let Some(record) = completed {
                    installation::cleanup(&record);
                }
            });
            continue;
        }
        {
            let mut state = ctx.operations.0.lock().unwrap();
            if state.running == 0 && state.records.iter().all(|r| r.snapshot.settled) {
                state.driving = false;
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
