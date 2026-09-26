//! The real `skills-manager-cli serve --stdio`, driven the way the app drives
//! a remote host: through `RemoteSession`. Each server gets its own temp base
//! dir and temp HOME, so nothing here reads or writes this machine's library
//! or agent folders. Unix only, like the remote hosts themselves.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_lib::core::error::ErrorKind;
use app_lib::core::host::HostEvents;
use app_lib::core::remote_host;
use app_lib::core::remote_session::RemoteSession;
use app_lib::core::skill_store::RemoteHostRecord;
use serde_json::{json, Value};
use tempfile::TempDir;

const CLI: &str = env!("CARGO_BIN_EXE_skills-manager-cli");

#[derive(Default)]
struct Recorded(Mutex<Vec<(String, Value)>>);

impl HostEvents for Recorded {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

fn host(ssh_target: &str, cli_path: Option<String>) -> RemoteHostRecord {
    RemoteHostRecord {
        id: "test-host".into(),
        name: "test host".into(),
        ssh_target: ssh_target.into(),
        cli_path,
        created_at: 0,
    }
}

struct Server {
    session: RemoteSession,
    events: Arc<Recorded>,
    tmp: TempDir,
}

fn start() -> Server {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let base = tmp.path().join("base");
    let cli = move |_: &RemoteHostRecord, args: &[&str]| {
        let mut cmd = Command::new(CLI);
        cmd.arg("--base-dir")
            .arg(&base)
            .args(args)
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME");
        cmd
    };
    let events = Arc::new(Recorded::default());
    let session = RemoteSession::spawn(&cli, &host("unused", None), events.clone()).unwrap();
    Server {
        session,
        events,
        tmp,
    }
}

fn spawn_in_isolated_home(tmp: &TempDir) -> RemoteSession {
    let home = tmp.path().join("home");
    let config = tmp.path().join("config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let cli = move |_: &RemoteHostRecord, args: &[&str]| {
        let mut cmd = Command::new(CLI);
        cmd.args(args)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env_remove("XDG_DATA_HOME");
        cmd
    };
    RemoteSession::spawn(&cli, &host("unused", None), Arc::new(Recorded::default())).unwrap()
}

fn run_in_isolated_home(tmp: &TempDir, args: &[&str]) -> Output {
    Command::new(CLI)
        .arg("--json")
        .args(args)
        .env("HOME", tmp.path().join("home"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap()
}

fn cli_json(tmp: &TempDir, args: &[&str]) -> Value {
    let output = run_in_isolated_home(tmp, args);
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
async fn answers_the_app_commands_for_its_own_library() {
    let server = start();
    let info = server.session.info();
    assert_eq!(info.host_id, "test-host");
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(Path::new(&info.base_dir), server.tmp.path().join("base"));
    assert_eq!(Path::new(&info.home), server.tmp.path().join("home"));

    let tools = server
        .session
        .call("get_tool_status", json!({}))
        .await
        .unwrap();
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["key"] == "claude_code"),
        "{tools}"
    );

    let created = server
        .session
        .call(
            "create_preset",
            json!({ "name": "Work", "description": null }),
        )
        .await
        .unwrap();
    let presets = server.session.call("get_presets", json!({})).await.unwrap();
    assert!(
        presets
            .as_array()
            .unwrap()
            .iter()
            .any(|preset| preset["id"] == created["id"] && preset["name"] == "Work"),
        "{presets}"
    );

    let err = server
        .session
        .call("get_skill_document", json!({ "skillId": "no-such-skill" }))
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "Skill not found");
}

#[tokio::test]
async fn progress_events_arrive_tagged_with_the_host() {
    let server = start();
    let folder = server.tmp.path().join("import");
    for name in ["alpha", "beta"] {
        let dir = folder.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test\n---\n"),
        )
        .unwrap();
    }

    let result = server
        .session
        .call(
            "batch_import_folder",
            json!({ "folderPath": folder.to_string_lossy() }),
        )
        .await
        .unwrap();
    assert_eq!(result["imported"], 2);

    // Events are written before the reply, and the reader forwards them in
    // order, so they are all here once the call returns.
    let recorded = server.events.0.lock().unwrap();
    let progress: Vec<&Value> = recorded
        .iter()
        .filter(|(event, _)| event == "batch-import-progress")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(progress.len(), 2, "{recorded:?}");
    for (n, payload) in progress.iter().enumerate() {
        assert_eq!(payload["host_id"], "test-host");
        assert_eq!(payload["current"], n as u64 + 1);
        assert_eq!(payload["total"], 2);
    }
}

#[tokio::test]
async fn remote_library_move_waits_for_serve_and_reconnect_preserves_data() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let old_base = home.join(".skills-manager");
    let new_base = tmp.path().join("relocated-library");
    let source = tmp.path().join("source/demo");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: demo\ndescription: relocation fixture\n---\nbody\n",
    )
    .unwrap();

    let session = spawn_in_isolated_home(&tmp);
    assert_eq!(Path::new(&session.info().base_dir), old_base);
    let imported = session
        .call(
            "batch_import_folder",
            json!({ "folderPath": source.parent().unwrap().to_string_lossy() }),
        )
        .await
        .unwrap();
    assert_eq!(imported["imported"], 1);

    let configured = session
        .call(
            "set_central_repo_path",
            json!({ "path": new_base.to_string_lossy() }),
        )
        .await
        .unwrap();
    assert_eq!(configured, new_base.to_string_lossy().as_ref());
    let active = session
        .call("get_central_repo_path", json!({}))
        .await
        .unwrap();
    assert_eq!(active, old_base.to_string_lossy().as_ref());
    let pending = session
        .call("get_central_repo_pending_path", json!({}))
        .await
        .unwrap();
    assert_eq!(pending, new_base.to_string_lossy().as_ref());
    assert!(old_base.join("skills-manager.db").is_file());
    assert!(!new_base.exists());

    // A normal status command may share the running library but must not
    // perform a move merely because a pending path exists.
    let status = cli_json(&tmp, &["repo", "status"]);
    assert_eq!(status["base_dir"], old_base.to_string_lossy().as_ref());
    assert!(!new_base.exists());

    // Even the explicit move command must leave a library alone while the
    // serving process holds its shared lifetime lease.
    let status = cli_json(&tmp, &["repo", "set-path", new_base.to_str().unwrap()]);
    assert_eq!(status["base_dir"], old_base.to_string_lossy().as_ref());
    assert!(!new_base.exists());

    drop(session);
    // A plain CLI status still must not migrate after the serving lease ends.
    let status = cli_json(&tmp, &["repo", "status"]);
    assert_eq!(status["base_dir"], old_base.to_string_lossy().as_ref());
    assert_eq!(
        status["pending_base_dir"],
        new_base.to_string_lossy().as_ref()
    );
    assert!(!new_base.exists());

    let reconnected = spawn_in_isolated_home(&tmp);
    assert_eq!(Path::new(&reconnected.info().base_dir), new_base);
    let active = reconnected
        .call("get_central_repo_path", json!({}))
        .await
        .unwrap();
    assert_eq!(active, new_base.to_string_lossy().as_ref());
    let pending = reconnected
        .call("get_central_repo_pending_path", json!({}))
        .await
        .unwrap();
    assert!(
        pending.is_null(),
        "pending path should clear after reconnect: {pending}"
    );
    let skills = reconnected
        .call("get_managed_skills", json!({}))
        .await
        .unwrap();
    assert!(
        skills
            .as_array()
            .unwrap()
            .iter()
            .any(|skill| skill["name"] == "demo"),
        "relocated database lost its imported skill: {skills}"
    );
    assert!(new_base.join("skills-manager.db").is_file());
    assert!(!old_base.exists());
}

#[test]
fn explicit_cli_repo_set_path_moves_when_no_serve_process_holds_the_lease() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    std::fs::create_dir_all(tmp.path().join("config")).unwrap();
    let source = tmp.path().join("source/demo");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: demo\ndescription: relocation fixture\n---\nbody\n",
    )
    .unwrap();
    let old_base = tmp.path().join("home/.skills-manager");
    let new_base = tmp.path().join("relocated-library");

    cli_json(&tmp, &["repo", "status"]);
    let installed = cli_json(&tmp, &["skills", "install", source.to_str().unwrap()]);
    // InstallReport serializes `name` at its top level.
    assert_eq!(installed["name"], "demo");

    let status = cli_json(&tmp, &["repo", "set-path", new_base.to_str().unwrap()]);
    assert_eq!(status["base_dir"], new_base.to_string_lossy().as_ref());
    assert_eq!(status["skill_count"], 1);
    assert!(new_base.join("skills-manager.db").is_file());
    assert!(!old_base.exists());
}

#[test]
fn closing_the_session_ends_the_server_promptly() {
    let server = start();
    let started = Instant::now();
    drop(server.session);
    // The kill fallback only fires after 2 s; finishing sooner means the
    // server exited by itself when its stdin closed.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
}

/// The full transport against this machine. Needs `ssh localhost` to work
/// non-interactively (key in `authorized_keys`, host key trusted); when it
/// does not, the test reports itself as skipped. The CLI path is a wrapper
/// that keeps the server on a temp HOME and base dir.
#[tokio::test]
async fn ssh_localhost_round_trip() {
    let reachable = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "--",
            "localhost",
            "true",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !reachable {
        eprintln!("skipped: ssh localhost is not available");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let wrapper = tmp.path().join("cli.sh");
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', r#"'\''"#));
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nHOME={} exec {} --base-dir {} \"$@\"\n",
            quote(&home),
            quote(Path::new(CLI)),
            quote(&tmp.path().join("base")),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let host = host("localhost", Some(wrapper.to_string_lossy().into_owned()));
    let events = Arc::new(Recorded::default());
    let session = RemoteSession::spawn(&remote_host::cli_command, &host, events).unwrap();
    assert_eq!(Path::new(&session.info().home), home);

    let presets = session.call("get_presets", json!({})).await.unwrap();
    assert!(presets.is_array());
}
