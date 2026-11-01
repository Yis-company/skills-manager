//! The client side of `serve --stdio`: one long-lived process (normally
//! `ssh … skills-manager-cli serve --stdio`) that answers the app's
//! host-scoped commands for another machine. See `core::serve` for the
//! protocol.
//!
//! The viewed host and hosts with live operation state retain one session each.
//! Idle off-screen sessions close after a successful switch. Calls never connect: when the link
//! drops, or a call hears nothing from the host for 30 s, pending calls fail,
//! a `remote-session-state` event tells the UI, and the UI reconnects.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};

use super::error::AppError;
use super::host::{HostEvents, NoopEvents};
use super::remote_host;
use super::remote_install;
use super::serve::{Hello, HelloLine, PROTOCOL};
use super::skill_store::{RemoteHostRecord, SkillStore};

const HELLO_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a host may stay silent while a call waits. A busy host is not
/// silent: the server says it is alive every 10 s (see `core::serve`).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// Enough of the remote's stderr to classify a failure before hello.
const STDERR_KEPT: usize = 16 * 1024;

pub const SESSION_STATE_EVENT: &str = "remote-session-state";

const SERVE: &[&str] = &["serve", "--stdio"];

/// Builds the command that runs the CLI with some arguments on a host:
/// [`remote_host::cli_command`] (over ssh) in the app, a stand-in in tests.
pub type CliCommand = dyn Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync;

/// Installs this app's CLI on a host and returns its updated row:
/// [`remote_install::install_cli`] in the app, a stand-in in tests.
pub type InstallCli =
    dyn Fn(&SkillStore, RemoteHostRecord) -> Result<RemoteHostRecord, AppError> + Send + Sync;

/// The events a host may raise in this app. Anything else a remote sends is
/// dropped, so it cannot pose as an app-level event such as a close request.
const FORWARDED_EVENTS: &[&str] = &[
    "app-files-changed",
    "install-progress",
    "batch-import-progress",
    "operation-changed",
];

/// What the UI is told about a connected host.
#[derive(Debug, Clone, Serialize)]
pub struct HostSessionInfo {
    pub host_id: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub home: String,
    pub base_dir: String,
}

type Reply = Result<Value, AppError>;

struct Calls {
    pending: HashMap<u64, oneshot::Sender<Reply>>,
    /// The stream ended; every later call fails at once.
    ended: bool,
    /// We are closing it ourselves, so its end is not news to the UI.
    closing: bool,
    /// When the host last said anything.
    heard: Instant,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Incoming {
    Event {
        event: String,
        #[serde(default)]
        payload: Value,
    },
    Reply {
        id: Option<u64>,
        ok: bool,
        #[serde(default)]
        result: Value,
        error: Option<AppError>,
    },
}

pub struct RemoteSession {
    info: HostSessionInfo,
    host_name: String,
    stdin: Mutex<Option<ChildStdin>>,
    child: Mutex<Child>,
    calls: Arc<Mutex<Calls>>,
    next_id: AtomicU64,
    timeout: Duration,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Who answered a start: a server with its hello, or a CLI from before
/// `serve` that can only say its version.
enum Answer {
    Serving(Box<RemoteSession>, Hello),
    Older(String),
}

impl RemoteSession {
    /// Start `serve --stdio` on the host and wait for its hello, refusing a
    /// server of another version. Blocks for up to 20 s, so async callers
    /// run it on a blocking thread.
    pub fn spawn(
        cli: &CliCommand,
        host: &RemoteHostRecord,
        events: Arc<dyn HostEvents>,
    ) -> Result<Self, AppError> {
        match Self::start(cli, host, events)? {
            Answer::Serving(session, hello) => {
                // On a mismatch `session` is dropped here, which closes it.
                check_hello(&hello, &host.name)?;
                Ok(*session)
            }
            Answer::Older(version) => Err(version_mismatch(&host.name, &version)),
        }
    }

    /// Start the server, read which version answers and close it again. A
    /// different version is an answer here, not an error.
    pub fn handshake(cli: &CliCommand, host: &RemoteHostRecord) -> Result<String, AppError> {
        Ok(match Self::start(cli, host, Arc::new(NoopEvents))? {
            Answer::Serving(_session, hello) => hello.version,
            Answer::Older(version) => version,
        })
    }

    fn start(
        cli: &CliCommand,
        host: &RemoteHostRecord,
        events: Arc<dyn HostEvents>,
    ) -> Result<Answer, AppError> {
        let mut cmd = cli(host, SERVE);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                AppError::io(format!(
                    "Cannot start {}: {e}",
                    cmd.get_program().to_string_lossy()
                ))
            })?;
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let stderr = drain_stderr(child.stderr.take().expect("stderr is piped"), &host.name);
        let stdin = child.stdin.take();

        let calls = Arc::new(Mutex::new(Calls {
            pending: HashMap::new(),
            ended: false,
            closing: false,
            heard: Instant::now(),
        }));
        let (hello_tx, hello_rx) = mpsc::channel();
        let reader = Reader {
            calls: calls.clone(),
            events,
            host_id: host.id.clone(),
            host_name: host.name.clone(),
        };
        thread::spawn(move || reader.run(stdout, hello_tx));

        let hello = match hello_rx.recv_timeout(HELLO_TIMEOUT) {
            Ok(hello) => hello,
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::network(format!(
                    "Skills Manager on {} did not answer within {} s",
                    host.name,
                    HELLO_TIMEOUT.as_secs()
                )));
            }
            // The output ended before any hello: ssh, the shell or the CLI
            // failed, and said why on stderr.
            Err(RecvTimeoutError::Disconnected) => {
                drop(stdin);
                let code = child.wait().ok().and_then(|status| status.code());
                let stderr = stderr.join().unwrap_or_default();
                // A CLI from before `serve` can still say which version it is.
                if remote_host::predates_serve(code, &stderr) {
                    if let Some(version) = cli_version(cli, host) {
                        return Ok(Answer::Older(version));
                    }
                }
                return Err(remote_host::classify_failure(host, code, &stderr));
            }
        };

        let session = Self {
            info: HostSessionInfo {
                host_id: host.id.clone(),
                version: hello.version.clone(),
                os: hello.os.clone(),
                arch: hello.arch.clone(),
                home: hello.home.clone(),
                base_dir: hello.base_dir.clone(),
            },
            host_name: host.name.clone(),
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            calls,
            next_id: AtomicU64::new(1),
            timeout: REQUEST_TIMEOUT,
        };
        Ok(Answer::Serving(Box::new(session), hello))
    }

    pub fn info(&self) -> &HostSessionInfo {
        &self.info
    }

    pub fn is_alive(&self) -> bool {
        let calls = lock(&self.calls);
        !calls.ended && !calls.closing
    }

    /// Run one host-scoped command there. A command may run long, as an
    /// install does, but the host must not fall silent: after 30 s without a
    /// word the call fails and the link is dropped, as if it had broken.
    pub async fn call(&self, cmd: &str, args: Value) -> Result<Value, AppError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = oneshot::channel();
        {
            let mut calls = lock(&self.calls);
            if calls.ended || calls.closing {
                return Err(self.lost());
            }
            calls.pending.insert(id, tx);
        }
        if let Err(err) = self.send(&json!({ "id": id, "cmd": cmd, "args": args })) {
            log::warn!("Cannot send {cmd} to {}: {err}", self.host_name);
            lock(&self.calls).pending.remove(&id);
            return Err(self.lost());
        }
        // Silence counts from the request, then from each word the host says.
        let mut since = Instant::now();
        loop {
            let deadline = tokio::time::Instant::from_std(since + self.timeout);
            if let Ok(reply) = tokio::time::timeout_at(deadline, &mut rx).await {
                return reply.unwrap_or_else(|_| Err(self.lost()));
            }
            let heard = lock(&self.calls).heard;
            if heard <= since {
                break;
            }
            since = heard;
        }
        log::warn!(
            "{} said nothing for {cmd}; dropping the link",
            self.host_name
        );
        lock(&self.calls).pending.remove(&id);
        // Killing the server ends the reader, which fails the other calls
        // and tells the UI the link dropped.
        let _ = lock(&self.child).kill();
        Err(AppError::network(format!(
            "{} did not answer within {} s",
            self.host_name,
            self.timeout.as_secs()
        )))
    }

    fn send(&self, message: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        let mut stdin = lock(&self.stdin);
        let pipe = stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "session closed"))?;
        pipe.write_all(&line)?;
        pipe.flush()
    }

    fn lost(&self) -> AppError {
        lost(&self.host_name)
    }

    /// Close stdin so the server exits on its own; kill it if it is still
    /// running after 2 s. Safe to call more than once.
    pub fn close(&self) {
        lock(&self.calls).closing = true;
        drop(lock(&self.stdin).take());
        let mut child = lock(&self.child);
        let deadline = Instant::now() + EXIT_GRACE;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Ok(Some(_)) | Err(_) => return,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn lost(host_name: &str) -> AppError {
    AppError::network(format!("Connection to {host_name} was lost"))
}

/// Both sides must run the same build: identical code means identical data.
pub(crate) fn check_hello(hello: &Hello, host_name: &str) -> Result<(), AppError> {
    if hello.version != env!("CARGO_PKG_VERSION") {
        return Err(version_mismatch(host_name, &hello.version));
    }
    if hello.protocol != PROTOCOL {
        return Err(AppError::invalid_input(format!(
            "Skills Manager on {host_name} speaks protocol {}; this app speaks {PROTOCOL}.",
            hello.protocol
        )));
    }
    Ok(())
}

fn version_mismatch(host_name: &str, remote: &str) -> AppError {
    AppError {
        kind: super::error::ErrorKind::RemoteVersionMismatch,
        message: format!(
            "Skills Manager on {host_name} is {remote}; this app is {}. Both must run the same version.",
            env!("CARGO_PKG_VERSION")
        ),
        details: None,
    }
}

/// What the CLI says it is: `skills-manager-cli 1.39.0` gives `1.39.0`.
fn cli_version(cli: &CliCommand, host: &RemoteHostRecord) -> Option<String> {
    let output = cli(host, &["--version"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.split_whitespace().last().map(str::to_string)
}

/// Keep the remote's stderr drained (a full pipe would stall it), log it,
/// and hand back the start of it for classifying a failed connect.
fn drain_stderr(stderr: impl Read + Send + 'static, host_name: &str) -> thread::JoinHandle<String> {
    let host_name = host_name.to_string();
    thread::spawn(move || {
        let mut kept = String::new();
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            log::warn!("{host_name} stderr: {line}");
            if kept.len() < STDERR_KEPT {
                kept.push_str(&line);
                kept.push('\n');
            }
        }
        kept
    })
}

struct Reader {
    calls: Arc<Mutex<Calls>>,
    events: Arc<dyn HostEvents>,
    host_id: String,
    host_name: String,
}

impl Reader {
    fn run(self, stdout: impl BufRead, hello: mpsc::Sender<Hello>) {
        let mut lines = stdout.split(b'\n');
        // A login script may print before the server starts; skip to hello.
        let mut greeted = false;
        for line in lines.by_ref() {
            let Ok(line) = line else { break };
            match serde_json::from_slice::<HelloLine>(&line) {
                Ok(line) => {
                    greeted = hello.send(line.hello).is_ok();
                    break;
                }
                Err(_) => log::warn!(
                    "{} printed before hello: {}",
                    self.host_name,
                    String::from_utf8_lossy(&line)
                ),
            }
        }
        drop(hello);
        if greeted {
            for line in lines {
                let Ok(line) = line else { break };
                lock(&self.calls).heard = Instant::now();
                self.route(&line);
            }
        }
        self.ended(greeted);
    }

    fn route(&self, line: &[u8]) {
        match serde_json::from_slice::<Incoming>(line) {
            Ok(Incoming::Event { event, payload }) => {
                if FORWARDED_EVENTS.contains(&event.as_str()) {
                    self.events.emit(&event, tag_host(payload, &self.host_id));
                }
            }
            Ok(Incoming::Reply {
                id: Some(id),
                ok,
                result,
                error,
            }) => {
                let reply = if ok {
                    Ok(result)
                } else {
                    Err(error.unwrap_or_else(|| AppError::internal("Remote error without details")))
                };
                if let Some(tx) = lock(&self.calls).pending.remove(&id) {
                    let _ = tx.send(reply);
                }
            }
            Ok(Incoming::Reply {
                id: None, error, ..
            }) => {
                log::warn!("{} rejected a request line: {error:?}", self.host_name);
            }
            Err(err) => log::warn!(
                "{} sent an unreadable line ({err}): {}",
                self.host_name,
                String::from_utf8_lossy(line)
            ),
        }
    }

    fn ended(&self, greeted: bool) {
        let pending = {
            let mut calls = lock(&self.calls);
            calls.ended = true;
            // Said under the lock, before any call fails: nothing can
            // reconnect until it is out, so the UI never hears of this drop
            // after hearing of a newer connect.
            if greeted && !calls.closing {
                self.events.emit(
                    SESSION_STATE_EVENT,
                    json!({
                        "host_id": self.host_id,
                        "state": "disconnected",
                        "message": lost(&self.host_name).message,
                    }),
                );
            }
            std::mem::take(&mut calls.pending)
        };
        for (_, tx) in pending {
            let _ = tx.send(Err(lost(&self.host_name)));
        }
    }
}

/// Mark an event with the host it came from, so the UI can drop events from
/// a host it has already switched away from.
fn tag_host(payload: Value, host_id: &str) -> Value {
    match payload {
        Value::Object(mut map) => {
            map.insert("host_id".into(), host_id.into());
            Value::Object(map)
        }
        Value::Null => json!({ "host_id": host_id }),
        other => json!({ "host_id": host_id, "payload": other }),
    }
}

type Outcome = Result<Arc<RemoteSession>, AppError>;

/// A connect in flight. Every caller for its host waits for this one.
/// Dropping it (a disconnect, or a switch to another host) fails them all at
/// once, and the session it would have made is closed when it arrives.
struct Connecting {
    host_id: String,
    attempt: u64,
    done: watch::Sender<Option<Outcome>>,
    select: bool,
    recovery: bool,
}

#[derive(Default)]
struct Slot {
    session: Option<Arc<RemoteSession>>,
    connecting: Option<Connecting>,
    calls: usize,
    mutation_calls: HashMap<String, (String, Option<String>)>,
    operations: HashMap<String, (u64, bool)>,
    uncertain: Vec<(String, Option<String>)>,
    awaiting_refresh: bool,
}

impl Slot {
    fn unselect(&mut self) {
        match self.connecting.as_mut() {
            Some(connecting) if connecting.recovery => connecting.select = false,
            _ => self.connecting = None,
        }
    }

    /// The session for `host_id`, if its link is up.
    fn live(&self, host_id: &str) -> Option<Arc<RemoteSession>> {
        self.session
            .as_ref()
            .filter(|session| session.info.host_id == host_id && session.is_alive())
            .cloned()
    }

    /// What to wait on while `host_id` connects.
    fn connecting_to(&self, host_id: &str) -> Option<watch::Receiver<Option<Outcome>>> {
        self.connecting
            .as_ref()
            .filter(|connecting| connecting.host_id == host_id)
            .map(|connecting| connecting.done.subscribe())
    }
}

#[derive(Default)]
struct Sessions {
    hosts: HashMap<String, Slot>,
    viewed: Option<String>,
    attempts: u64,
}

impl Slot {
    fn needed(&self) -> bool {
        self.calls > 0
            || !self.uncertain.is_empty()
            || self.awaiting_refresh
            || self.operations.values().any(|(_, needed)| *needed)
    }

    fn observe(&mut self, value: &Value, host_id: &str) {
        if let Some(snapshots) = value.as_array() {
            for snapshot in snapshots {
                self.observe(snapshot, host_id);
            }
            return;
        }
        if value.get("host_id").is_some() && value["host_id"].as_str() != Some(host_id) {
            return;
        }
        let (Some(id), Some(revision), Some(settled)) = (
            value["operation_id"].as_str(),
            value["revision"].as_u64(),
            value["settled"].as_bool(),
        ) else {
            return;
        };
        if self
            .operations
            .get(id)
            .is_some_and(|(old, _)| *old > revision)
        {
            return;
        }
        if settled
            && value["items"]
                .as_array()
                .is_some_and(|items| items.iter().all(|item| item["phase"] != "outcome_unknown"))
        {
            self.uncertain.retain(|(operation, attempt)| {
                operation != id
                    || attempt
                        .as_deref()
                        .is_some_and(|attempt| value["attempt_id"].as_str() != Some(attempt))
            });
        }
        // Failed/review/unknown results and Git previews still own live retry evidence.
        let needed = !settled
            || value["items"].as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    matches!(
                        item["phase"].as_str(),
                        Some("failed" | "needs_review" | "outcome_unknown")
                    )
                })
            });
        // A read-only destination check may settle the intent's same revision.
        // An old active reply at that revision cannot make verified work active again.
        if self.operations.get(id).is_some_and(|(old, was_needed)| {
            *old == revision && (!*was_needed || *was_needed == needed)
        }) {
            return;
        }
        self.awaiting_refresh |= settled;
        self.operations.insert(id.to_owned(), (revision, needed));
    }
}

impl Sessions {
    fn release_idle(&mut self) -> Vec<Arc<RemoteSession>> {
        let idle: Vec<_> = self
            .hosts
            .iter()
            .filter(|(id, slot)| {
                self.viewed.as_ref() != Some(*id) && slot.connecting.is_none() && !slot.needed()
            })
            .map(|(id, _)| id.clone())
            .collect();
        idle.into_iter()
            .filter_map(|id| self.hosts.remove(&id).and_then(|slot| slot.session))
            .collect()
    }
}

fn close_sessions(sessions: Vec<Arc<RemoteSession>>) {
    if !sessions.is_empty() {
        tokio::task::spawn_blocking(move || {
            for session in sessions {
                session.close();
            }
        });
    }
}

/// The app's viewed session and retained operation sessions. Its lock is never held while connecting,
/// so a disconnect never waits behind a slow or unreachable host.
pub struct RemoteSessions {
    store: Arc<SkillStore>,
    events: Arc<dyn HostEvents>,
    cli: Arc<CliCommand>,
    install: Arc<InstallCli>,
    slots: Arc<Mutex<Sessions>>,
}

impl RemoteSessions {
    pub fn new(store: Arc<SkillStore>, events: Arc<dyn HostEvents>) -> Self {
        Self::with_cli(
            store,
            events,
            Arc::new(remote_host::cli_command),
            Arc::new(|store: &SkillStore, host| remote_install::install_cli(store, host, |_| {})),
        )
    }

    fn with_cli(
        store: Arc<SkillStore>,
        events: Arc<dyn HostEvents>,
        cli: Arc<CliCommand>,
        install: Arc<InstallCli>,
    ) -> Self {
        Self {
            store,
            events,
            cli,
            install,
            slots: Arc::default(),
        }
    }

    /// Select a host, sharing its live connection. A failed switch preserves the view;
    /// a superseded connect cannot select itself when its hello eventually arrives.
    pub async fn session_for(&self, host_id: &str) -> Result<Arc<RemoteSession>, AppError> {
        let done = {
            let mut slots = lock(&self.slots);
            for (id, slot) in &mut slots.hosts {
                if id != host_id {
                    slot.unselect();
                }
            }
            let slot = slots.hosts.entry(host_id.to_owned()).or_default();
            if let Some(session) = slot.live(host_id) {
                slots.viewed = Some(host_id.to_owned());
                close_sessions(slots.release_idle());
                return Ok(session);
            }
            match slot.connecting_to(host_id) {
                Some(done) => {
                    slot.connecting.as_mut().unwrap().select = true;
                    done
                }
                None => self.start_connect(&mut slots, host_id, true),
            }
        };
        outcome(done).await
    }

    /// Reconnect operation evidence on its original host without selecting it.
    /// The observer releases the temporary refresh hold after reconciliation.
    pub async fn reconnect(&self, host_id: &str) -> Result<Arc<RemoteSession>, AppError> {
        let done = {
            let mut slots = lock(&self.slots);
            let slot = slots.hosts.entry(host_id.to_owned()).or_default();
            slot.awaiting_refresh = true;
            if let Some(session) = slot.live(host_id) {
                return Ok(session);
            }
            match slot.connecting_to(host_id) {
                Some(done) => {
                    slot.connecting.as_mut().unwrap().recovery = true;
                    done
                }
                None => self.start_connect(&mut slots, host_id, false),
            }
        };
        outcome(done).await
    }

    /// Calls never connect. Capture the host and hold its session through the reply,
    /// including the interval before a submission's admission snapshot is known.
    pub async fn call(&self, host_id: &str, cmd: &str, args: Value) -> Result<Value, AppError> {
        let mutation = cmd == "operation_request"
            && matches!(
                args["request"]["action"].as_str(),
                Some(
                    "submit_install"
                        | "submit_project_update"
                        | "confirm_install"
                        | "retry_failed"
                        | "cancel"
                )
            );
        let correlation = (
            args["request"]["operation_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            args["request"]["attempt_id"].as_str().map(str::to_owned),
        );
        let call_id = uuid::Uuid::new_v4().to_string();
        let (live, connecting) = {
            let slots = lock(&self.slots);
            let slot = slots
                .hosts
                .get(host_id)
                .ok_or_else(|| AppError::network("This host is not connected"))?;
            (slot.live(host_id), slot.connecting_to(host_id))
        };
        let session = match (live, connecting) {
            (Some(session), _) => session,
            (None, Some(done)) => outcome(done).await?,
            (None, None) => return Err(AppError::network("This host is not connected")),
        };
        {
            let mut slots = lock(&self.slots);
            let slot = slots
                .hosts
                .get_mut(host_id)
                .ok_or_else(|| AppError::network("This host is not connected"))?;
            if !slot
                .session
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &session))
            {
                return Err(AppError::network("This host connection changed"));
            }
            slot.calls += 1;
            if mutation {
                slot.mutation_calls
                    .insert(call_id.clone(), correlation.clone());
            }
        }
        let result = session.call(cmd, args).await;
        let mut slots = lock(&self.slots);
        if let Some(slot) = slots.hosts.get_mut(host_id).filter(|slot| {
            slot.session
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &session))
        }) {
            slot.calls -= 1;
            if cmd == "operation_request" {
                if mutation {
                    slot.mutation_calls.remove(&call_id);
                }
                match &result {
                    Ok(value) => slot.observe(value, host_id),
                    // A missing response is not proof that admission or a write failed.
                    Err(error) if mutation && error.kind == super::error::ErrorKind::Network => {
                        slot.uncertain.push(correlation);
                    }
                    Err(_) => {}
                }
            }
        }
        close_sessions(slots.release_idle());
        result
    }

    fn start_connect(
        &self,
        slots: &mut Sessions,
        host_id: &str,
        select: bool,
    ) -> watch::Receiver<Option<Outcome>> {
        slots.attempts += 1;
        let attempt = slots.attempts;
        let (done, waiting) = watch::channel(None);
        slots
            .hosts
            .entry(host_id.to_owned())
            .or_default()
            .connecting = Some(Connecting {
            host_id: host_id.to_string(),
            attempt,
            done,
            select,
            recovery: !select,
        });
        let store = self.store.clone();
        let events = self.events.clone();
        let cli = self.cli.clone();
        let install = self.install.clone();
        let shared = self.slots.clone();
        let host_id = host_id.to_string();
        tokio::task::spawn_blocking(move || {
            let outcome = connect(&store, &*cli, &*install, &host_id, events.clone()).map(Arc::new);
            let mut slots = lock(&shared);
            let Some(slot) = slots.hosts.get_mut(&host_id) else {
                return;
            };
            if slot.connecting.as_ref().map(|c| c.attempt) != Some(attempt) {
                drop(slots);
                return;
            }
            let connecting = slot.connecting.take().expect("checked above");
            let replaced = match &outcome {
                Ok(session) => {
                    // Old futures may observe their failed replies after this hello.
                    // Their counts belong to the old connection; uncertain admissions
                    // remain retained for SKI-23 to reconcile, without replaying them.
                    slot.uncertain
                        .extend(std::mem::take(&mut slot.mutation_calls).into_values());
                    slot.calls = 0;

                    slot.session.replace(session.clone())
                }
                Err(_) => None,
            };
            if outcome.is_ok() && connecting.select {
                slots.viewed = Some(host_id.clone());
            }
            let idle = slots.release_idle();
            drop(slots);
            if outcome.is_ok() {
                events.emit(
                    SESSION_STATE_EVENT,
                    json!({ "host_id": host_id, "state": "connected" }),
                );
            }
            let _ = connecting.done.send(Some(outcome));
            for session in idle.into_iter().chain(replaced) {
                session.close();
            }
        });
        waiting
    }

    /// Select Local. Retain hosts needed by pending calls or operation evidence.
    pub async fn disconnect(&self) {
        let mut slots = lock(&self.slots);
        slots.viewed = None;
        for slot in slots.hosts.values_mut() {
            slot.unselect();
        }
        close_sessions(slots.release_idle());
    }

    /// The observer has refreshed this host's settled results. Release only idle
    /// off-screen connections; a concurrent admission or retry still retains it.
    pub fn release_host(&self, host_id: &str) {
        let mut slots = lock(&self.slots);
        if let Some(slot) = slots.hosts.get_mut(host_id) {
            slot.awaiting_refresh = false;
        }
        close_sessions(slots.release_idle());
    }

    /// Explicit reconnect of one idle host must not tear down another host's work.
    pub async fn disconnect_host(&self, host_id: &str) -> Result<(), AppError> {
        let session = {
            let mut slots = lock(&self.slots);
            if slots.hosts.get(host_id).is_some_and(Slot::needed) {
                return Err(AppError::invalid_input(
                    "This host still has operations that need its connection",
                ));
            }
            slots.hosts.remove(host_id).and_then(|slot| slot.session)
        };
        if let Some(session) = session {
            tokio::task::spawn_blocking(move || session.close()).await?;
        }
        Ok(())
    }

    /// Session-owned hosts for observation after a webview remount, not restart restoration.
    pub fn retained_hosts(&self) -> Vec<String> {
        lock(&self.slots)
            .hosts
            .iter()
            .filter(|(_, slot)| slot.needed())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// App exit remains a separate lifecycle boundary (SKI-25).
    pub fn close_now(&self) {
        let hosts = std::mem::take(&mut lock(&self.slots).hosts);
        for slot in hosts.into_values() {
            if let Some(session) = slot.session {
                session.close();
            }
        }
    }
}

/// Wait for a connect to answer.
async fn outcome(mut done: watch::Receiver<Option<Outcome>>) -> Outcome {
    let outcome = done
        .wait_for(Option::is_some)
        .await
        .map(|outcome| outcome.clone());
    // The connect was abandoned before it answered.
    outcome
        .ok()
        .flatten()
        .unwrap_or_else(|| Err(AppError::cancelled("The connection was cancelled")))
}

/// Connect to the host. When another version answers, install this app's CLI
/// there without asking, unless that would downgrade it, and connect to that.
fn connect(
    store: &SkillStore,
    cli: &CliCommand,
    install: &InstallCli,
    host_id: &str,
    events: Arc<dyn HostEvents>,
) -> Result<RemoteSession, AppError> {
    let host = store
        .get_remote_host(host_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Remote host not found"))?;
    let app = env!("CARGO_PKG_VERSION");
    // Another version's session is dropped, and so closed, before installing.
    let version = match RemoteSession::start(cli, &host, events.clone())? {
        Answer::Serving(session, hello) if hello.version == app => {
            check_hello(&hello, &host.name)?;
            return Ok(*session);
        }
        Answer::Serving(_, hello) => hello.version,
        Answer::Older(version) => version,
    };
    remote_install::check_install(&host.name, app, Some(&version), None)?;
    let host = install(store, host)?;
    events.emit(remote_install::HOSTS_CHANGED_EVENT, Value::Null);
    RemoteSession::spawn(cli, &host, events)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::core::error::ErrorKind;
    use crate::core::host::RecordingEvents;
    use std::sync::atomic::AtomicUsize;
    use tempfile::TempDir;

    const VERSION: &str = env!("CARGO_PKG_VERSION");

    fn host() -> RemoteHostRecord {
        RemoteHostRecord {
            id: "h1".into(),
            name: "build box".into(),
            ssh_target: "me@build".into(),
            cli_path: None,
            created_at: 0,
        }
    }

    fn hello_line(version: &str) -> String {
        json!({ "hello": {
            "protocol": PROTOCOL, "version": version, "os": "linux", "arch": "x86_64",
            "home": "/home/me", "base_dir": "/home/me/.skills-manager", "pid": 1,
        }})
        .to_string()
    }

    /// A stand-in server: `sh -c SCRIPT sh HELLO`.
    fn fake_server(script: &str, hello: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script, "sh", hello]);
        cmd
    }

    /// A CLI that runs the stand-in server whatever it is asked.
    fn fake_cli(
        script: &str,
        hello: &str,
    ) -> impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static {
        let (script, hello) = (script.to_string(), hello.to_string());
        move |_: &RemoteHostRecord, _: &[&str]| fake_server(&script, &hello)
    }

    /// A 1.39.0 CLI, which has no `serve` yet.
    fn older_cli(_: &RemoteHostRecord, args: &[&str]) -> Command {
        match args {
            ["--version"] => fake_server("echo skills-manager-cli 1.39.0", ""),
            _ => fake_server(
                "echo \"error: unrecognized subcommand 'serve'\" >&2; exit 2",
                "",
            ),
        }
    }

    /// Sessions over hosts `h1` and `h2`, reached through `cli`, that never
    /// install a CLI.
    fn sessions(
        cli: impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static,
    ) -> (RemoteSessions, Arc<RecordingEvents>, TempDir) {
        sessions_installing(cli, |_, _| {
            Err(AppError::internal("this test installs nothing"))
        })
    }

    fn sessions_installing(
        cli: impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static,
        install: impl Fn(&SkillStore, RemoteHostRecord) -> Result<RemoteHostRecord, AppError>
            + Send
            + Sync
            + 'static,
    ) -> (RemoteSessions, Arc<RecordingEvents>, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let store = SkillStore::new(&tmp.path().join("test.db")).unwrap();
        for (id, name) in [("h1", "build box"), ("h2", "gpu box")] {
            store
                .insert_remote_host(&RemoteHostRecord {
                    id: id.into(),
                    name: name.into(),
                    ssh_target: format!("me@{id}"),
                    cli_path: None,
                    created_at: 0,
                })
                .unwrap();
        }
        let events = Arc::new(RecordingEvents::default());
        let sessions = RemoteSessions::with_cli(
            Arc::new(store),
            events.clone(),
            Arc::new(cli),
            Arc::new(install),
        );
        (sessions, events, tmp)
    }

    /// A server that answers hello and then waits for its stdin to close.
    fn serving() -> impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static {
        fake_cli(
            r#"printf '%s\n' "$1"; cat >/dev/null"#,
            &hello_line(VERSION),
        )
    }

    #[test]
    fn a_different_version_refuses_the_session() {
        let cli = fake_cli(
            r#"printf '%s\n' "$1"; cat >/dev/null"#,
            &hello_line("0.1.0"),
        );
        let started = Instant::now();
        let err = RemoteSession::spawn(&cli, &host(), Arc::new(NoopEvents))
            .err()
            .expect("a mismatch must refuse");
        assert_eq!(err.kind, ErrorKind::RemoteVersionMismatch);
        assert_eq!(
            err.message,
            format!(
                "Skills Manager on build box is 0.1.0; this app is {}. Both must run the same version.",
                env!("CARGO_PKG_VERSION")
            )
        );
        // Refusing closed stdin, and the stand-in exited on it, well before
        // the kill deadline.
        assert!(started.elapsed() < EXIT_GRACE);
    }

    #[test]
    fn output_before_hello_is_skipped() {
        let cli = fake_cli(
            r#"echo 'Welcome to build box'; printf '%s\n' "$1"; cat >/dev/null"#,
            &hello_line(env!("CARGO_PKG_VERSION")),
        );
        let session = RemoteSession::spawn(&cli, &host(), Arc::new(NoopEvents)).unwrap();
        assert_eq!(session.info().host_id, "h1");
        assert_eq!(session.info().home, "/home/me");
        assert!(session.is_alive());
    }

    #[test]
    fn a_failure_before_hello_is_classified() {
        let cli = fake_cli("echo CLI_NOT_FOUND >&2; exit 3", "");
        let err = RemoteSession::spawn(&cli, &host(), Arc::new(NoopEvents))
            .err()
            .unwrap();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(err.message.contains("build box"), "{}", err.message);
    }

    #[test]
    fn a_handshake_reports_another_version_and_closes() {
        let cli = fake_cli(
            r#"printf '%s\n' "$1"; cat >/dev/null"#,
            &hello_line("0.1.0"),
        );
        let started = Instant::now();
        assert_eq!(RemoteSession::handshake(&cli, &host()).unwrap(), "0.1.0");
        // Closing stdin ended the stand-in well before the kill deadline.
        assert!(started.elapsed() < EXIT_GRACE);
    }

    #[test]
    fn a_handshake_reports_the_version_of_a_cli_from_before_serve() {
        assert_eq!(
            RemoteSession::handshake(&older_cli, &host()).unwrap(),
            "1.39.0"
        );
    }

    #[test]
    fn a_failed_handshake_is_classified() {
        let cli = fake_cli("echo BRIDGE_BROKEN >&2; exit 3", "");
        let err = RemoteSession::handshake(&cli, &host()).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(err.message.contains("republish"), "{}", err.message);
    }

    /// The server reads one request and dies without answering.
    #[tokio::test]
    async fn a_dropped_link_fails_pending_calls_and_says_so() {
        let cli = fake_cli(
            r#"printf '%s\n' "$1"; read line; exit 0"#,
            &hello_line(env!("CARGO_PKG_VERSION")),
        );
        let events = Arc::new(RecordingEvents::default());
        let session = RemoteSession::spawn(&cli, &host(), events.clone()).unwrap();

        let err = session.call("get_presets", json!({})).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Network);
        assert_eq!(err.message, "Connection to build box was lost");
        assert!(!session.is_alive());
        let err = session.call("get_presets", json!({})).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Network);

        let recorded = events.0.lock().unwrap();
        assert_eq!(
            *recorded,
            vec![(
                SESSION_STATE_EVENT.to_string(),
                json!({
                    "host_id": "h1",
                    "state": "disconnected",
                    "message": "Connection to build box was lost",
                })
            )]
        );
    }

    #[test]
    fn a_cli_from_before_serve_is_refused_with_its_version() {
        let err = RemoteSession::spawn(&older_cli, &host(), Arc::new(NoopEvents))
            .err()
            .expect("an older CLI must refuse");
        assert_eq!(err.kind, ErrorKind::RemoteVersionMismatch);
        assert_eq!(
            err.message,
            format!(
                "Skills Manager on build box is 1.39.0; this app is {VERSION}. Both must run the same version."
            )
        );
    }

    /// Five calls arrive while the host is unreachable: one connect runs, and
    /// they all fail together when it does, not one after another.
    #[tokio::test]
    async fn parallel_callers_share_one_failing_connect() {
        let runs = Arc::new(AtomicUsize::new(0));
        let counted = runs.clone();
        let (sessions, _events, _tmp) = sessions(move |_: &RemoteHostRecord, _: &[&str]| {
            counted.fetch_add(1, Ordering::SeqCst);
            fake_server("sleep 0.5; echo CLI_NOT_FOUND >&2; exit 3", "")
        });

        let started = Instant::now();
        let results = tokio::join!(
            sessions.session_for("h1"),
            sessions.session_for("h1"),
            sessions.session_for("h1"),
            sessions.session_for("h1"),
            sessions.session_for("h1"),
        );
        for result in [results.0, results.1, results.2, results.3, results.4] {
            assert_eq!(result.err().unwrap().kind, ErrorKind::NotFound);
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "{:?}",
            started.elapsed()
        );

        // A failure is not remembered: the next call tries again.
        sessions.session_for("h1").await.err().unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    /// The host runs 0.1.0 until this app's CLI is installed there.
    #[tokio::test]
    async fn connect_installs_this_version_over_an_older_cli_without_asking() {
        let installs = Arc::new(AtomicUsize::new(0));
        let counted = installs.clone();
        let (sessions, events, _tmp) = sessions_installing(
            |host: &RemoteHostRecord, _: &[&str]| {
                let version = if host.cli_path.is_some() {
                    VERSION
                } else {
                    "0.1.0"
                };
                fake_server(
                    r#"printf '%s\n' "$1"; cat >/dev/null"#,
                    &hello_line(version),
                )
            },
            move |_, host| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(RemoteHostRecord {
                    cli_path: Some("/home/me/installed".into()),
                    ..host
                })
            },
        );

        let session = sessions.session_for("h1").await.unwrap();
        assert_eq!(session.info().version, VERSION);
        assert_eq!(installs.load(Ordering::SeqCst), 1);
        assert!(events
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(event, _)| event == remote_install::HOSTS_CHANGED_EVENT));
    }

    #[tokio::test]
    async fn connect_asks_to_update_the_app_instead_of_downgrading_a_newer_host() {
        let installs = Arc::new(AtomicUsize::new(0));
        let counted = installs.clone();
        let (sessions, _events, _tmp) = sessions_installing(
            fake_cli(
                r#"printf '%s\n' "$1"; cat >/dev/null"#,
                &hello_line("999.0.0"),
            ),
            move |_, host| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(host)
            },
        );

        let err = sessions.session_for("h1").await.err().unwrap();
        assert_eq!(err.kind, ErrorKind::RemoteVersionMismatch);
        assert!(err.message.contains("Update this app"), "{}", err.message);
        assert_eq!(installs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disconnect_does_not_wait_for_a_connect_in_flight() {
        let (sessions, _events, _tmp) = sessions(fake_cli(
            r#"sleep 1; printf '%s\n' "$1"; cat >/dev/null"#,
            &hello_line(VERSION),
        ));
        let sessions = Arc::new(sessions);
        let waiting = tokio::spawn({
            let sessions = sessions.clone();
            async move { sessions.session_for("h1").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let started = Instant::now();
        sessions.disconnect().await;
        let err = waiting.await.unwrap().err().unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(err.kind, ErrorKind::Cancelled);

        // The abandoned connect finishes later and is closed, not kept.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(lock(&sessions.slots)
            .hosts
            .values()
            .all(|slot| slot.session.is_none()));
    }

    #[tokio::test]
    async fn switching_hosts_closes_the_old_session() {
        let (sessions, _events, _tmp) = sessions(serving());
        let first = sessions.session_for("h1").await.unwrap();
        let again = sessions.session_for("h1").await.unwrap();
        assert!(Arc::ptr_eq(&first, &again));

        let second = sessions.session_for("h2").await.unwrap();
        assert_eq!(second.info().host_id, "h2");
        let deadline = Instant::now() + Duration::from_secs(1);
        while first.is_alive() {
            assert!(Instant::now() < deadline, "the old session is still open");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(second.is_alive());
    }

    #[tokio::test]
    async fn switching_away_from_a_shared_connect_keeps_its_recovery_waiter() {
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        let barrier = tempfile::tempdir().unwrap();
        let release = barrier.path().join("hello");
        let signal = release.clone();
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entered = started.clone();
        let (sessions, _events, _tmp) = sessions(move |host: &RemoteHostRecord, _: &[&str]| {
            if host.id == "h1" {
                entered.store(true, Ordering::SeqCst);
                fake_server(
                    &format!(
                        r#"while ! test -e '{}'; do sleep 0.01; done; printf '%s\n' "$1"; cat >/dev/null"#,
                        signal.display()
                    ),
                    &hello_line(VERSION),
                )
            } else {
                fake_server(
                    r#"printf '%s\n' "$1"; cat >/dev/null"#,
                    &hello_line(VERSION),
                )
            }
        });
        let sessions = Arc::new(sessions);
        let viewed = sessions.session_for("h2").await.unwrap();
        let recovery = {
            let sessions = sessions.clone();
            tokio::spawn(async move { sessions.reconnect("h1").await })
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        let mut selection = Box::pin(sessions.session_for("h1"));
        poll_fn(|cx| {
            assert!(selection.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        sessions.session_for("h2").await.unwrap();
        std::fs::write(release, "hello").unwrap();
        let recovered = recovery.await.unwrap().unwrap();
        selection.await.unwrap();
        sessions.release_host("h1");
        assert!(sessions.retained_hosts().is_empty());
        assert!(viewed.is_alive());
        assert_eq!(
            sessions
                .call("h1", "get_presets", json!({}))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Network
        );
        drop(recovered);
    }

    #[tokio::test]
    async fn recovery_reconnect_preserves_the_view_and_shares_connections() {
        let (sessions, _events, _tmp) = sessions(serving());
        let viewed = sessions.session_for("h2").await.unwrap();
        let (first, second) = tokio::join!(sessions.reconnect("h1"), sessions.reconnect("h1"));
        let first = first.unwrap();
        assert!(Arc::ptr_eq(&first, &second.unwrap()));
        assert!(viewed.is_alive());
        // Releasing recovered idle work must close h1, never the viewed h2.
        sessions.release_host("h1");
        assert_eq!(
            sessions
                .call("h1", "get_presets", json!({}))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Network
        );
        assert!(Arc::ptr_eq(
            &viewed,
            &sessions.session_for("h2").await.unwrap()
        ));
    }

    #[tokio::test]
    async fn active_operations_keep_their_host_session_across_switches() {
        let cli = fake_cli(
            r#"printf '%s\n' "$1"
               read line
               echo '{"id":1,"ok":true,"result":{"operation_id":"same-id","revision":1,"settled":false,"items":[{"phase":"applying"}]}}'
               read line
               echo '{"id":2,"ok":true,"result":{"operation_id":"same-id","revision":2,"settled":true,"items":[{"phase":"succeeded"}]}}'
               read line
               echo '{"id":3,"ok":true,"result":42}'
               cat >/dev/null"#,
            &hello_line(VERSION),
        );
        let (sessions, _events, _tmp) = sessions(cli);
        let first = sessions.session_for("h1").await.unwrap();
        sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"submit_install"}}),
            )
            .await
            .unwrap();
        let second = sessions.session_for("h2").await.unwrap();
        assert!(
            first.is_alive(),
            "switching must retain the operation's host"
        );
        assert!(second.is_alive());
        let result = sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"read","operation_id":"same-id"}}),
            )
            .await
            .unwrap();
        assert_eq!(result["items"][0]["phase"], "succeeded");
        assert_eq!(
            sessions.call("h1", "get_presets", json!({})).await.unwrap(),
            42,
            "settlement must leave the originating host available for its result refresh"
        );
        sessions.release_host("h1");
        let deadline = Instant::now() + Duration::from_secs(2);
        while first.is_alive() {
            assert!(
                Instant::now() < deadline,
                "idle off-screen host was not released"
            );
            tokio::task::yield_now().await;
        }
        assert!(second.is_alive());
    }

    #[tokio::test]
    async fn pending_admission_and_a_slow_host_do_not_block_other_hosts_or_local() {
        let barrier = tempfile::tempdir().unwrap();
        let release = barrier.path().join("release");
        let signal = release.clone();
        let (sessions, events, _tmp) = sessions(move |host: &RemoteHostRecord, _: &[&str]| {
            let script = if host.id == "h1" {
                format!(
                    r#"printf '%s\n' "$1"; read line
                    echo '{{"event":"app-files-changed","payload":{{"stage":"admitting"}}}}'
                    while ! test -e '{}'; do sleep 0.01; done
                    echo '{{"id":1,"ok":true,"result":{{"operation_id":"same-id","revision":1,"settled":false,"items":[{{"phase":"queued"}}]}}}}'
                    cat >/dev/null"#,
                    signal.display()
                )
            } else {
                r#"printf '%s\n' "$1"; read line
                    echo '{"id":1,"ok":true,"result":42}'; cat >/dev/null"#
                    .to_string()
            };
            fake_server(&script, &hello_line(VERSION))
        });
        let sessions = Arc::new(sessions);
        let first = sessions.session_for("h1").await.unwrap();
        let admission = tokio::spawn({
            let sessions = sessions.clone();
            async move {
                sessions
                    .call(
                        "h1",
                        "operation_request",
                        json!({"request":{"action":"submit_install"}}),
                    )
                    .await
            }
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if events
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|(_, payload)| payload["stage"] == "admitting")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the request never reached the stand-in"
            );
            tokio::task::yield_now().await;
        }
        let second = sessions.session_for("h2").await.unwrap();
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(2),
                sessions.call("h2", "get_presets", json!({}))
            )
            .await
            .unwrap()
            .unwrap(),
            42
        );
        sessions.disconnect().await;
        assert!(
            first.is_alive(),
            "an unresolved admission owns its host even on Local"
        );
        assert!(!admission.is_finished());
        std::fs::write(release, "continue").unwrap();
        assert_eq!(admission.await.unwrap().unwrap()["settled"], false);
        assert!(
            first.is_alive(),
            "the admitted operation still needs its original connection"
        );
        assert_eq!(sessions.retained_hosts(), ["h1"]);
        assert!(
            sessions.disconnect_host("h1").await.is_err(),
            "explicit reconnect cannot tear down live work"
        );
        sessions.close_now();
        assert!(!second.is_alive());
    }

    #[tokio::test]
    async fn late_replies_release_reads_but_retain_uncertain_mutations() {
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        for (command, args, uncertain) in [
            ("get_presets", json!({}), false),
            (
                "operation_request",
                json!({"request":{"action":"list"}}),
                false,
            ),
            (
                "operation_request",
                json!({"request":{"action":"submit_install"}}),
                true,
            ),
        ] {
            let barrier = tempfile::tempdir().unwrap();
            let release = barrier.path().join("drop");
            let signal = release.clone();
            let (sessions, events, _tmp) = sessions(move |_: &RemoteHostRecord, _: &[&str]| {
                fake_server(
                    &format!(
                        r#"printf '%s\n' "$1"; read line
                while ! test -e '{}'; do sleep 0.01; done; exit 0"#,
                        signal.display()
                    ),
                    &hello_line(VERSION),
                )
            });
            sessions.session_for("h1").await.unwrap();
            let mut old_call = Box::pin(sessions.call("h1", command, args));
            // Poll admission once, then deliberately leave the reply unobserved until
            // after a replacement session exists. The pipe drop is externally gated.
            poll_fn(|cx| {
                assert!(old_call.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            std::fs::write(release, "drop").unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if events
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(_, payload)| payload["state"] == "disconnected")
                {
                    break;
                }
                assert!(Instant::now() < deadline);
                tokio::task::yield_now().await;
            }
            let replacement = sessions.session_for("h1").await.unwrap();
            old_call.await.unwrap_err();
            sessions.disconnect().await;
            assert_eq!(
                sessions.retained_hosts().is_empty(),
                !uncertain,
                "only a lost mutation can retain unresolved admission evidence"
            );
            if uncertain {
                sessions.close_now();
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while replacement.is_alive() {
                assert!(Instant::now() < deadline);
                tokio::task::yield_now().await;
            }
        }
    }

    #[tokio::test]
    async fn verified_destination_evidence_can_settle_the_same_receipt_revision() {
        let (sessions, _events, _tmp) = sessions(fake_cli(
            r#"printf '%s\n' "$1"
          read line; echo '{"id":1,"ok":true,"result":{"operation_id":"lost","revision":1,"settled":false,"items":[{"phase":"applying"}]}}'
          read line; echo '{"id":2,"ok":true,"result":{"operation_id":"lost","revision":1,"settled":true,"items":[{"phase":"unchanged"}]}}'
          read line; echo '{"id":3,"ok":true,"result":{"operation_id":"lost","revision":1,"settled":false,"items":[{"phase":"applying"}]}}'
          cat >/dev/null"#,
            &hello_line(VERSION),
        ));
        sessions.session_for("h1").await.unwrap();
        sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"submit_install","operation_id":"lost"}}),
            )
            .await
            .unwrap();
        let viewed = sessions.session_for("h2").await.unwrap();
        sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"reconcile","operation_id":"lost"}}),
            )
            .await
            .unwrap();
        sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"list"}}),
            )
            .await
            .unwrap();
        sessions.release_host("h1");
        assert!(sessions.retained_hosts().is_empty());
        assert!(viewed.is_alive());
    }

    #[tokio::test]
    async fn only_a_correlated_verified_result_releases_a_lost_mutation() {
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        let barrier = tempfile::tempdir().unwrap();
        let release = barrier.path().join("drop");
        let signal = release.clone();
        let runs = Arc::new(AtomicUsize::new(0));
        let (sessions, _events, _tmp) = sessions(move |host: &RemoteHostRecord, _: &[&str]| {
            let first = host.id == "h1" && runs.fetch_add(1, Ordering::SeqCst) == 0;
            let dropping = format!(
                r#"printf '%s\n' "$1"; read line; read line; while ! test -e '{}'; do sleep 0.01; done; exit 0"#,
                signal.display()
            );
            fake_server(
                if first {
                    &dropping
                } else {
                    r#"printf '%s\n' "$1"
              read line; echo '{"id":1,"ok":true,"result":{"operation_id":"lost","attempt_id":"old","revision":1,"settled":true,"items":[{"phase":"succeeded"}]}}'
              read line; echo '{"id":2,"ok":true,"result":[]}'
              read line; echo '{"id":3,"ok":true,"result":{"operation_id":"lost","attempt_id":"new","revision":2,"settled":true,"items":[{"phase":"succeeded"}]}}'
              cat >/dev/null"#
                },
                &hello_line(VERSION),
            )
        });
        sessions.session_for("h1").await.unwrap();
        let mut retry = Box::pin(sessions.call(
            "h1",
            "operation_request",
            json!({"request":{"action":"retry_failed","operation_id":"lost","attempt_id":"new"}}),
        ));
        let mut cancel = Box::pin(sessions.call(
            "h1",
            "operation_request",
            json!({"request":{"action":"cancel","operation_id":"lost"}}),
        ));
        poll_fn(|cx| {
            assert!(retry.as_mut().poll(cx).is_pending());
            assert!(cancel.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        std::fs::write(release, "drop").unwrap();
        retry.await.unwrap_err();
        cancel.await.unwrap_err();
        let viewed = sessions.session_for("h2").await.unwrap();
        let recovered = sessions.reconnect("h1").await.unwrap();
        sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"reconcile","operation_id":"other"}}),
            )
            .await
            .unwrap();
        sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"list"}}),
            )
            .await
            .unwrap();
        sessions.release_host("h1");
        assert_eq!(sessions.retained_hosts(), ["h1"]);
        assert!(
            recovered.is_alive(),
            "an unrelated receipt or empty list cannot clear uncertainty"
        );
        sessions
            .call(
                "h1",
                "operation_request",
                json!({"request":{"action":"reconcile","operation_id":"lost"}}),
            )
            .await
            .unwrap();
        sessions.release_host("h1");
        assert!(
            sessions.retained_hosts().is_empty(),
            "verified original work should release its uncertainty hold"
        );
        assert!(viewed.is_alive());
    }

    /// The link drops after one request. Calls fail until the UI connects
    /// again, which says the host is back.
    #[tokio::test]
    async fn after_a_drop_calls_fail_until_the_host_is_connected_again() {
        let (sessions, events, _tmp) = sessions(fake_cli(
            r#"printf '%s\n' "$1"; read line; exit 0"#,
            &hello_line(VERSION),
        ));
        let first = sessions.session_for("h1").await.unwrap();
        sessions
            .call("h1", "get_presets", json!({}))
            .await
            .unwrap_err();

        let err = sessions
            .call("h1", "get_presets", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Network);
        assert!(lock(&sessions.slots)
            .hosts
            .values()
            .all(|slot| slot.connecting.is_none()));

        let second = sessions.session_for("h1").await.unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(second.is_alive());

        let recorded = events.0.lock().unwrap();
        let states: Vec<(&Value, &Value)> = recorded
            .iter()
            .filter(|(event, _)| event == SESSION_STATE_EVENT)
            .map(|(_, payload)| (&payload["host_id"], &payload["state"]))
            .collect();
        assert_eq!(
            states,
            [
                (&json!("h1"), &json!("connected")),
                (&json!("h1"), &json!("disconnected")),
                (&json!("h1"), &json!("connected")),
            ]
        );
    }

    /// `h1` serves; `h2` fails after `delay` seconds.
    fn h2_fails_after(
        delay: &str,
    ) -> impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static {
        let serving = serving();
        let failing = fake_cli(
            &format!("sleep {delay}; echo CLI_NOT_FOUND >&2; exit 3"),
            "",
        );
        move |host: &RemoteHostRecord, args: &[&str]| match host.id.as_str() {
            "h2" => failing(host, args),
            _ => serving(host, args),
        }
    }

    #[tokio::test]
    async fn a_failed_switch_keeps_the_current_session() {
        let (sessions, _events, _tmp) = sessions(h2_fails_after("0"));
        let first = sessions.session_for("h1").await.unwrap();

        sessions.session_for("h2").await.err().unwrap();
        assert!(first.is_alive());
        assert!(Arc::ptr_eq(
            &first,
            &sessions.session_for("h1").await.unwrap()
        ));
    }

    /// Asking for the current host while a switch connects cancels the
    /// switch at once; the current session is kept.
    #[tokio::test]
    async fn asking_for_the_current_host_cancels_a_switch() {
        let (sessions, _events, _tmp) = sessions(h2_fails_after("1"));
        let sessions = Arc::new(sessions);
        let first = sessions.session_for("h1").await.unwrap();
        let switching = tokio::spawn({
            let sessions = sessions.clone();
            async move { sessions.session_for("h2").await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        let started = Instant::now();
        let again = sessions.session_for("h1").await.unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        let err = switching.await.unwrap().err().unwrap();
        assert_eq!(err.kind, ErrorKind::Cancelled);
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(first.is_alive());
    }

    /// A late call for a host the app has switched away from fails, and does
    /// not connect that host again.
    #[tokio::test]
    async fn a_call_for_a_host_left_behind_does_not_take_the_session_back() {
        let runs = Arc::new(AtomicUsize::new(0));
        let counted = runs.clone();
        let serving = serving();
        let (sessions, _events, _tmp) = sessions(move |host: &RemoteHostRecord, args: &[&str]| {
            counted.fetch_add(1, Ordering::SeqCst);
            serving(host, args)
        });
        sessions.session_for("h1").await.unwrap();
        let second = sessions.session_for("h2").await.unwrap();

        let err = sessions
            .call("h1", "get_presets", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Network);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert!(second.is_alive());
        assert!(Arc::ptr_eq(
            &second,
            &sessions.session_for("h2").await.unwrap()
        ));
    }

    /// The server takes a request and falls silent.
    #[tokio::test]
    async fn a_silent_host_times_out_and_counts_as_a_drop() {
        let cli = fake_cli(
            r#"printf '%s\n' "$1"; read line; exec sleep 30"#,
            &hello_line(VERSION),
        );
        let events = Arc::new(RecordingEvents::default());
        let mut session = RemoteSession::spawn(&cli, &host(), events.clone()).unwrap();
        session.timeout = Duration::from_millis(300);

        let started = Instant::now();
        let err = session.call("get_presets", json!({})).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::Network);
        assert!(err.message.contains("did not answer"), "{}", err.message);
        assert!(started.elapsed() < Duration::from_secs(2));

        let deadline = Instant::now() + Duration::from_secs(2);
        while session.is_alive() {
            assert!(Instant::now() < deadline, "the link is still up");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let recorded = events.0.lock().unwrap();
        assert_eq!(recorded[0].0, SESSION_STATE_EVENT);
        assert_eq!(recorded[0].1["state"], "disconnected");
    }

    /// The server works on a request for longer than the timeout but keeps
    /// saying it is alive, then answers.
    #[tokio::test]
    async fn a_busy_host_that_keeps_saying_so_does_not_time_out() {
        let cli = fake_cli(
            r#"printf '%s\n' "$1"; read line
               for i in 1 2 3 4 5 6; do echo '{"event":"keepalive","payload":null}'; sleep 0.1; done
               echo '{"id":1,"ok":true,"result":42}'; cat >/dev/null"#,
            &hello_line(VERSION),
        );
        let mut session = RemoteSession::spawn(&cli, &host(), Arc::new(NoopEvents)).unwrap();
        session.timeout = Duration::from_millis(300);

        assert_eq!(session.call("get_presets", json!({})).await.unwrap(), 42);
        assert!(session.is_alive());
    }

    #[test]
    fn events_carry_their_host() {
        assert_eq!(
            tag_host(json!({ "current": 1 }), "h1"),
            json!({ "current": 1, "host_id": "h1" })
        );
        assert_eq!(tag_host(Value::Null, "h1"), json!({ "host_id": "h1" }));
    }
}
