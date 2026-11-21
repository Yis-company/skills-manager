//! Project setup on a registered SSH host: the same `project_setup_request`,
//! answered by the same core there over `serve --stdio`. One request per
//! tool call, never one per item.
//!
//! Each host's session opens with [`RemoteSession::spawn`], which refuses
//! another version and never installs or updates the host, and stays open
//! while the server runs, so `serve` starts once, and is started again when
//! the host's connection details change. When the link drops, the server
//! reconnects once. A request that only reads is sent again. An apply that may
//! write is not: the server inspects the same `operation_id` and returns a
//! recorded result, and otherwise reports Outcome unknown, since a missing
//! receipt is not proof that nothing ran.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::core::error::{AppError, ErrorKind};
use crate::core::host::{HostCtx, NoopEvents};
use crate::core::project_setup::ReceiptState;
use crate::core::remote_session::{CliCommand, RemoteSession};
use crate::core::skill_store::RemoteHostRecord;

const COMMAND: &str = "project_setup_request";

pub(super) struct Hosts<'a> {
    cli: &'a CliCommand,
    /// By host ID, kept open: each `serve` start rewrites state on its host.
    /// Each with the record it was started from: a session is reused only
    /// while the host's connection details still match it.
    sessions: RefCell<HashMap<String, (RemoteHostRecord, RemoteSession)>>,
    /// Built on first use and shared by every call.
    runtime: OnceCell<tokio::runtime::Runtime>,
}

impl<'a> Hosts<'a> {
    pub(super) fn new(cli: &'a CliCommand) -> Self {
        Self {
            cli,
            sessions: RefCell::default(),
            runtime: OnceCell::new(),
        }
    }

    fn runtime(&self) -> Result<&tokio::runtime::Runtime, AppError> {
        if let Some(runtime) = self.runtime.get() {
            return Ok(runtime);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(AppError::internal)?;
        Ok(self.runtime.get_or_init(|| runtime))
    }

    /// Answer `request` on the SSH host registered here as `name`.
    pub(super) fn call(
        &self,
        local: &HostCtx,
        name: &str,
        request: Map<String, Value>,
    ) -> Result<Value, AppError> {
        let host = resolve(local, name)?;
        let runtime = self.runtime()?;
        let send = |session: &RemoteSession, request: &Map<String, Value>| {
            runtime.block_on(session.call(COMMAND, json!({ "request": request })))
        };
        let mut sessions = self.sessions.borrow_mut();
        let reusable = sessions
            .get(&host.id)
            .is_some_and(|(started, session)| session.is_alive() && same_address(started, &host));
        if !reusable {
            sessions.remove(&host.id);
            let session = connect(self.cli, &host)?;
            sessions.insert(host.id.clone(), (host.clone(), session));
        }

        match send(&sessions[&host.id].1, &request) {
            // Project setup itself raises no network errors: this is the link.
            Err(error) if error.kind == ErrorKind::Network => {
                sessions.remove(&host.id);
                if writes_nothing(&request) {
                    let session = connect(self.cli, &host)?;
                    let session = &sessions.entry(host.id.clone()).or_insert((host, session)).1;
                    return send(session, &request);
                }
                let unknown = |why: String| {
                    AppError::network(format!(
                        "Outcome unknown: the connection to {name} was lost during apply, and \
                         {why}. Leave the checkout as it is. Inspect again later: call \
                         inspect_project_setup_result with the same host and operation_id; do \
                         not apply under a new operation_id until then."
                    ))
                };
                let session = RemoteSession::spawn(self.cli, &host, Arc::new(NoopEvents))
                    .map_err(|e| unknown(format!("reconnecting failed: {}", e.message)))?;
                let session = &sessions.entry(host.id.clone()).or_insert((host, session)).1;
                let mut inspect: Map<String, Value> = ["checkout_path", "operation_id", "evidence"]
                    .into_iter()
                    .filter_map(|key| Some((key.to_string(), request.get(key)?.clone())))
                    .collect();
                inspect.insert("action".into(), json!("inspect"));
                let found = send(session, &inspect)
                    .map_err(|e| unknown(format!("inspecting it failed: {}", e.message)))?;
                match serde_json::from_value(found["receipt"].clone()) {
                    Ok(ReceiptState::Result) => {
                        let mut recorded = found["recorded"].clone();
                        recorded["from_receipt"] = json!(true);
                        Ok(recorded)
                    }
                    _ => Err(unknown("it has not recorded a result".into())),
                }
            }
            answered => answered,
        }
    }
}

/// Whether `request` only reads: discovery, inspection, or an apply that
/// builds a first setup's proposal. Only these are sent again after the link
/// drops.
fn writes_nothing(request: &Map<String, Value>) -> bool {
    request["action"] != "apply"
        || (request.contains_key("items") && !request.contains_key("confirm"))
}

/// Whether a session started for `started` reaches the machine `host` names.
fn same_address(started: &RemoteHostRecord, host: &RemoteHostRecord) -> bool {
    (&started.ssh_target, &started.cli_path) == (&host.ssh_target, &host.cli_path)
}

/// The one registered SSH host with this name.
fn resolve(local: &HostCtx, name: &str) -> Result<RemoteHostRecord, AppError> {
    let mut hosts: Vec<RemoteHostRecord> = local
        .store
        .get_all_remote_hosts()
        .map_err(AppError::db)?
        .into_iter()
        .filter(|host| host.name == name)
        .collect();
    match hosts.len() {
        1 => Ok(hosts.remove(0)),
        0 => Err(AppError::not_found(format!(
            "No SSH host named {name} is registered on this computer"
        ))),
        n => Err(AppError::invalid_input(format!(
            "{n} SSH hosts are named {name}; rename them so the name picks one"
        ))),
    }
}

/// Start `serve --stdio` there, explaining a host that cannot answer.
fn connect(cli: &CliCommand, host: &RemoteHostRecord) -> Result<RemoteSession, AppError> {
    let name = &host.name;
    RemoteSession::spawn(cli, host, Arc::new(NoopEvents)).map_err(|error| match error.kind {
        ErrorKind::Network => AppError::network(format!(
            "{name} is unreachable, so nothing was read or changed there. {}",
            error.message
        )),
        ErrorKind::RemoteVersionMismatch => AppError {
            message: format!(
                "{} Nothing was installed or updated on {name}. To update it, connect to \
                 {name} in the Agents Manager app, which installs this version there; if \
                 {name} runs a newer version, update this app instead.",
                error.message
            ),
            ..error
        },
        _ => error,
    })
}
