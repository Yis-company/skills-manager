//! Backend-owned interactive work. See docs/operations.md for the wire contract.
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{error::AppError, host::HostCtx};
use crate::commands::projects;

mod installation;
pub mod results;
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
    #[serde(default)]
    review_only: bool,
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
    requests: usize,
    sealed: bool,
    canceling: bool,
    shutdown_epoch: u64,
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

// Counts an admission/reconciliation before it can inspect external evidence.
// The final seal and the real worker count use the same service lock.
struct RequestGuard<'a>(&'a OperationService);
impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().requests -= 1;
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
    ShutdownCancel {
        epoch: Option<u64>,
    },
    ShutdownStatus,
    ShutdownSeal {
        epoch: Option<u64>,
    },
    ShutdownResume {
        epoch: Option<u64>,
    },
    PrepareReview {
        operation_id: String,
        attempt_id: String,
        item_ids: Option<Vec<String>>,
    },
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
    let verify = request["action"] == "reconcile";
    let request: OperationRequest =
        serde_json::from_value(request).map_err(|e| AppError::invalid_input(e.to_string()))?;
    if matches!(
        request,
        OperationRequest::ShutdownCancel { .. }
            | OperationRequest::ShutdownStatus
            | OperationRequest::ShutdownSeal { .. }
            | OperationRequest::ShutdownResume { .. }
    ) {
        let mut state = ctx.operations.0.lock().unwrap();
        let epoch = match &request {
            OperationRequest::ShutdownCancel { epoch }
            | OperationRequest::ShutdownSeal { epoch }
            | OperationRequest::ShutdownResume { epoch } => *epoch,
            _ => None,
        };
        // A response deadline may expire while a host waits for its service
        // lock. An older seal/resume cannot overtake a newer Keep open or seal.
        if let Some(epoch) = epoch {
            if epoch < state.shutdown_epoch {
                return Ok(json!({"ready":false,"snapshots":[]}));
            }
            state.shutdown_epoch = epoch;
        }
        if matches!(request, OperationRequest::ShutdownResume { .. }) {
            state.sealed = false;
            state.canceling = false;
        }
        if matches!(request, OperationRequest::ShutdownCancel { .. }) {
            state.canceling = true;
            for record in &mut state.records {
                if record.snapshot.settled {
                    continue;
                }
                cancel_record(ctx, record)?;
            }
        }
        let ready = state.requests == 0
            && state.running == 0
            && !state.driving
            && state.records.iter().all(|r| r.snapshot.settled);
        if ready
            && matches!(
                request,
                OperationRequest::ShutdownSeal { .. } | OperationRequest::ShutdownCancel { .. }
            )
        {
            state.sealed = true;
        }
        return Ok(
            json!({"ready":ready,"snapshots":state.records.iter().map(|r| &r.snapshot).collect::<Vec<_>>() }),
        );
    }
    {
        let mut state = ctx.operations.0.lock().unwrap();
        if state.sealed
            && !matches!(
                request,
                OperationRequest::List | OperationRequest::Read { .. }
            )
        {
            return Err(AppError::invalid_input(
                "The app is closing; keep it open before starting more work",
            ));
        }
        if state.canceling
            && matches!(
                request,
                OperationRequest::SubmitInstall { .. }
                    | OperationRequest::SubmitProjectUpdate { .. }
                    | OperationRequest::ConfirmInstall { .. }
                    | OperationRequest::RetryFailed { .. }
                    | OperationRequest::PrepareReview { .. }
            )
        {
            return Err(AppError::invalid_input(
                "The app is stopping unfinished work; keep it open before starting more",
            ));
        }
        state.requests += 1;
    }
    let _request = RequestGuard(&ctx.operations);
    match request {
        OperationRequest::ShutdownCancel { .. }
        | OperationRequest::ShutdownStatus
        | OperationRequest::ShutdownSeal { .. }
        | OperationRequest::ShutdownResume { .. } => unreachable!(),
        OperationRequest::PrepareReview {
            operation_id,
            attempt_id,
            item_ids,
        } => installation::prepare_review(ctx, &operation_id, attempt_id, item_ids),
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
                review_only: false,
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
            let mut state = ctx.operations.0.lock().unwrap();
            let mut record = state
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
            let mut snapshot = record.snapshot.clone();
            // A receipt proves recorded outcomes, not ownership of a worker
            // in another process/session. Never replay an unowned apply.
            if !state
                .records
                .iter()
                .any(|r| r.snapshot.operation_id == operation_id)
            {
                snapshot = unowned_snapshot(snapshot);
            }
            if verify {
                for (index, item) in snapshot.items.iter_mut().enumerate() {
                    if item.phase != Phase::OutcomeUnknown {
                        continue;
                    }
                    let verified = if let Some(Some(update)) = record.updates.get(index) {
                        projects::verify_captured_project_update(
                            ctx,
                            snapshot.project_id.as_deref().unwrap_or_default(),
                            update,
                        )
                        .unwrap_or(false)
                    } else if let Some(work) = record.installs.get(index) {
                        installation::verify(ctx, &snapshot.destination, work).unwrap_or(false)
                    } else {
                        false
                    };
                    if verified {
                        item.phase = Phase::Unchanged;
                        item.message = Some("Verified the original destination matches the intended content; no write repeated".into());
                    }
                }
            }
            if verify
                && snapshot
                    .items
                    .iter()
                    .zip(&record.snapshot.items)
                    .any(|(new, old)| {
                        new.phase == Phase::Unchanged && old.phase == Phase::OutcomeUnknown
                    })
            {
                if let Some(index) = state
                    .records
                    .iter()
                    .position(|r| r.snapshot.operation_id == operation_id)
                {
                    record.snapshot = snapshot;
                    save(ctx, &mut record)?;
                    snapshot = record.snapshot.clone();
                    state.records[index] = record;
                }
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
            cancel_record(ctx, record)?;
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
            if !state
                .records
                .iter()
                .any(|r| r.snapshot.operation_id == operation_id)
            {
                let saved: Record = ctx
                    .store
                    .resource_state_get(RECEIPT, &operation_id)
                    .map_err(AppError::db)?
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(AppError::db)?
                    .ok_or_else(|| {
                        AppError::not_found("Operation receipt not found; prepare and review again")
                    })?;
                if matches!(saved.snapshot.work, OperationWork::Installation) {
                    return Err(AppError::invalid_input(
                        "Prepare and review this source again before retrying",
                    ));
                }
                // Explicit retry creates a fresh attempt from durable project
                // evidence; no original worker or temporary staging is adopted.
                state.records.push(Record {
                    review_only: false,
                    snapshot: unowned_snapshot(saved.snapshot),
                    updates: saved.updates,
                    installs: vec![],
                    attempts: saved.attempts,
                });
            }
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

fn cancel_record(ctx: &HostCtx, record: &mut Record) -> Result<(), AppError> {
    record.snapshot.cancel_requested = true;
    ctx.cancel.cancel(&record.snapshot.operation_id);
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
    save(ctx, record)
}

fn unowned_snapshot(mut snapshot: OperationSnapshot) -> OperationSnapshot {
    snapshot.preview = None;
    for item in &mut snapshot.items {
        if !item.phase.settled() {
            item.phase = Phase::OutcomeUnknown;
            item.message = Some(
                "Result not verified; work may still be running in its original session".into(),
            );
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
