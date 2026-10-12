//! Sync from the CLI, the way a host's timer runs it. Each test gets its own
//! temp base dir, HOME and bare remote, so nothing here touches this
//! machine's library. Unix only, like `cli_json.rs`.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

const CLI: &str = env!("CARGO_BIN_EXE_skills-manager-cli");

fn cli(tmp: &TempDir, args: &[&str]) {
    let out = Command::new(CLI)
        .arg("--json")
        .arg("--base-dir")
        .arg(tmp.path().join("base"))
        .args(args)
        .env("HOME", tmp.path().join("home"))
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    let remote = tmp.path().join("remote.git");
    git(
        tmp.path(),
        &["init", "--bare", "--initial-branch=main", "remote.git"],
    );
    cli(&tmp, &["git", "init"]);
    cli(&tmp, &["git", "set-remote", remote.to_str().unwrap()]);
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
