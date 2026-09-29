//! Project-scoped Git review and write operations.

mod github;

use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::error::AppError;

const MAX_DIFF_BYTES: usize = 128 * 1024;
const MAX_REVIEWS_PER_REPO: usize = 3;
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    Status,
    Commit {
        review_id: String,
        paths: Vec<String>,
        message: String,
    },
    CreateBranch {
        review_id: String,
        name: String,
    },
    PushPreview {
        remote: String,
        branch: String,
    },
    Push {
        review_id: String,
    },
    PrPreview {
        remote: String,
        base: Option<String>,
    },
    CreatePr {
        review_id: String,
        title: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct FileStatus {
    pub path: String,
    pub original_path: Option<String>,
    pub status: String,
    pub skill_related: bool,
    pub blocked_reason: Option<String>,
    pub diff: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub review_id: String,
    pub root: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub blocked_reason: Option<String>,
    pub files: Vec<FileStatus>,
    pub remotes: Vec<Remote>,
    pub upstream_remote: Option<String>,
    pub upstream_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushReview {
    pub review_id: String,
    pub remote: String,
    pub branch: String,
    pub url: String,
    pub commits: Vec<OutgoingCommit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutgoingCommit {
    pub id: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PrReview {
    pub review_id: String,
    #[serde(flatten)]
    pub preview: github::PrPreview,
}

#[derive(Clone)]
pub struct Repo {
    project_path: PathBuf,
    skills_roots: Vec<PathBuf>,
}

impl Repo {
    pub fn new(
        project_path: impl AsRef<Path>,
        skills_roots: Vec<PathBuf>,
    ) -> Result<Self, AppError> {
        let input_path = project_path.as_ref();
        let canonical_path = std::fs::canonicalize(input_path)?;
        let skills_roots = skills_roots
            .into_iter()
            .map(|path| {
                if let Ok(relative) = path.strip_prefix(input_path) {
                    canonical_path.join(relative)
                } else if path.is_absolute() {
                    path
                } else {
                    canonical_path.join(path)
                }
            })
            .collect();
        Ok(Self {
            project_path: canonical_path,
            skills_roots,
        })
    }

    pub fn request(&self, request: Request) -> Result<Value, AppError> {
        match request {
            Request::Status => serde_json::to_value(self.status()?).map_err(AppError::internal),
            Request::Commit {
                review_id,
                paths,
                message,
            } => {
                let head = self.commit(&review_id, paths, &message)?;
                Ok(Value::String(head))
            }
            Request::CreateBranch { review_id, name } => {
                self.create_branch(&review_id, &name)?;
                Ok(Value::Null)
            }
            Request::PushPreview { remote, branch } => {
                serde_json::to_value(self.push_preview(&remote, &branch)?)
                    .map_err(AppError::internal)
            }
            Request::Push { review_id } => {
                self.push(&review_id)?;
                Ok(Value::Null)
            }
            Request::PrPreview { remote, base } => {
                let root = self.require_root()?;
                let lock = repo_lock(&root);
                let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                let state = self.read_state(&root)?;
                if let Some(reason) = state.blocked_reason {
                    return Err(AppError::invalid_input(reason));
                }
                let url = self.push_url_at(&root, &remote)?;
                let preview = github::pr_preview(&root, &url, base.as_deref())?;
                let branch = state.branch.ok_or_else(|| {
                    AppError::invalid_input("Pull requests require an attached branch")
                })?;
                let head = state
                    .head
                    .ok_or_else(|| AppError::git("HEAD is unavailable"))?;
                let review_id = store_review(
                    &root,
                    Review::Pr(PrState {
                        remote,
                        url,
                        head,
                        branch,
                        base: preview.base.clone(),
                    }),
                );
                serde_json::to_value(PrReview { review_id, preview }).map_err(AppError::internal)
            }
            Request::CreatePr { review_id, title } => {
                let root = self.require_root()?;
                let lock = repo_lock(&root);
                let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                let state = self.read_state(&root)?;
                if let Some(reason) = state.blocked_reason {
                    return Err(AppError::invalid_input(reason));
                }
                let Some(Review::Pr(review)) = get_review(&root, &review_id) else {
                    return Err(stale());
                };
                if state.head.as_deref() != Some(&review.head)
                    || state.branch.as_deref() != Some(&review.branch)
                {
                    return Err(stale());
                }
                if self.push_url_at(&root, &review.remote)? != review.url {
                    return Err(stale());
                }
                let url =
                    github::create_pr(&root, &review.url, &review.branch, &review.base, &title)?;
                serde_json::to_value(url).map_err(AppError::internal)
            }
        }
    }

    fn discover_root(&self) -> Result<Option<PathBuf>, AppError> {
        let out =
            git_output_allow_failure(&self.project_path, args(&["rev-parse", "--show-toplevel"]))?;
        if !out.status.success() {
            return Ok(None);
        }
        let root = output_path_text(out)?;
        Ok(Some(std::fs::canonicalize(root)?))
    }

    fn require_root(&self) -> Result<PathBuf, AppError> {
        self.discover_root()?
            .ok_or_else(|| AppError::invalid_input("This project is not a Git repository"))
    }

    fn status(&self) -> Result<Status, AppError> {
        let Some(root) = self.discover_root()? else {
            let id = uuid::Uuid::new_v4().to_string();
            return Ok(Status {
                review_id: id,
                root: self.project_path.to_string_lossy().into_owned(),
                branch: None,
                head: None,
                blocked_reason: Some("This project is not a Git repository.".into()),
                files: vec![],
                remotes: vec![],
                upstream_remote: None,
                upstream_branch: None,
            });
        };
        let lock = repo_lock(&root);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.read_state(&root)?;
        let review_id = store_review(&root, Review::Working(state.clone()));
        let mut files = Vec::new();
        for file in state.files.values() {
            let diff = self.diff_for_file(&root, file)?;
            files.push(FileStatus {
                path: file.path.clone(),
                original_path: file.original_path.clone(),
                status: file.status.clone(),
                skill_related: self.is_skill_path(&root, &file.path),
                blocked_reason: file.blocked_reason.clone(),
                truncated: diff.len() > MAX_DIFF_BYTES,
                diff: String::from_utf8_lossy(&diff[..diff.len().min(MAX_DIFF_BYTES)]).into_owned(),
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let remotes = self.remotes(&root)?;
        Ok(Status {
            review_id,
            root: root.to_string_lossy().into_owned(),
            branch: state.branch.clone(),
            head: state.head.clone(),
            blocked_reason: state.blocked_reason.clone(),
            files,
            remotes,
            upstream_remote: state.upstream_remote.clone(),
            upstream_branch: state.upstream_branch.clone(),
        })
    }

    fn read_state(&self, root: &Path) -> Result<WorkingState, AppError> {
        let head_out = git_output_allow_failure(root, args(&["rev-parse", "--verify", "HEAD"]))?;
        let head = head_out
            .status
            .success()
            .then(|| output_text(head_out).ok())
            .flatten();
        let branch_out =
            git_output_allow_failure(root, args(&["symbolic-ref", "--quiet", "--short", "HEAD"]))?;
        let branch = branch_out
            .status
            .success()
            .then(|| output_text(branch_out).ok())
            .flatten();
        let status = git_output(
            root,
            args(&[
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ]),
        )?;
        let mut files = parse_status(&status.stdout);
        if files.len() > 256 {
            return Err(AppError::invalid_input(
                "This repository has too many changed files for a safe review.",
            ));
        }
        let mut snapshot_bytes = 0usize;
        let mut ordered_paths = files.keys().cloned().collect::<Vec<_>>();
        ordered_paths.sort();
        for path in ordered_paths {
            let file = files.get_mut(&path).expect("key came from files");
            if snapshot_bytes >= MAX_SNAPSHOT_BYTES {
                file.blocked_reason =
                    Some("Changed file content is outside the bounded review snapshot.".into());
                file.content = Some(EntrySnapshot::Error);
                continue;
            }
            match read_entry(root, &file.path) {
                Ok(content) => {
                    if snapshot_bytes + content.size() > MAX_SNAPSHOT_BYTES {
                        file.blocked_reason = Some(
                            "Changed file content exceeds the bounded review snapshot.".into(),
                        );
                        file.content = Some(EntrySnapshot::Error);
                        continue;
                    }
                    snapshot_bytes += content.size();
                    file.content = Some(content);
                }
                Err(error) => {
                    file.blocked_reason = Some(error.message);
                    file.content = Some(EntrySnapshot::Error);
                }
            }
        }
        let index_snapshot = git_output(root, args(&["ls-files", "--stage", "-z"]))?.stdout;
        let too_large = index_snapshot.len() > MAX_SNAPSHOT_BYTES;
        let upstream = git_output_allow_failure(
            root,
            args(&[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ]),
        )?;
        let (upstream_remote, upstream_branch) = if upstream.status.success() {
            output_text(upstream)
                .ok()
                .and_then(|s| {
                    s.split_once('/')
                        .map(|(r, b)| (Some(r.to_string()), Some(b.to_string())))
                })
                .unwrap_or((None, None))
        } else {
            (None, None)
        };
        if head.is_none() {
            for file in files.values_mut() {
                file.blocked_reason.get_or_insert_with(|| {
                    "Create an initial commit before using project Git actions.".into()
                });
            }
        }
        let blocked_reason = repository_block(root, &files, branch.as_deref(), head.is_some())
            .or_else(|| {
                too_large
                    .then(|| "Git review exceeds the safe in-memory snapshot limit.".to_string())
            });
        Ok(WorkingState {
            head,
            branch,
            files,
            blocked_reason,
            upstream_remote,
            upstream_branch,
            fingerprint: status.stdout,
            index_snapshot,
        })
    }

    fn diff_for_file(&self, root: &Path, file: &FileSnapshot) -> Result<Vec<u8>, AppError> {
        if let Some(reason) = &file.blocked_reason {
            return Ok(reason.as_bytes().to_vec());
        }
        if file.status == "??" {
            return Ok(untracked_diff(file));
        }
        let mut a = args(&["diff", "--no-ext-diff", "--no-color", "HEAD", "--"]);
        a.push(pathspec(&file.path));
        if let Some(old) = &file.original_path {
            a.push(pathspec(old));
        }
        let output = git_output_allow_failure(root, a)?;
        if !output.status.success() {
            return Err(git_failure(output));
        }
        if !output.stdout.is_empty() {
            return Ok(output.stdout);
        }
        Ok(Vec::new())
    }

    fn is_skill_path(&self, root: &Path, path: &str) -> bool {
        self.skills_roots
            .iter()
            .filter_map(|p| p.strip_prefix(root).ok())
            .any(|base| Path::new(path).starts_with(base))
    }

    fn remotes(&self, root: &Path) -> Result<Vec<Remote>, AppError> {
        let out = git_output(root, args(&["remote"]))?;
        let mut remotes = Vec::new();
        for name in String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|s| !s.is_empty())
        {
            if let Ok(url) = self.remote_url_at(root, name) {
                remotes.push(Remote {
                    name: name.into(),
                    url: redact_url(&url),
                });
            }
        }
        Ok(remotes)
    }

    pub(crate) fn push_url_at(&self, root: &Path, name: &str) -> Result<String, AppError> {
        validate_remote(name)?;
        let mirror = git_output_allow_failure(
            root,
            [
                OsString::from("config"),
                OsString::from("--bool"),
                OsString::from("--get"),
                OsString::from(format!("remote.{name}.mirror")),
            ],
        )?;
        if mirror.status.success() && output_text(mirror)?.eq_ignore_ascii_case("true") {
            return Err(AppError::invalid_input(
                "Mirror remotes are not supported for reviewed pushes.",
            ));
        }
        let configured = git_output_allow_failure(
            root,
            [
                OsString::from("remote"),
                OsString::from("get-url"),
                OsString::from("--push"),
                OsString::from("--all"),
                OsString::from(name),
            ],
        )?;
        if !configured.status.success() {
            return Err(git_failure(configured));
        }
        let urls = configured
            .stdout
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .collect::<Vec<_>>();
        if urls.len() != 1 {
            return Err(AppError::invalid_input(
                "Push requires exactly one configured push URL.",
            ));
        }
        String::from_utf8(urls[0].to_vec())
            .map_err(|_| AppError::invalid_input("Push URL is not UTF-8."))
    }

    fn remote_url_at(&self, root: &Path, name: &str) -> Result<String, AppError> {
        validate_remote(name)?;
        let key = format!("remote.{name}.url");
        let out = git_output(
            root,
            [
                OsString::from("config"),
                OsString::from("--get-all"),
                OsString::from(key),
            ],
        )?;
        let urls = out
            .stdout
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .collect::<Vec<_>>();
        if urls.len() != 1 {
            return Err(AppError::invalid_input(
                "Remote must have exactly one configured URL",
            ));
        }
        String::from_utf8(urls[0].to_vec())
            .map_err(|_| AppError::invalid_input("Remote URL is not UTF-8"))
    }

    fn commit(
        &self,
        review_id: &str,
        selected: Vec<String>,
        message: &str,
    ) -> Result<String, AppError> {
        let root = self.require_root()?;
        let lock = repo_lock(&root);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if message.trim().is_empty() {
            return Err(AppError::invalid_input("Commit message is required"));
        }
        if selected.is_empty() {
            return Err(AppError::invalid_input("Select at least one file"));
        }
        let Some(Review::Working(review)) = get_review(&root, review_id) else {
            return Err(stale());
        };
        self.assert_unchanged(&root, &review)?;
        let selected_set = selected.iter().collect::<HashSet<_>>();
        if selected_set.len() != selected.len() {
            return Err(AppError::invalid_input("Selected files must be unique"));
        }
        let mut commit_args = args(&["commit", "--only", "-m", message, "--"]);
        let mut intent = Vec::new();
        for path in selected_set {
            let file = review.files.get(path.as_str()).ok_or_else(|| {
                AppError::invalid_input("Selected file was not part of this review")
            })?;
            if let Some(reason) = &file.blocked_reason {
                return Err(AppError::invalid_input(reason));
            }
            if file.staged && file.unstaged {
                return Err(AppError::invalid_input(
                    "Partially staged files must be reviewed as a whole before committing",
                ));
            }
            if file.status.contains('U') || file.status.starts_with('S') {
                return Err(AppError::invalid_input(
                    "Conflicts and submodules are not supported",
                ));
            }
            if file.status == "??" {
                intent.push(pathspec(&file.path));
            }
            commit_args.push(pathspec(&file.path));
            if let Some(old) = &file.original_path {
                commit_args.push(pathspec(old));
            }
        }
        if !intent.is_empty() {
            let mut stage_args = args(&["add", "--intent-to-add", "--"]);
            stage_args.extend(intent.clone());
            git_output(&root, stage_args)?;
        }
        let out = git_output_allow_failure(&root, commit_args)?;
        if !out.status.success() {
            if !intent.is_empty() {
                let mut reset = args(&["reset", "--quiet", "--"]);
                reset.extend(intent);
                let _ = git_output_allow_failure(&root, reset);
            }
            return Err(git_failure(out));
        }
        output_text(git_output(&root, args(&["rev-parse", "--verify", "HEAD"]))?)
    }

    fn assert_unchanged(&self, root: &Path, reviewed: &WorkingState) -> Result<(), AppError> {
        let now = self.read_state(root)?;
        let contents_match = reviewed.files.iter().all(|(path, f)| {
            now.files.get(path).and_then(|v| v.content.as_ref()) == f.content.as_ref()
        });
        if reviewed.blocked_reason.is_some()
            || now.blocked_reason.is_some()
            || now.head != reviewed.head
            || now.branch != reviewed.branch
            || now.fingerprint != reviewed.fingerprint
            || now.index_snapshot != reviewed.index_snapshot
            || !contents_match
        {
            return Err(stale());
        }
        Ok(())
    }

    fn create_branch(&self, review_id: &str, name: &str) -> Result<(), AppError> {
        validate_branch(name)?;
        let root = self.require_root()?;
        let lock = repo_lock(&root);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Review::Working(review)) = get_review(&root, review_id) else {
            return Err(stale());
        };
        self.assert_unchanged(&root, &review)?;
        if review.branch.is_none() {
            return Err(AppError::invalid_input(
                "Create a branch only from an attached branch",
            ));
        }
        if !review.files.is_empty() {
            return Err(AppError::invalid_input(
                "Create a branch only from a clean repository",
            ));
        }
        let out = git_output_allow_failure(
            &root,
            [
                OsString::from("switch"),
                OsString::from("-c"),
                OsString::from(name),
            ],
        )?;
        if !out.status.success() {
            return Err(git_failure(out));
        }
        Ok(())
    }

    fn push_preview(&self, remote: &str, branch: &str) -> Result<PushReview, AppError> {
        validate_remote(remote)?;
        validate_branch(branch)?;
        let root = self.require_root()?;
        let lock = repo_lock(&root);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let state = self.read_state(&root)?;
        if state.branch.as_deref() != Some(branch) {
            return Err(AppError::invalid_input(
                "Push preview must target the currently checked out branch",
            ));
        }
        if let Some(reason) = state.blocked_reason {
            return Err(AppError::invalid_input(reason));
        }
        let url = self.push_url_at(&root, remote)?;
        let remote_tip = remote_tip(&root, &url, branch)?;
        if let Some(tip) = &remote_tip {
            let refspec = format!("refs/heads/{branch}");
            let fetch = git_output_allow_failure(
                &root,
                [
                    OsString::from("fetch"),
                    OsString::from("--no-tags"),
                    OsString::from(&url),
                    OsString::from(refspec),
                ],
            )?;
            if !fetch.status.success() {
                return Err(git_failure(fetch));
            }
            let ancestor = git_output_allow_failure(
                &root,
                [
                    OsString::from("merge-base"),
                    OsString::from("--is-ancestor"),
                    OsString::from(tip),
                    OsString::from("HEAD"),
                ],
            )?;
            if !ancestor.status.success() {
                return Err(AppError::invalid_input("The remote branch has commits that are not in this branch. Reconcile the branches before pushing."));
            }
        }
        let range = remote_tip
            .as_ref()
            .map(|v| format!("{v}..HEAD"))
            .unwrap_or_else(|| "HEAD".into());
        let commits_out = git_output(
            &root,
            [
                OsString::from("log"),
                OsString::from("-z"),
                OsString::from("--format=%H%x09%s"),
                OsString::from(range),
            ],
        )?;
        let commits = parse_commits(&commits_out.stdout);
        if commits.is_empty() {
            return Err(AppError::invalid_input(
                "There are no outgoing commits to push",
            ));
        }
        let head = state
            .head
            .ok_or_else(|| AppError::git("HEAD is unavailable"))?;
        let review_id = store_review(
            &root,
            Review::Push(PushState {
                branch: branch.into(),
                remote: remote.into(),
                url: url.clone(),
                head,
                remote_tip,
            }),
        );
        Ok(PushReview {
            review_id,
            remote: remote.into(),
            branch: branch.into(),
            url: redact_url(&url),
            commits,
        })
    }

    fn push(&self, review_id: &str) -> Result<(), AppError> {
        let root = self.require_root()?;
        let lock = repo_lock(&root);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let Some(Review::Push(review)) = get_review(&root, review_id) else {
            return Err(stale());
        };
        let state = self.read_state(&root)?;
        if let Some(reason) = &state.blocked_reason {
            return Err(AppError::invalid_input(reason));
        }
        if state.branch.as_deref() != Some(&review.branch)
            || state.head.as_deref() != Some(&review.head)
        {
            return Err(stale());
        }
        if self.push_url_at(&root, &review.remote)? != review.url {
            return Err(stale());
        }
        let actual = remote_tip(&root, &review.url, &review.branch)?;
        if actual != review.remote_tip {
            return Err(stale());
        }
        let mut push_args = args(&["push", "--porcelain"]);
        if state.upstream_remote.is_none() {
            push_args.push(OsString::from("--set-upstream"));
        }
        push_args.push(OsString::from("--"));
        push_args.push(OsString::from(&review.remote));
        push_args.push(OsString::from(format!("HEAD:refs/heads/{}", review.branch)));
        let push = git_output_allow_failure(&root, push_args)?;
        if !push.status.success() {
            return Err(git_failure(push));
        }
        Ok(())
    }
}

/// Shared by the explicit GitHub CLI backend; arguments are never shell parsed.
pub(crate) fn git_output<I, S>(root: &Path, args: I) -> Result<Output, AppError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = git_output_allow_failure(root, args)?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(git_failure(out))
    }
}

pub(crate) fn git_output_allow_failure<I, S>(root: &Path, args: I) -> Result<Output, AppError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(AppError::io)
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn pathspec(value: &str) -> OsString {
    OsString::from(format!(":(literal){value}"))
}
fn output_text(output: Output) -> Result<String, AppError> {
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|_| AppError::git("Git returned non-UTF-8 output"))
}

fn output_path_text(output: Output) -> Result<String, AppError> {
    let mut bytes = output.stdout;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|_| AppError::git("Git returned a non-UTF-8 repository path"))
}
fn stale() -> AppError {
    AppError::invalid_input("Git state changed after review. Refresh the review before continuing.")
}
fn git_failure(out: Output) -> AppError {
    AppError::classify_git_error(redact_text(String::from_utf8_lossy(&out.stderr).trim()))
}
fn redact_text(text: &str) -> String {
    text.split_whitespace()
        .map(redact_url)
        .collect::<Vec<_>>()
        .join(" ")
}
fn redact_url(value: &str) -> String {
    if let Some((scheme, rest)) = value.split_once("://") {
        if let Some((_, host)) = rest.split_once('@') {
            return format!("{scheme}://***@{host}");
        }
    }
    value.to_string()
}
fn validate_remote(name: &str) -> Result<(), AppError> {
    if name.is_empty()
        || name.starts_with('-')
        || name.contains('/')
        || name.contains('\\')
        || name.chars().any(char::is_whitespace)
    {
        Err(AppError::invalid_input("Invalid Git remote name"))
    } else {
        Ok(())
    }
}
fn validate_branch(name: &str) -> Result<(), AppError> {
    if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_whitespace) {
        return Err(AppError::invalid_input("Invalid branch name"));
    }
    let out = Command::new("git")
        .args(["check-ref-format", "--branch", name])
        .output()
        .map_err(AppError::io)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(AppError::invalid_input("Invalid branch name"))
    }
}

#[derive(Clone)]
struct FileSnapshot {
    path: String,
    original_path: Option<String>,
    status: String,
    staged: bool,
    unstaged: bool,
    blocked_reason: Option<String>,
    content: Option<EntrySnapshot>,
}

#[derive(Clone, PartialEq, Eq)]
enum EntrySnapshot {
    File(Vec<u8>, bool),
    Symlink(Vec<u8>),
    Directory,
    Other,
    Missing,
    Error,
}
impl EntrySnapshot {
    fn size(&self) -> usize {
        match self {
            Self::File(bytes, _) | Self::Symlink(bytes) => bytes.len(),
            _ => 0,
        }
    }
}

#[derive(Clone)]
struct WorkingState {
    head: Option<String>,
    branch: Option<String>,
    files: HashMap<String, FileSnapshot>,
    blocked_reason: Option<String>,
    upstream_remote: Option<String>,
    upstream_branch: Option<String>,
    fingerprint: Vec<u8>,
    index_snapshot: Vec<u8>,
}

#[derive(Clone)]
struct PushState {
    branch: String,
    remote: String,
    url: String,
    head: String,
    remote_tip: Option<String>,
}
#[derive(Clone)]
struct PrState {
    branch: String,
    remote: String,
    url: String,
    head: String,
    base: String,
}

#[derive(Clone)]
enum Review {
    Working(WorkingState),
    Push(PushState),
    Pr(PrState),
}

fn parse_status(bytes: &[u8]) -> HashMap<String, FileSnapshot> {
    let mut records = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut result = HashMap::new();
    while let Some(record) = records.next() {
        let kind = record.first().copied().unwrap_or_default();
        let (path_bytes, status, staged, unstaged, original_path, blocked_reason) = match kind {
            b'?' => (
                record.get(2..).unwrap_or_default(),
                "??".to_string(),
                false,
                true,
                None,
                None,
            ),
            b'1' | b'2' | b'u' => {
                // Porcelain v2 fixed fields contain no spaces; the final path may.
                let count = match kind {
                    b'2' => 10,
                    b'u' => 11,
                    _ => 9,
                };
                let fields = record.splitn(count, |b| *b == b' ').collect::<Vec<_>>();
                if fields.len() < count {
                    continue;
                }
                let xy = fields.get(1).copied().unwrap_or_default();
                let status = String::from_utf8_lossy(xy).into_owned();
                let (original, original_blocked) = if kind == b'2' {
                    match records.next() {
                        Some(v) => match std::str::from_utf8(v) {
                            Ok(path) => (Some(path.to_string()), None),
                            Err(_) => (None, Some("Rename source has a non-UTF-8 path and cannot be changed here.".to_string())),
                        },
                        None => (None, Some("Rename source is missing from Git status.".to_string())),
                    }
                } else {
                    (None, None)
                };
                let submodule = fields.get(2).is_some_and(|v| v.starts_with(b"S"));
                let conflict = kind == b'u' || xy.contains(&b'U');
                let blocked = if conflict {
                    Some("Conflicted files cannot be changed here.")
                } else if submodule {
                    Some("Submodule changes are not supported.")
                } else {
                    None
                };
                (
                    fields.last().copied().unwrap_or_default(),
                    status,
                    xy.first().is_some_and(|v| *v != b'.'),
                    xy.get(1).is_some_and(|v| *v != b'.'),
                    original,
                    blocked.map(str::to_string).or(original_blocked),
                )
            }
            _ => continue,
        };
        let path = match std::str::from_utf8(path_bytes) {
            Ok(value) => value.to_string(),
            Err(_) => format!("<unsupported non-UTF-8 path {}>", result.len()),
        };
        let unsupported = path.starts_with("<unsupported non-UTF-8 path ");
        let blocked_reason = blocked_reason.or_else(|| {
            unsupported.then(|| "Non-UTF-8 paths are visible only as blocked entries.".to_string())
        });
        let blocked_reason = blocked_reason.or_else(|| (staged && unstaged).then(|| "Partially staged files are blocked until their index and working copy are reconciled.".into()));
        result.insert(
            path.clone(),
            FileSnapshot {
                path,
                original_path,
                status,
                staged,
                unstaged,
                blocked_reason,
                content: None,
            },
        );
    }
    result
}

fn read_entry(root: &Path, relative: &str) -> Result<EntrySnapshot, AppError> {
    let relative_path = Path::new(relative);
    if relative_path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(AppError::invalid_input(
            "Git returned an unsafe repository path",
        ));
    }
    let components = relative_path.components().collect::<Vec<_>>();
    let mut path = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        if index + 1 < components.len() {
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.file_type().is_symlink() {
                    return Err(AppError::invalid_input(
                        "A parent symlink prevents safe project Git access",
                    ));
                }
            }
        }
    }
    if !path.starts_with(root) {
        return Err(AppError::invalid_input("Git path escapes repository root"));
    }
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EntrySnapshot::Missing)
        }
        Err(error) => return Err(error.into()),
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&path)?;
        #[cfg(unix)]
        use std::os::unix::ffi::OsStrExt;
        #[cfg(unix)]
        let bytes = target.as_os_str().as_bytes().to_vec();
        #[cfg(not(unix))]
        let bytes = target.to_string_lossy().as_bytes().to_vec();
        Ok(EntrySnapshot::Symlink(bytes))
    } else if meta.is_file() {
        if meta.len() as usize > MAX_SNAPSHOT_BYTES {
            return Err(AppError::invalid_input(
                "Changed file exceeds the safe in-memory review limit.",
            ));
        }
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        #[cfg(unix)]
        let executable = meta.permissions().mode() & 0o111 != 0;
        #[cfg(not(unix))]
        let executable = false;
        Ok(EntrySnapshot::File(std::fs::read(path)?, executable))
    } else if meta.is_dir() {
        Ok(EntrySnapshot::Directory)
    } else {
        Ok(EntrySnapshot::Other)
    }
}

fn repository_block(
    root: &Path,
    files: &HashMap<String, FileSnapshot>,
    branch: Option<&str>,
    has_head: bool,
) -> Option<String> {
    if branch.is_none() {
        return Some("Detached HEAD is not supported.".into());
    }
    if !has_head {
        return Some("This repository has no commits yet. Create an initial commit before using project Git actions.".into());
    }
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-apply",
        "rebase-merge",
    ] {
        if let Ok(out) = git_output_allow_failure(root, args(&["rev-parse", "--git-path", name])) {
            if out.status.success() {
                let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
                let path = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                if path.exists() {
                    return Some("Finish or abort the active Git operation first.".into());
                }
            }
        }
    }
    files.values().find_map(|f| {
        f.blocked_reason
            .as_deref()
            .filter(|reason| reason.starts_with("Conflicted"))
            .map(str::to_string)
    })
}

fn parse_commits(bytes: &[u8]) -> Vec<OutgoingCommit> {
    bytes
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .filter_map(|record| {
            let split = record.iter().position(|b| *b == b'\t')?;
            Some(OutgoingCommit {
                id: String::from_utf8_lossy(&record[..split]).into_owned(),
                summary: String::from_utf8_lossy(&record[split + 1..]).to_string(),
            })
        })
        .collect()
}

fn untracked_diff(file: &FileSnapshot) -> Vec<u8> {
    match file.content.as_ref() {
        Some(EntrySnapshot::File(bytes, executable)) => {
            if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
                return format!("Binary file added: {} ({} bytes)\n", file.path, bytes.len())
                    .into_bytes();
            }
            let text = std::str::from_utf8(bytes).unwrap_or_default();
            let mut diff = format!(
                "diff --git a/{0} b/{0}\nnew file mode {1}\n--- /dev/null\n+++ b/{0}\n",
                file.path,
                if *executable { "100755" } else { "100644" }
            );
            let count =
                text.lines().count() + usize::from(!text.is_empty() && !text.ends_with('\n'));
            diff.push_str(&format!("@@ -0,0 +1,{count} @@\n"));
            for line in text.split_inclusive('\n') {
                diff.push('+');
                diff.push_str(line);
                if !line.ends_with('\n') {
                    diff.push('\n');
                }
            }
            diff.into_bytes()
        }
        Some(EntrySnapshot::Symlink(target)) => format!(
            "New symbolic link: {} -> {}\n",
            file.path,
            String::from_utf8_lossy(target)
        )
        .into_bytes(),
        Some(EntrySnapshot::Missing) => {
            format!("Untracked path is missing: {}\n", file.path).into_bytes()
        }
        _ => format!("Untracked change: {}\n", file.path).into_bytes(),
    }
}

fn remote_tip(root: &Path, url: &str, branch: &str) -> Result<Option<String>, AppError> {
    let refname = format!("refs/heads/{branch}");
    let out = git_output_allow_failure(
        root,
        [
            OsString::from("ls-remote"),
            OsString::from("--heads"),
            OsString::from("--"),
            OsString::from(url),
            OsString::from(&refname),
        ],
    )?;
    if !out.status.success() {
        return Err(git_failure(out));
    }
    let rows = out
        .stdout
        .split(|b| *b == b'\n')
        .filter(|r| !r.is_empty())
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() != 1 {
        return Err(AppError::invalid_input(
            "Remote branch lookup returned ambiguous results.",
        ));
    }
    let fields = rows[0].split(|b| *b == b'\t').collect::<Vec<_>>();
    if fields.len() != 2 || fields[1] != refname.as_bytes() {
        return Err(AppError::invalid_input(
            "Remote branch lookup returned an unexpected ref.",
        ));
    }
    let sha = std::str::from_utf8(fields[0])
        .map_err(|_| AppError::git("Remote returned an invalid commit ID"))?;
    if sha.len() < 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::git("Remote returned an invalid commit ID"));
    }
    Ok(Some(sha.to_string()))
}

fn store_review(root: &Path, review: Review) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let mut all = review_cache().lock().unwrap_or_else(|e| e.into_inner());
    if !all.contains_key(root) && all.len() >= 8 {
        if let Some(first_root) = all.keys().next().cloned() {
            all.remove(&first_root);
        }
    }
    let entries = all.entry(root.to_path_buf()).or_default();
    while entries.len() >= MAX_REVIEWS_PER_REPO {
        if let Some(first) = entries.keys().next().cloned() {
            entries.remove(&first);
        }
    }
    entries.insert(id.clone(), review);
    id
}

fn get_review(root: &Path, id: &str) -> Option<Review> {
    review_cache().lock().ok()?.get(root)?.get(id).cloned()
}
fn review_cache() -> &'static Mutex<HashMap<PathBuf, HashMap<String, Review>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, HashMap<String, Review>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn repo_lock(root: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    locks.retain(|_, value| value.strong_count() > 0);
    if let Some(lock) = locks.get(root).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(root.to_path_buf(), Arc::downgrade(&lock));
    lock
}
