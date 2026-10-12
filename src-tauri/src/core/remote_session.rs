//! The client side of `serve --stdio`: one long-lived process (normally
//! `ssh … skills-manager-cli serve --stdio`) that answers the app's
//! host-scoped commands for another machine. See `core::serve` for the
//! protocol.
//!
//! Only the active host has a session, except during a switch: the old one
//! stays open until the new one is ready. Calls never connect: when the link
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

/// The events a host may raise in this app. Anything else a remote sends is
/// dropped, so it cannot pose as an app-level event such as a close request.
const FORWARDED_EVENTS: &[&str] = &[
    "app-files-changed",
    "install-progress",
    "batch-import-progress",
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
}

#[derive(Default)]
struct Slot {
    session: Option<Arc<RemoteSession>>,
    connecting: Option<Connecting>,
    attempts: u64,
}

impl Slot {
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

/// The app's one active session. Its lock is never held while connecting,
/// so a disconnect never waits behind a slow or unreachable host.
pub struct RemoteSessions {
    store: Arc<SkillStore>,
    events: Arc<dyn HostEvents>,
    cli: Arc<CliCommand>,
    slot: Arc<Mutex<Slot>>,
}

impl RemoteSessions {
    pub fn new(store: Arc<SkillStore>, events: Arc<dyn HostEvents>) -> Self {
        Self::with_cli(store, events, Arc::new(remote_host::cli_command))
    }

    fn with_cli(store: Arc<SkillStore>, events: Arc<dyn HostEvents>, cli: Arc<CliCommand>) -> Self {
        Self {
            store,
            events,
            cli,
            slot: Arc::default(),
        }
    }

    /// The live session for `host_id`, connecting when there is none.
    /// Parallel callers share one connect and its result. Asking for a host
    /// abandons a connect to any other. The current session stays open until
    /// a new one is ready, so a failed switch leaves it in place.
    pub async fn session_for(&self, host_id: &str) -> Result<Arc<RemoteSession>, AppError> {
        let done = {
            let mut slot = lock(&self.slot);
            if slot
                .connecting
                .as_ref()
                .is_some_and(|connecting| connecting.host_id != host_id)
            {
                slot.connecting = None;
            }
            if let Some(session) = slot.live(host_id) {
                return Ok(session);
            }
            match slot.connecting_to(host_id) {
                Some(done) => done,
                None => self.start_connect(&mut slot, host_id),
            }
        };
        outcome(done).await
    }

    /// Run one command on `host_id`. A call never connects: it uses the live
    /// session or waits for one connecting, and fails at once otherwise, so
    /// a late call for a host the app has left cannot take the session back.
    pub async fn call(&self, host_id: &str, cmd: &str, args: Value) -> Result<Value, AppError> {
        let (live, connecting) = {
            let slot = lock(&self.slot);
            (slot.live(host_id), slot.connecting_to(host_id))
        };
        let session = match (live, connecting) {
            (Some(session), _) => session,
            (None, Some(done)) => outcome(done).await?,
            (None, None) => return Err(AppError::network("This host is not connected")),
        };
        session.call(cmd, args).await
    }

    /// Connect to `host_id` on a blocking thread, abandoning any other
    /// connect in flight, and return what to wait on for the result.
    fn start_connect(&self, slot: &mut Slot, host_id: &str) -> watch::Receiver<Option<Outcome>> {
        slot.attempts += 1;
        let attempt = slot.attempts;
        let (done, waiting) = watch::channel(None);
        slot.connecting = Some(Connecting {
            host_id: host_id.to_string(),
            attempt,
            done,
        });

        let store = self.store.clone();
        let events = self.events.clone();
        let cli = self.cli.clone();
        let shared = self.slot.clone();
        let host_id = host_id.to_string();
        tokio::task::spawn_blocking(move || {
            let outcome = connect(&store, &*cli, &host_id, events.clone()).map(Arc::new);
            let mut slot = lock(&shared);
            if slot.connecting.as_ref().map(|c| c.attempt) != Some(attempt) {
                // Abandoned: dropping `outcome` closes its session.
                return;
            }
            let connecting = slot.connecting.take().expect("checked above");
            let replaced = match &outcome {
                Ok(session) => slot.session.replace(session.clone()),
                Err(_) => None,
            };
            drop(slot);
            if outcome.is_ok() {
                events.emit(
                    SESSION_STATE_EVENT,
                    json!({ "host_id": host_id, "state": "connected" }),
                );
            }
            let _ = connecting.done.send(Some(outcome));
            if let Some(old) = replaced {
                old.close();
            }
        });
        waiting
    }

    /// Close the session and abandon a connect in flight, failing its
    /// callers at once.
    pub async fn disconnect(&self) {
        if let Some(session) = self.take() {
            let _ = tokio::task::spawn_blocking(move || session.close()).await;
        }
    }

    /// Close the session from a non-async context such as app exit.
    pub fn close_now(&self) {
        if let Some(session) = self.take() {
            session.close();
        }
    }

    fn take(&self) -> Option<Arc<RemoteSession>> {
        let mut slot = lock(&self.slot);
        slot.connecting = None;
        slot.session.take()
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

fn connect(
    store: &SkillStore,
    cli: &CliCommand,
    host_id: &str,
    events: Arc<dyn HostEvents>,
) -> Result<RemoteSession, AppError> {
    let host = store
        .get_remote_host(host_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::not_found("Remote host not found"))?;
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

    /// Sessions over hosts `h1` and `h2`, reached through `cli`.
    fn sessions(
        cli: impl Fn(&RemoteHostRecord, &[&str]) -> Command + Send + Sync + 'static,
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
        let sessions = RemoteSessions::with_cli(Arc::new(store), events.clone(), Arc::new(cli));
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
        assert!(lock(&sessions.slot).session.is_none());
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
        assert!(lock(&sessions.slot).connecting.is_none());

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
