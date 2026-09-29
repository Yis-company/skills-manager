# Project Git actions

Add a focused Git workflow to each project's detail page: review changes, commit selected files, push a branch, and create or open its GitHub pull request.

This is the canonical implementation proposal. Implementation starts only after HTML review approval. Approval covers implementation and local verification, not publishing this repository's changes.

## Grounded findings

| Existing code | Implication |
| --- | --- |
| `src/views/ProjectDetail.tsx:61`, project header around line 872 | Add a Git entry alongside project actions; keep the workflow in a separate component. |
| `src-tauri/src/core/skill_store.rs:98` and `commands/projects/crud.rs:95–115` | Resolve project ID to its stored path on the owning host. Standard projects deploy symlinks or vendored skills. Linked workspaces instead store a skills-root path. |
| `src/lib/hostCall.ts:30`, `src/lib/appQueries.ts:22` | Capture host and project IDs for reads and mutations; changing hosts must not redirect work or display stale results in the new host. |
| `src/lib/hostScope.ts`, `src-tauri/src/core/host_dispatch.rs`, `src-tauri/src/lib.rs:1018` | Add matching host scope, remote dispatch, and desktop command registration. |
| `src-tauri/src/commands/git_backup.rs:62`, `core/git_backup.rs:454–484` | Backup targets the central library and stages broadly. Do not reuse these commands for project Git. |
| `src/index.css` | Reuse neutral surfaces, emerald actions, and existing light/dark styling. |

## Proposed first version

A **Git** action opens a focused dialog or panel for standard project workspaces. Linked workspaces are excluded initially because their stored path denotes a skill folder rather than the project boundary. Discover the actual Git root and display it with host, branch, and remote, including when the project is nested in a larger repository. Non-repositories receive an explanation; no initialization action is added.

1. **Review and commit.** Show all changed repository files and a diff, with file-level checkboxes. Default selection includes only confidently recognized skill deployment paths inside this project; other files require explicit selection. If classification is uncertain, select nothing. Require a commit message and show selected files before Commit.
2. **Push.** Show the exact destination, target branch, and every outgoing commit, including pre-existing commits. Push is a separate explicit action. Require a destination choice when ambiguous; do not silently assume origin. Establish a missing upstream only through the reviewed push.
3. **Pull request.** For a GitHub destination, show repository, head, selectable base, and editable title. Require the head to be pushed. Offer Open PR when an open PR already exists for the same repository/head/base; otherwise offer Create PR. Open its URL locally even when the project lives on a remote host.

## Assumptions for approval

- GitHub is the only PR provider in this version. Use the owning host's installed, authenticated `gh`. Missing tool/authentication disables PR creation with an explanation; commit and push remain usable. No automatic installation, login, or new token storage.
- Use installed `git`, existing identity, credentials, and hooks. Run subprocesses with separate arguments, never shell-interpolated input.
- Support whole-file selection. Block selected files with both staged and unstaged changes rather than altering partial staging silently.
- Require a feature branch distinct from the selected PR base. Offer simple branch creation only in a clean, attached, non-conflicted repository; otherwise explain how to prepare the branch externally.
- No remote creation, fork setup, force push, merge, rebase, conflict resolution, checkout manager, or general Git client.

## Correctness boundaries

**Preserve staging.** Commit literal selected paths with `git commit --only` so unrelated staged paths do not enter the commit. Handle untracked selected files through the minimum required selected-path preparation; prove that it preserves unrelated index entries. Reject partially staged selected files before any write. Treat rename source and destination together. Do not use broad staging. A failing hook must not disturb unrelated staging; refresh status and explain any selected-path preparation left behind. Verify this behavior in real temporary repositories before UI integration.

**Revalidate reviews.** Before writes, reread HEAD, branch, selected status/content, and destination or outgoing commits as applicable. Compare with the reviewed snapshot without adding an application hashing scheme. Changed content requires a refreshed review. Serialize this application's writes per repository; retain Git's own locking. External edits during commands remain possible and must be surfaced accurately.

**Respect boundaries.** Resolve stored project paths on the owning host, validate selections relative to the discovered root, use literal pathspecs and option separation, and parse NUL-delimited status. Stage symlink entries without following their library targets. Block conflicts, detached HEAD, active merge/rebase, and unsupported submodule selections before mutation. Cap oversized diffs with a visible notice and identify binary changes as metadata.

**Review the push.** Refresh remote branch information before calculating outgoing commits. An authentication/network failure is not current remote evidence. Redact credentials in displayed URLs and errors. Report diverged/rejected pushes without force or automatic recovery. Do not blindly retry publication.

**Make PR calls explicit.** Pass repository, head, base, and title to `gh`; do not allow an implicit push or interactive defaults. Recheck existing PRs before creating. A lookup failure is not “no PR.” After an uncertain create result, look up the PR before offering retry.

## Implementation sequence

1. Add a small project Git backend module and typed commands, with stored-path resolution, argument-based execution, status/diff reads, repository checks, and write serialization. Register desktop and remote routes together.
2. Prove selected-file commits and hook failure behavior in temporary repositories. Implement reviewed push and explicit GitHub PR lookup/create next.
3. Extend existing typed frontend calls and host/project query keys. Bind asynchronous work to captured IDs and refresh relevant Git state after both success and failure.
4. Add a focused component opened from ProjectDetail, reusing modal/button/error/notification and translation conventions. Keep Commit, Push, and Create PR separate, with their affected files or destinations visible.
5. Run the checks below, relevant existing tests, build, and lint. Include any required Changeset. Report local checks separately from live-host, GitHub, or CI proof.

## Acceptance checks

| Boundary | Verification |
| --- | --- |
| File selection | Temporary repositories with skill changes, unrelated staged/unstaged files, new files, deletions, renames, symlinks, spaces and leading dashes. Assert committed tree and unchanged unrelated index/worktree content. |
| Partial staging and hooks | Partial selection state is blocked without mutation. Successful hooks run; failed hooks preserve unrelated staging. Empty selection/message cannot commit. |
| Stale state | Edit reviewed content or move HEAD; require refreshed review. Conflicts, detached HEAD, active operations, and unsupported submodules do not trigger writes. |
| Push | Local bare remote verifies destination, all outgoing commits, upstream creation, and successful push. Independently advance remote and verify rejection without force. |
| PR | Mock gh process responses for existing/new PR, explicit arguments, missing tool/authentication, lookup failure, and uncertain creation. No real publication is needed. |
| Host routing | Scope, desktop registry, and remote dispatch agree. A delayed request stays on its original host and cannot overwrite the newly selected host's view. |
| UI and project checks | Verify default selections, diffs, action separation, busy/error states, destinations, PR base/title, keyboard use, narrow/wide light/dark layouts. Run focused Rust/Vitest tests, frontend build/lint, and a focused browser scenario where the existing harness supports it. |

## Review decision

Approve this scope or request revisions to the GitHub CLI dependency, staging rules, branch boundary, or workflow. Record the structured decision beside these files as `review.json`. This planning change contains no application implementation.
