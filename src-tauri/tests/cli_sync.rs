//! Sync from the CLI, the way a host's timer runs it. Each test gets its own
//! temp base dir, HOME and bare remote, so nothing here touches this
//! machine's library. Unix only, like `cli_json.rs`.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const CLI: &str = env!("CARGO_BIN_EXE_skills-manager-cli");

/// A temp base dir and HOME, with an empty bare remote next to them.
fn sandbox() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    git(
        tmp.path(),
        &["init", "--bare", "--initial-branch=main", "remote.git"],
    );
    tmp
}

fn run(tmp: &TempDir, json: bool, args: &[&str]) -> Output {
    let mut command = Command::new(CLI);
    if json {
        command.arg("--json");
    }
    command
        .arg("--base-dir")
        .arg(tmp.path().join("base"))
        .args(args)
        .env("HOME", tmp.path().join("home"))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap()
}

/// The `--json` report of a command that must succeed.
fn cli(tmp: &TempDir, args: &[&str]) -> Value {
    let out = run(tmp, true, args);
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// A library with a Sync remote, set up through the old commands until
/// `sync join` exists.
fn joined(tmp: &TempDir, remote: &Path) {
    cli(tmp, &["git", "init"]);
    cli(tmp, &["git", "set-remote", remote.to_str().unwrap()]);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn pull_finishes_a_sync_this_host_was_interrupted_in() {
    // SID-108: a crash after the branch moved but before the files were
    // written must not be committed as a local edit by the next pull — that
    // would undo the synced change. The CLI recovers first, like the app.
    let tmp = sandbox();
    joined(&tmp, &tmp.path().join("remote.git"));
    cli(&tmp, &["git", "push"]);

    // Crash state: HEAD is the synced commit, the files are still the old ones.
    let skills = tmp.path().join("base/skills");
    let old_head = git(&skills, &["rev-parse", "HEAD"]);
    std::fs::write(skills.join("from-sync.md"), "synced").unwrap();
    git(&skills, &["add", "from-sync.md"]);
    git(
        &skills,
        &["commit", "-m", "sync\n\nSkills-Manager-Protocol: 2"],
    );
    let new_head = git(&skills, &["rev-parse", "HEAD"]);
    git(&skills, &["push", "origin", "main"]);
    git(
        &skills,
        &["update-ref", "refs/skills-manager/pre-merge", &old_head],
    );
    git(
        &skills,
        &["update-ref", "refs/skills-manager/applying", &new_head],
    );
    std::fs::remove_file(skills.join("from-sync.md")).unwrap();

    cli(&tmp, &["git", "pull"]);

    assert_eq!(
        std::fs::read_to_string(skills.join("from-sync.md")).unwrap(),
        "synced"
    );
    assert_eq!(git(&skills, &["show", "HEAD:from-sync.md"]), "synced");
    assert_eq!(
        git(&skills, &["for-each-ref", "refs/skills-manager/applying"]),
        ""
    );
}

#[test]
fn sync_status_reports_the_state_the_app_checks_over_ssh() {
    let tmp = sandbox();
    let status = cli(&tmp, &["sync", "status"]);
    assert_eq!(status["state"], "not_set_up");
    for key in ["host", "version", "format", "conflicts", "edited_here"] {
        assert!(status.get(key).is_some(), "missing {key} in {status}");
    }

    joined(&tmp, &tmp.path().join("remote.git"));
    let synced = cli(&tmp, &["sync", "now"]);
    assert_eq!(synced["pushed"], true);
    let status = cli(&tmp, &["sync", "status"]);
    assert_eq!(status["state"], "up_to_date");
    assert!(status["last_sync"].as_i64().is_some(), "{status}");

    assert_eq!(cli(&tmp, &["sync", "pause"])["state"], "paused");
    assert_eq!(cli(&tmp, &["sync", "resume"])["state"], "up_to_date");
}

#[test]
fn sync_now_exits_1_when_stuck_and_status_says_why() {
    let tmp = sandbox();
    joined(&tmp, &tmp.path().join("missing.git"));

    let out = run(&tmp, true, &["sync", "now"]);
    assert_eq!(out.status.code(), Some(1));
    let envelope: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(envelope["ok"], false);

    let status = cli(&tmp, &["sync", "status"]);
    assert_eq!(status["state"], "needs_review");
    assert!(
        status["reason"]
            .as_str()
            .is_some_and(|r| r.contains("missing.git")),
        "{status}"
    );
}

#[test]
fn old_git_commands_still_work_and_point_to_sync() {
    let tmp = sandbox();
    joined(&tmp, &tmp.path().join("remote.git"));

    let out = run(&tmp, false, &["git", "status"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("use `skills-manager-cli sync status`"),
        "{stderr}"
    );
    // With --json, stderr stays free for a failure envelope.
    assert!(run(&tmp, true, &["git", "status"]).stderr.is_empty());
}
