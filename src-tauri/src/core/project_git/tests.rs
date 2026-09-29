use super::*;
use std::process::{Command, Output};
use tempfile::TempDir;

struct TestRepo {
    _temp: TempDir,
    path: PathBuf,
    repo: Repo,
}

impl TestRepo {
    fn new(files: &[(&str, &str)]) -> Self {
        let temp = TempDir::new().expect("temporary directory");
        let path = temp.path().join("work tree");
        std::fs::create_dir(&path).unwrap();
        run_git_at(&path, &["init", "-b", "main"]);
        run_git_at(&path, &["config", "user.name", "Project Git Test"]);
        run_git_at(&path, &["config", "user.email", "project-git@example.test"]);
        run_git_at(&path, &["config", "commit.gpgsign", "false"]);
        for (name, contents) in files {
            write(&path, name, contents);
            git(&path, &["add", "--", name]);
        }
        git(&path, &["commit", "-m", "initial"]);
        let repo = Repo::new(&path, vec![]).unwrap();
        Self {
            _temp: temp,
            path,
            repo,
        }
    }

    fn status(&self) -> Status {
        self.repo.status().unwrap()
    }

    fn head(&self) -> String {
        text(git(&self.path, &["rev-parse", "HEAD"]))
    }
}

fn run_git_at(path: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(path: &Path, args: &[&str]) -> Output {
    run_git_at(path, args)
}

fn text(output: Output) -> String {
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn bare_repo(temp: &TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    let output = Command::new("git")
        .args(["init", "--bare", "--", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git init --bare: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    path
}

fn commit_review(repo: &Repo, review_id: &str, paths: &[&str]) -> String {
    repo.commit(
        review_id,
        paths.iter().map(|path| (*path).to_owned()).collect(),
        "reviewed commit",
    )
    .unwrap()
}

#[test]
fn selected_commit_preserves_unrelated_index_and_worktree_changes() {
    let fixture = TestRepo::new(&[
        ("selected.txt", "original selected\n"),
        ("staged.txt", "original staged\n"),
        ("unstaged.txt", "original unstaged\n"),
    ]);
    write(&fixture.path, "selected.txt", "selected update\n");
    write(&fixture.path, "staged.txt", "staged update\n");
    git(&fixture.path, &["add", "--", "staged.txt"]);
    write(&fixture.path, "unstaged.txt", "unstaged update\n");

    let status = fixture.status();
    let json = serde_json::to_value(&status).unwrap();
    assert!(json.get("review_id").is_some());
    assert!(json.get("reviewId").is_none());
    commit_review(&fixture.repo, &status.review_id, &["selected.txt"]);

    assert_eq!(
        text(git(&fixture.path, &["show", "HEAD:selected.txt"])),
        "selected update"
    );
    assert_eq!(
        text(git(&fixture.path, &["show", "HEAD:staged.txt"])),
        "original staged"
    );
    assert_eq!(
        text(git(&fixture.path, &["show", "HEAD:unstaged.txt"])),
        "original unstaged"
    );
    assert_eq!(
        text(git(&fixture.path, &["diff", "--cached", "--name-only"])),
        "staged.txt"
    );
    assert_eq!(
        text(git(&fixture.path, &["diff", "--name-only"])),
        "unstaged.txt"
    );
}

#[cfg(unix)]
#[test]
fn selected_commit_handles_rename_delete_special_paths_and_symlink_entry() {
    use std::os::unix::fs::symlink;

    let fixture = TestRepo::new(&[
        ("rename source.txt", "rename me\n"),
        ("delete me.txt", "delete me\n"),
    ]);
    let external = fixture._temp.path().join("outside target");
    std::fs::write(&external, "must not enter repository").unwrap();
    git(
        &fixture.path,
        &["mv", "--", "rename source.txt", "- renamed file.txt"],
    );
    std::fs::remove_file(fixture.path.join("delete me.txt")).unwrap();
    write(&fixture.path, "new file with spaces.txt", "new text\n");
    symlink(&external, fixture.path.join("link entry")).unwrap();

    let status = fixture.status();
    let renamed = status
        .files
        .iter()
        .find(|file| file.path == "- renamed file.txt")
        .unwrap();
    assert_eq!(renamed.original_path.as_deref(), Some("rename source.txt"));
    commit_review(
        &fixture.repo,
        &status.review_id,
        &[
            "- renamed file.txt",
            "delete me.txt",
            "new file with spaces.txt",
            "link entry",
        ],
    );

    assert_eq!(
        text(git(&fixture.path, &["show", "HEAD:- renamed file.txt"])),
        "rename me"
    );
    let tree_paths = text(git(
        &fixture.path,
        &["ls-tree", "-r", "--name-only", "HEAD"],
    ));
    assert!(!tree_paths.lines().any(|path| path == "rename source.txt"));
    assert!(!tree_paths.lines().any(|path| path == "delete me.txt"));
    assert!(!fixture.path.join("rename source.txt").exists());
    assert!(!fixture.path.join("delete me.txt").exists());
    assert_eq!(
        text(git(
            &fixture.path,
            &["show", "HEAD:new file with spaces.txt"]
        )),
        "new text"
    );
    assert_eq!(
        text(git(&fixture.path, &["cat-file", "blob", "HEAD:link entry"])),
        external.to_string_lossy().to_string()
    );
    assert_eq!(
        std::fs::read_to_string(external).unwrap(),
        "must not enter repository"
    );
}

#[test]
fn partial_staging_and_stale_reviews_are_rejected_before_commit() {
    let fixture = TestRepo::new(&[("file.txt", "base\n")]);
    write(&fixture.path, "file.txt", "staged version\n");
    git(&fixture.path, &["add", "--", "file.txt"]);
    write(&fixture.path, "file.txt", "working version\n");
    let partial = fixture.status();
    let blocked = partial
        .files
        .iter()
        .find(|file| file.path == "file.txt")
        .unwrap();
    assert!(blocked
        .blocked_reason
        .as_deref()
        .unwrap()
        .contains("Partially staged"));
    let head_before = fixture.head();
    assert!(fixture
        .repo
        .commit(&partial.review_id, vec!["file.txt".into()], "partial")
        .is_err());
    assert_eq!(fixture.head(), head_before);
    assert_eq!(
        text(git(&fixture.path, &["show", ":file.txt"])),
        "staged version"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.path.join("file.txt")).unwrap(),
        "working version\n"
    );

    git(&fixture.path, &["reset", "--hard", "HEAD"]);
    write(&fixture.path, "file.txt", "reviewed\n");
    let stale_content = fixture.status();
    write(&fixture.path, "file.txt", "changed after review\n");
    assert!(fixture
        .repo
        .commit(
            &stale_content.review_id,
            vec!["file.txt".into()],
            "stale content"
        )
        .is_err());

    let stale_head = fixture.status();
    write(&fixture.path, "file.txt", "external commit\n");
    git(&fixture.path, &["commit", "-am", "external change"]);
    let head_after_external = fixture.head();
    assert!(fixture
        .repo
        .commit(&stale_head.review_id, vec!["file.txt".into()], "stale head")
        .is_err());
    assert_eq!(fixture.head(), head_after_external);
}

#[test]
fn active_git_operation_blocks_a_reviewed_commit() {
    let fixture = TestRepo::new(&[("file.txt", "base\n")]);
    write(&fixture.path, "file.txt", "change\n");
    let status = fixture.status();
    let merge_head = fixture.path.join(".git").join("MERGE_HEAD");
    std::fs::write(&merge_head, "0123456789012345678901234567890123456789\n").unwrap();

    let blocked = fixture.status();
    assert!(blocked
        .blocked_reason
        .as_deref()
        .unwrap()
        .contains("active Git operation"));
    let head = fixture.head();
    assert!(fixture
        .repo
        .commit(&status.review_id, vec!["file.txt".into()], "during merge")
        .is_err());
    assert_eq!(fixture.head(), head);
    std::fs::remove_file(merge_head).unwrap();
}

#[test]
fn cached_pr_review_rejects_a_changed_push_destination() {
    let fixture = TestRepo::new(&[("file.txt", "base\n")]);
    let old_destination = bare_repo(&fixture._temp, "old-destination.git");
    let new_destination = bare_repo(&fixture._temp, "new-destination.git");
    git(
        &fixture.path,
        &["remote", "add", "origin", old_destination.to_str().unwrap()],
    );
    let review_id = store_review(
        &fixture.path,
        Review::Pr(PrState {
            remote: "origin".into(),
            url: old_destination.to_string_lossy().into_owned(),
            head: fixture.head(),
            branch: "main".into(),
            base: "trunk".into(),
        }),
    );
    git(
        &fixture.path,
        &[
            "config",
            "remote.origin.pushurl",
            new_destination.to_str().unwrap(),
        ],
    );

    let result = fixture.repo.request(Request::CreatePr {
        review_id,
        title: "Should not call gh".into(),
    });
    assert!(result.unwrap_err().message.contains("Git state changed"));
}

#[cfg(unix)]
#[test]
fn commit_hooks_run_and_failure_keeps_unrelated_staging() {
    use std::os::unix::fs::PermissionsExt;

    for (should_fail, expected_success) in [(false, true), (true, false)] {
        let fixture = TestRepo::new(&[
            ("selected.txt", "base selected\n"),
            ("other.txt", "base other\n"),
        ]);
        let hooks = fixture._temp.path().join("hooks");
        std::fs::create_dir(&hooks).unwrap();
        let marker = fixture._temp.path().join("hook ran");
        let exit = if should_fail { "exit 1" } else { "exit 0" };
        let hook = format!("#!/bin/sh\nprintf ran > '{}'\n{}\n", marker.display(), exit);
        let hook_path = hooks.join("pre-commit");
        std::fs::write(&hook_path, hook).unwrap();
        let mut permissions = std::fs::metadata(&hook_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook_path, permissions).unwrap();
        git(
            &fixture.path,
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        );

        write(&fixture.path, "selected.txt", "selected change\n");
        write(&fixture.path, "other.txt", "other staged change\n");
        git(&fixture.path, &["add", "--", "other.txt"]);
        let status = fixture.status();
        let result =
            fixture
                .repo
                .commit(&status.review_id, vec!["selected.txt".into()], "hook test");
        assert_eq!(result.is_ok(), expected_success);
        assert!(marker.exists());
        assert_eq!(
            text(git(&fixture.path, &["diff", "--cached", "--name-only"])),
            "other.txt"
        );
        assert_eq!(
            text(git(&fixture.path, &["show", "HEAD:other.txt"])),
            "base other"
        );
        if should_fail {
            assert_eq!(
                text(git(&fixture.path, &["show", "HEAD:selected.txt"])),
                "base selected"
            );
            assert_eq!(
                std::fs::read_to_string(fixture.path.join("selected.txt")).unwrap(),
                "selected change\n"
            );
        } else {
            assert_eq!(
                text(git(&fixture.path, &["show", "HEAD:selected.txt"])),
                "selected change"
            );
        }
    }
}

#[test]
fn reviewed_push_lists_all_outgoing_commits_sets_upstream_and_rejects_remote_advance() {
    let fixture = TestRepo::new(&[("base.txt", "base\n")]);
    git(&fixture.path, &["switch", "-c", "topic"]);
    write(&fixture.path, "first.txt", "first\n");
    git(&fixture.path, &["add", "--", "first.txt"]);
    git(&fixture.path, &["commit", "-m", "first topic commit"]);
    write(&fixture.path, "second.txt", "second\n");
    git(&fixture.path, &["add", "--", "second.txt"]);
    git(&fixture.path, &["commit", "-m", "second topic commit"]);
    let local_head = fixture.head();
    let bare = bare_repo(&fixture._temp, "origin.git");
    git(
        &fixture.path,
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );

    let preview = fixture.repo.push_preview("origin", "topic").unwrap();
    let json = serde_json::to_value(&preview).unwrap();
    assert!(json.get("review_id").is_some());
    assert!(json.get("reviewId").is_none());
    assert_eq!(preview.commits.len(), 3);
    let outgoing = preview
        .commits
        .iter()
        .map(|commit| commit.id.as_str())
        .collect::<Vec<_>>();
    assert!(outgoing.contains(&local_head.as_str()));
    assert!(outgoing
        .contains(&text(git(&fixture.path, &["rev-list", "--max-parents=0", "HEAD"])).as_str()));
    fixture.repo.push(&preview.review_id).unwrap();
    assert_eq!(
        text(git(
            &fixture.path,
            &["rev-parse", "--abbrev-ref", "@{upstream}"]
        )),
        "origin/topic"
    );

    write(&fixture.path, "local-only.txt", "local only\n");
    git(&fixture.path, &["add", "--", "local-only.txt"]);
    git(&fixture.path, &["commit", "-m", "local advance"]);
    let remote_clone = fixture._temp.path().join("remote clone");
    let cloned = Command::new("git")
        .args([
            "clone",
            bare.to_str().unwrap(),
            remote_clone.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        cloned.status.success(),
        "git clone: {}",
        String::from_utf8_lossy(&cloned.stderr)
    );
    run_git_at(&remote_clone, &["config", "user.name", "Remote Test"]);
    run_git_at(
        &remote_clone,
        &["config", "user.email", "remote@example.test"],
    );
    run_git_at(&remote_clone, &["config", "commit.gpgsign", "false"]);
    git(
        &remote_clone,
        &["switch", "-c", "topic", "--track", "origin/topic"],
    );
    write(&remote_clone, "remote-only.txt", "remote only\n");
    git(&remote_clone, &["add", "--", "remote-only.txt"]);
    git(&remote_clone, &["commit", "-m", "remote advance"]);
    git(&remote_clone, &["push", "origin", "topic"]);
    let remote_head = text(git(&remote_clone, &["rev-parse", "HEAD"]));

    assert!(fixture.repo.push_preview("origin", "topic").is_err());
    assert_eq!(
        text(git(&bare, &["rev-parse", "refs/heads/topic"])),
        remote_head
    );
}

#[test]
fn push_uses_single_configured_pushurl_instead_of_fetch_url() {
    let fixture = TestRepo::new(&[("base.txt", "base\n")]);
    git(&fixture.path, &["switch", "-c", "topic"]);
    write(&fixture.path, "topic.txt", "topic\n");
    git(&fixture.path, &["add", "--", "topic.txt"]);
    git(&fixture.path, &["commit", "-m", "topic"]);
    let fetch_remote = bare_repo(&fixture._temp, "fetch.git");
    let push_remote = bare_repo(&fixture._temp, "push.git");
    git(
        &fixture.path,
        &["remote", "add", "origin", fetch_remote.to_str().unwrap()],
    );
    git(
        &fixture.path,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            push_remote.to_str().unwrap(),
        ],
    );

    let preview = fixture.repo.push_preview("origin", "topic").unwrap();
    assert_eq!(preview.url, push_remote.to_string_lossy().to_string());
    fixture.repo.push(&preview.review_id).unwrap();
    assert_eq!(
        text(git(&push_remote, &["rev-parse", "refs/heads/topic"])),
        fixture.head()
    );
    let absent = Command::new("git")
        .arg("-C")
        .arg(&fetch_remote)
        .args(["show-ref", "--verify", "refs/heads/topic"])
        .output()
        .unwrap();
    assert!(!absent.status.success());
}
