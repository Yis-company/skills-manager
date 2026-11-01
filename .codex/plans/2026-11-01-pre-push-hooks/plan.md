# Pre-push guard

## Decision for review

Install a small, source-controlled pre-push guard automatically on `bun install`. Run existing non-watch frontend checks and Rust formatting before each push. Keep CI unchanged. Remote caching is a separate decision.

## Context and boundaries

- Root: `/Users/yitam/Documents/code/ybtam/skills-manager`; branch `chore/bun-migration`, HEAD `ad60b384216afe3db4af1710b214a733376b0dc8`; clean and no unmerged paths at inspection. PR #101 is merged at that head. Do not switch branches, commit, push, or create a PR as part of this work. Recheck state before implementation and preserve unrelated work, including ignored/untracked plans and agent configuration. If conflicts or messy history block the change, stop and ask.
- Bun migration is the current base; `packageManager` and CI specify Bun 1.4.4 and CI Node 22.23.4. This machine has Bun 1.4.0 and Node 26.5.0, so local results are not identical to CI. Git and cargo/rustfmt are available. No `core.hooksPath` is configured here; `.git/hooks` contains only samples. No existing package lifecycle script or committed hook was found.
- No project installs, prepare hooks, or broad tests were run during planning. Vitest includes `src/**/*.test.ts` and excludes Playwright. Inspection found mocked Tauri IPC and fixture URLs; one test reads source. No obvious live service or database setup was found, but this is static evidence, not a runtime guarantee.

## Proposed changes

1. Add a short tracked hook and a small installation script, using existing Node/Bun tooling and no dependency or task runner. Add a `prepare` lifecycle entry without replacing any existing lifecycle behavior. Installation resolves the effective Git hook location, installs only when safe, is repeatable, and never silently overwrites a foreign hook or changes `core.hooksPath`. Refuse with an actionable message on conflicting hook or unsupported configuration; don't claim protection when installation is skipped. Handle linked worktrees and paths with spaces. Document install, prerequisites, checks, and how to resolve a personal-hook conflict in the developer setup section of README.
2. Hook runs from the project root, fails on a missing required tool or first failed command, and runs: `bun run format:check`, `bun run lint`, a new `typecheck` script invoking `tsc -b`, `bun run test`, and `cargo fmt --manifest-path src-tauri/Cargo.toml --check`. TypeScript build mode includes app, tooling and E2E type configurations and writes incremental metadata under ignored `node_modules/.tmp`, but emits no app code. This avoids the Vite build output while retaining CI's TypeScript check. The checks inspect the working tree, not an exact pushed commit; Git can bypass hooks explicitly and installs can skip lifecycle scripts.
3. Keep frontend CI's full build and Playwright, changeset checks, macOS/Windows Rust tests, and Linux Rust formatting and all-target Clippy unchanged. Native compile/tests may require platform libraries, cached crates, network, and substantial time; local Rust formatting supplies a proportionate native guard, not cross-platform validation. No Turbo, remote cache, Docker producer, new framework, custom hashing, or app changes.

## Verification after approval

- Recheck branch, HEAD, status, conflicts and current PR; do not incorporate unrelated migration commits or modify unrelated files.
- Use isolated temporary Git repositories and a local bare remote. Exercise the real lifecycle installation and installed hook: passing checks permit a synthetic ref update; controlled failures of each chosen check and missing tools block it; repeated installation is idempotent; existing personal hooks and configured hook paths are preserved or explicitly refused. No real remote push. Clean up only fixtures created for this task.
- Run the chosen checks and focused native verification on this checkout; report exact commands, results, elapsed times and any baseline failures or environment/version limits. Investigate failures rather than weaken checks. Verify install behavior on the available platform; note untested Windows/platform behavior. No broad checks or lifecycle execution until this plan is approved.

## Approval gate

Yi's shared AGENTS.md requires this plan rendered to HTML and reviewed with `plannotator annotate --gate --json --require-approval`. Only `decision=approved` permits implementation; carry approval notes into the work. Revise and review again if changes are requested. Dismissal is not approval.
