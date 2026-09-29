use crate::core::error::AppError;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Output};

use super::{git_output, git_output_allow_failure};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PrPreview {
    pub repository: String,
    pub head: String,
    pub base: String,
    pub bases: Vec<String>,
    pub existing_url: Option<String>,
}

pub(crate) fn pr_preview(
    root: &Path,
    remote_url: &str,
    requested_base: Option<&str>,
) -> Result<PrPreview, AppError> {
    pr_preview_with(
        root,
        remote_url,
        requested_base,
        &SystemGh,
        &SystemProjectGit,
    )
}

pub(crate) fn create_pr(
    root: &Path,
    remote_url: &str,
    head: &str,
    base: &str,
    title: &str,
) -> Result<String, AppError> {
    create_pr_with(
        root,
        remote_url,
        head,
        base,
        title,
        &SystemGh,
        &SystemProjectGit,
    )
}

fn pr_preview_with(
    root: &Path,
    remote_url: &str,
    requested_base: Option<&str>,
    gh: &impl GhRunner,
    git: &impl ProjectGitRunner,
) -> Result<PrPreview, AppError> {
    let repository = github_repository(remote_url)?;
    ensure_authenticated(gh)?;
    let head = current_branch(root, git)?;
    ensure_pushed_tip(root, remote_url, &head, git)?;
    let bases = list_bases(gh, &repository)?;
    let base = match requested_base {
        Some(base) if bases.iter().any(|candidate| candidate == base) => base.to_owned(),
        Some(_) => {
            return Err(AppError::invalid_input(
                "The selected base branch no longer exists on GitHub. Refresh the pull request review.",
            ));
        }
        None => bases.first().cloned().ok_or_else(|| {
            AppError::git("GitHub returned no branches for the pull request base")
        })?,
    };
    if head == base {
        return Err(AppError::invalid_input(
            "The pull request branch must differ from its base branch.",
        ));
    }
    let existing_url = find_open_pr(gh, &repository, &head, &base)?;
    Ok(PrPreview {
        repository,
        head,
        base,
        bases,
        existing_url,
    })
}

fn create_pr_with(
    root: &Path,
    remote_url: &str,
    head: &str,
    base: &str,
    title: &str,
    gh: &impl GhRunner,
    git: &impl ProjectGitRunner,
) -> Result<String, AppError> {
    let repository = github_repository(remote_url)?;
    if title.trim().is_empty() {
        return Err(AppError::invalid_input("A pull request title is required."));
    }
    ensure_authenticated(gh)?;
    let current = current_branch(root, git)?;
    if current != head {
        return Err(AppError::invalid_input(
            "The current branch changed. Refresh the Git review before creating a pull request.",
        ));
    }
    if head == base {
        return Err(AppError::invalid_input(
            "The pull request branch must differ from its base branch.",
        ));
    }
    ensure_pushed_tip(root, remote_url, head, git)?;
    let bases = list_bases(gh, &repository)?;
    if !bases.iter().any(|candidate| candidate == base) {
        return Err(AppError::invalid_input(
            "The selected base branch no longer exists on GitHub. Refresh the pull request review.",
        ));
    }
    if let Some(url) = find_open_pr(gh, &repository, head, base)? {
        return Ok(url);
    }

    create_after_confirmed_absent(gh, &repository, head, base, title)
}

fn create_after_confirmed_absent(
    gh: &impl GhRunner,
    repository: &str,
    head: &str,
    base: &str,
    title: &str,
) -> Result<String, AppError> {
    let args = [
        "pr", "create", "--repo", repository, "--head", head, "--base", base, "--title", title,
        "--body", "",
    ];
    match gh.run(&args) {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if valid_pr_url(&value, repository) {
                Ok(value)
            } else {
                find_open_pr(gh, repository, head, base)?.ok_or_else(|| {
                    AppError::git(
                        "GitHub accepted the pull request request but returned no verifiable pull request URL. Refresh the pull request review before retrying.",
                    )
                })
            }
        }
        _ => {
            // A network timeout can happen after GitHub created the PR. Look it up before
            // reporting failure so the caller never offers a blind create retry.
            find_open_pr(gh, repository, head, base)?.ok_or_else(|| {
                AppError::git(
                    "GitHub could not confirm whether the pull request was created. Check the repository's pull requests, then refresh before retrying.",
                )
            })
        }
    }
}

fn current_branch(root: &Path, git: &impl ProjectGitRunner) -> Result<String, AppError> {
    let output = git.run_git(
        root,
        &args(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
        false,
    )?;
    let branch = String::from_utf8(output.stdout)
        .map_err(|_| AppError::git("Git returned a non-UTF-8 branch name."))?
        .trim()
        .to_owned();
    if branch.is_empty() {
        return Err(AppError::invalid_input(
            "Create or check out a feature branch before creating a pull request.",
        ));
    }
    Ok(branch)
}

fn ensure_pushed_tip(
    root: &Path,
    remote_url: &str,
    head: &str,
    git: &impl ProjectGitRunner,
) -> Result<(), AppError> {
    let local = git.run_git(
        root,
        &args(&["rev-parse", "--verify", "HEAD^{commit}"]),
        false,
    )?;
    let local_tip = String::from_utf8_lossy(&local.stdout).trim().to_owned();
    let reference = format!("refs/heads/{head}");
    let remote = git.run_git(
        root,
        &args(&["ls-remote", "--heads", remote_url, &reference]),
        true,
    )?;
    if !remote.status.success() {
        return Err(AppError::network(
            "Could not verify the pushed branch on the selected GitHub remote. Check network access and credentials, then refresh the pull request review.",
        ));
    }
    let matches = String::from_utf8_lossy(&remote.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, name)| *name == reference)
        .map(|(sha, _)| sha.to_owned())
        .collect::<Vec<_>>();
    if matches.len() != 1 || matches[0] != local_tip {
        return Err(AppError::invalid_input(
            "Push the current branch to this GitHub remote before creating a pull request. The remote branch must point to the current commit.",
        ));
    }
    Ok(())
}

fn list_bases(gh: &impl GhRunner, repository: &str) -> Result<Vec<String>, AppError> {
    let default = gh_call(
        gh,
        &[
            "api",
            &format!("repos/{repository}"),
            "--jq",
            ".default_branch",
        ],
        "read the repository's default branch",
    )?
    .trim()
    .to_owned();
    let output = gh_call(
        gh,
        &[
            "api",
            "--paginate",
            &format!("repos/{repository}/branches"),
            "--jq",
            ".[].name",
        ],
        "list repository branches",
    )?;
    let mut bases = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    bases.sort();
    bases.dedup();
    if let Some(index) = bases.iter().position(|branch| branch == &default) {
        bases.swap(0, index);
    }
    if bases.is_empty() {
        return Err(AppError::git(
            "GitHub returned no branches for this repository.",
        ));
    }
    Ok(bases)
}

#[derive(Deserialize)]
struct PullRequestRow {
    url: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "isCrossRepository")]
    is_cross_repository: bool,
}

fn find_open_pr(
    gh: &impl GhRunner,
    repository: &str,
    head: &str,
    base: &str,
) -> Result<Option<String>, AppError> {
    let output = gh_call(
        gh,
        &[
            "pr",
            "list",
            "--repo",
            repository,
            "--state",
            "open",
            "--head",
            head,
            "--base",
            base,
            "--json",
            "url,headRefName,baseRefName,isCrossRepository",
            "--limit",
            "100",
        ],
        "look up an existing pull request",
    )?;
    let rows: Vec<PullRequestRow> = serde_json::from_str(&output)
        .map_err(|_| AppError::git("GitHub returned an invalid pull request lookup response."))?;
    let Some(row) = rows.into_iter().find(|row| {
        row.head_ref_name == head && row.base_ref_name == base && !row.is_cross_repository
    }) else {
        return Ok(None);
    };
    if !valid_pr_url(&row.url, repository) {
        return Err(AppError::git(
            "GitHub returned an invalid URL for the matching pull request.",
        ));
    }
    Ok(Some(row.url))
}

fn ensure_authenticated(gh: &impl GhRunner) -> Result<(), AppError> {
    match gh.run(&["auth", "status", "--hostname", "github.com"]) {
        Ok(output) if output.status.success() => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(AppError::not_found(
            "GitHub CLI (gh) is not installed on this host. Install it and run `gh auth login` to enable pull requests.",
        )),
        _ => Err(AppError::invalid_input(
            "GitHub CLI is not authenticated on this host. Run `gh auth login` to enable pull requests.",
        )),
    }
}

fn gh_call(gh: &impl GhRunner, args: &[&str], operation: &str) -> Result<String, AppError> {
    let output = gh.run(args).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            AppError::not_found(
                "GitHub CLI (gh) is not installed on this host. Install it and run `gh auth login` to enable pull requests.",
            )
        } else {
            AppError::git(format!("Could not {operation} using GitHub CLI."))
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("not logged") || stderr.contains("authentication") {
            return Err(AppError::invalid_input(
                "GitHub CLI is not authenticated on this host. Run `gh auth login` to enable pull requests.",
            ));
        }
        return Err(AppError::git(format!(
            "Could not {operation} using GitHub CLI. Check repository access and network connectivity, then refresh the pull request review."
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| AppError::git("GitHub CLI returned a non-UTF-8 response."))
}

fn github_repository(remote_url: &str) -> Result<String, AppError> {
    let path = if let Some(path) = remote_url.strip_prefix("https://github.com/") {
        if has_forbidden_url_character(path) {
            return Err(invalid_remote());
        }
        path
    } else if let Some(path) = remote_url.strip_prefix("ssh://git@github.com/") {
        if has_forbidden_url_character(path) {
            return Err(invalid_remote());
        }
        path
    } else if let Some(path) = remote_url.strip_prefix("git@github.com:") {
        if has_forbidden_url_character(path) {
            return Err(invalid_remote());
        }
        path
    } else {
        return Err(invalid_remote());
    };
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segments = path.split('/');
    let (Some(owner), Some(name), None) = (segments.next(), segments.next(), segments.next())
    else {
        return Err(invalid_remote());
    };
    if !valid_repo_segment(owner) || !valid_repo_segment(name) {
        return Err(invalid_remote());
    }
    Ok(format!("{owner}/{name}"))
}

fn has_forbidden_url_character(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(character, '?' | '#' | '@'))
}

fn valid_repo_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

fn invalid_remote() -> AppError {
    AppError::invalid_input(
        "Pull requests require a GitHub HTTPS or SSH remote that identifies one owner/repository.",
    )
}

fn valid_pr_url(value: &str, repository: &str) -> bool {
    let prefix = format!("https://github.com/{repository}/pull/");
    value.strip_prefix(&prefix).is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

trait GhRunner {
    fn run(&self, args: &[&str]) -> io::Result<Output>;
}

trait ProjectGitRunner {
    fn run_git(
        &self,
        root: &Path,
        args: &[OsString],
        allow_failure: bool,
    ) -> Result<Output, AppError>;
}

struct SystemProjectGit;

impl ProjectGitRunner for SystemProjectGit {
    fn run_git(
        &self,
        root: &Path,
        args: &[OsString],
        allow_failure: bool,
    ) -> Result<Output, AppError> {
        let args = args.iter().map(OsString::as_os_str);
        if allow_failure {
            git_output_allow_failure(root, args)
        } else {
            git_output(root, args)
        }
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

struct SystemGh;

impl GhRunner for SystemGh {
    fn run(&self, args: &[&str]) -> io::Result<Output> {
        Command::new("gh")
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1")
            .args(args)
            .output()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct FakeGh(Mutex<VecDeque<(Vec<String>, io::Result<Output>)>>);

    impl FakeGh {
        fn new(responses: Vec<(Vec<&str>, Output)>) -> Self {
            Self(Mutex::new(
                responses
                    .into_iter()
                    .map(|(args, output)| {
                        (args.into_iter().map(str::to_owned).collect(), Ok(output))
                    })
                    .collect(),
            ))
        }
    }

    impl GhRunner for FakeGh {
        fn run(&self, args: &[&str]) -> io::Result<Output> {
            let (expected, response) = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected gh call");
            assert_eq!(args, expected);
            response
        }
    }

    struct FakeGit(Mutex<VecDeque<(Vec<String>, bool, Output)>>);

    impl FakeGit {
        fn new(responses: Vec<(Vec<&str>, bool, Output)>) -> Self {
            Self(Mutex::new(
                responses
                    .into_iter()
                    .map(|(args, allow_failure, output)| {
                        (
                            args.into_iter().map(str::to_owned).collect(),
                            allow_failure,
                            output,
                        )
                    })
                    .collect(),
            ))
        }
    }

    impl ProjectGitRunner for FakeGit {
        fn run_git(
            &self,
            _root: &Path,
            args: &[OsString],
            allow_failure: bool,
        ) -> Result<Output, AppError> {
            let (expected, expected_allow_failure, output) = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected git call");
            assert_eq!(
                args.iter()
                    .map(|arg| arg.to_string_lossy().to_string())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(allow_failure, expected_allow_failure);
            Ok(output)
        }
    }

    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code as u32)
    }

    #[test]
    fn parses_only_unambiguous_github_remotes() {
        assert_eq!(
            github_repository("https://github.com/owner/repo.git").unwrap(),
            "owner/repo"
        );
        assert_eq!(
            github_repository("ssh://git@github.com/owner/repo").unwrap(),
            "owner/repo"
        );
        assert_eq!(
            github_repository("git@github.com:owner/repo.git").unwrap(),
            "owner/repo"
        );
        for remote in [
            "https://user:secret@github.com/owner/repo.git",
            "https://github.com/owner/repo?token=secret",
            "https://github.com/owner/repo/extra",
            "https://github.example.com/owner/repo",
        ] {
            assert!(github_repository(remote).is_err());
        }
    }

    #[test]
    fn missing_cli_and_missing_auth_have_actionable_errors() {
        let auth_args = vec![
            "auth".to_owned(),
            "status".to_owned(),
            "--hostname".to_owned(),
            "github.com".to_owned(),
        ];
        let missing = FakeGh(Mutex::new(VecDeque::from([(
            auth_args.clone(),
            Err(io::Error::from(io::ErrorKind::NotFound)),
        )])));
        assert!(ensure_authenticated(&missing)
            .unwrap_err()
            .message
            .contains("not installed"));

        let unauthenticated = FakeGh::new(vec![(
            vec!["auth", "status", "--hostname", "github.com"],
            output(1, "", "not logged in"),
        )]);
        assert!(ensure_authenticated(&unauthenticated)
            .unwrap_err()
            .message
            .contains("gh auth login"));
    }

    #[test]
    fn lookup_failure_is_not_treated_as_no_existing_pr() {
        let gh = FakeGh::new(vec![(
            vec![
                "pr",
                "list",
                "--repo",
                "owner/repo",
                "--state",
                "open",
                "--head",
                "topic",
                "--base",
                "main",
                "--json",
                "url,headRefName,baseRefName,isCrossRepository",
                "--limit",
                "100",
            ],
            output(1, "", "network error"),
        )]);
        let error = find_open_pr(&gh, "owner/repo", "topic", "main").unwrap_err();
        assert!(error.message.contains("network connectivity"));
    }

    #[test]
    fn parses_existing_pr_by_exact_head_and_base() {
        let gh = FakeGh::new(vec![(
            vec![
                "pr",
                "list",
                "--repo",
                "owner/repo",
                "--state",
                "open",
                "--head",
                "topic",
                "--base",
                "release",
                "--json",
                "url,headRefName,baseRefName,isCrossRepository",
                "--limit",
                "100",
            ],
            output(
                0,
                r#"[{"url":"https://github.com/owner/repo/pull/5","headRefName":"topic","baseRefName":"release","isCrossRepository":false}]"#,
                "",
            ),
        )]);
        assert_eq!(
            find_open_pr(&gh, "owner/repo", "topic", "release")
                .unwrap()
                .as_deref(),
            Some("https://github.com/owner/repo/pull/5")
        );
    }

    #[test]
    fn matching_pr_with_invalid_url_is_a_lookup_error() {
        let gh = FakeGh::new(vec![(
            vec![
                "pr",
                "list",
                "--repo",
                "owner/repo",
                "--state",
                "open",
                "--head",
                "topic",
                "--base",
                "main",
                "--json",
                "url,headRefName,baseRefName,isCrossRepository",
                "--limit",
                "100",
            ],
            output(
                0,
                r#"[{"url":"https://example.com/not-a-pr","headRefName":"topic","baseRefName":"main","isCrossRepository":false}]"#,
                "",
            ),
        )]);
        let error = find_open_pr(&gh, "owner/repo", "topic", "main").unwrap_err();
        assert!(error.message.contains("invalid URL"));
    }

    #[test]
    fn preview_uses_current_pushed_head_and_returns_existing_pr() {
        let gh = FakeGh::new(vec![
            (
                vec!["auth", "status", "--hostname", "github.com"],
                output(0, "", ""),
            ),
            (
                vec!["api", "repos/owner/repo", "--jq", ".default_branch"],
                output(0, "main\n", ""),
            ),
            (
                vec![
                    "api",
                    "--paginate",
                    "repos/owner/repo/branches",
                    "--jq",
                    ".[].name",
                ],
                output(0, "main\ntopic\n", ""),
            ),
            (
                vec![
                    "pr",
                    "list",
                    "--repo",
                    "owner/repo",
                    "--state",
                    "open",
                    "--head",
                    "topic",
                    "--base",
                    "main",
                    "--json",
                    "url,headRefName,baseRefName,isCrossRepository",
                    "--limit",
                    "100",
                ],
                output(
                    0,
                    r#"[{"url":"https://github.com/owner/repo/pull/8","headRefName":"topic","baseRefName":"main","isCrossRepository":false}]"#,
                    "",
                ),
            ),
        ]);
        let git = FakeGit::new(vec![
            (
                vec!["symbolic-ref", "--quiet", "--short", "HEAD"],
                false,
                output(0, "topic\n", ""),
            ),
            (
                vec!["rev-parse", "--verify", "HEAD^{commit}"],
                false,
                output(0, "abc123\n", ""),
            ),
            (
                vec![
                    "ls-remote",
                    "--heads",
                    "https://github.com/owner/repo.git",
                    "refs/heads/topic",
                ],
                true,
                output(0, "abc123\trefs/heads/topic\n", ""),
            ),
        ]);
        let preview = pr_preview_with(
            Path::new("/project"),
            "https://github.com/owner/repo.git",
            Some("main"),
            &gh,
            &git,
        )
        .unwrap();
        assert_eq!(preview.repository, "owner/repo");
        assert_eq!(preview.head, "topic");
        assert_eq!(
            preview.existing_url.as_deref(),
            Some("https://github.com/owner/repo/pull/8")
        );
        assert!(gh.0.lock().unwrap().is_empty());
        assert!(git.0.lock().unwrap().is_empty());
    }

    #[test]
    fn uncertain_create_is_followed_by_lookup_before_error() {
        let gh = FakeGh::new(vec![
            (
                vec!["auth", "status", "--hostname", "github.com"],
                output(0, "", ""),
            ),
            (
                vec!["api", "repos/owner/repo", "--jq", ".default_branch"],
                output(0, "main\n", ""),
            ),
            (
                vec![
                    "api",
                    "--paginate",
                    "repos/owner/repo/branches",
                    "--jq",
                    ".[].name",
                ],
                output(0, "main\ntopic\n", ""),
            ),
            (
                vec![
                    "pr",
                    "list",
                    "--repo",
                    "owner/repo",
                    "--state",
                    "open",
                    "--head",
                    "topic",
                    "--base",
                    "main",
                    "--json",
                    "url,headRefName,baseRefName,isCrossRepository",
                    "--limit",
                    "100",
                ],
                output(0, "[]", ""),
            ),
            (
                vec![
                    "pr",
                    "create",
                    "--repo",
                    "owner/repo",
                    "--head",
                    "topic",
                    "--base",
                    "main",
                    "--title",
                    "Title",
                    "--body",
                    "",
                ],
                output(1, "", "connection lost"),
            ),
            (
                vec![
                    "pr",
                    "list",
                    "--repo",
                    "owner/repo",
                    "--state",
                    "open",
                    "--head",
                    "topic",
                    "--base",
                    "main",
                    "--json",
                    "url,headRefName,baseRefName,isCrossRepository",
                    "--limit",
                    "100",
                ],
                output(0, "[]", ""),
            ),
        ]);
        let git = FakeGit::new(vec![
            (
                vec!["symbolic-ref", "--quiet", "--short", "HEAD"],
                false,
                output(0, "topic\n", ""),
            ),
            (
                vec!["rev-parse", "--verify", "HEAD^{commit}"],
                false,
                output(0, "abc123\n", ""),
            ),
            (
                vec![
                    "ls-remote",
                    "--heads",
                    "https://github.com/owner/repo.git",
                    "refs/heads/topic",
                ],
                true,
                output(0, "abc123\trefs/heads/topic\n", ""),
            ),
        ]);
        let error = create_pr_with(
            Path::new("/project"),
            "https://github.com/owner/repo.git",
            "topic",
            "main",
            "Title",
            &gh,
            &git,
        )
        .unwrap_err();
        assert!(error
            .message
            .contains("Check the repository's pull requests"));
        assert!(gh.0.lock().unwrap().is_empty());
        assert!(git.0.lock().unwrap().is_empty());
    }
}
