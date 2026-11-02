# Retro: ybtam/skills-manager, 2026-10-26 to 2026-11-01

- **Window:** 2026-10-26T00:00:00Z (inclusive) to 2026-11-02T00:00:00Z (exclusive), UTC
- **Device:** MacBook Pro (Yi), environment `17d3cb3b-7d63-4119-8d4f-ab0f49e7b15d`
- **Project:** ybtam/skills-manager (`52c33fb2-1d72-463e-b7af-010ad211e74b`), workspace `/Users/yitam/Documents/code/ybtam/skills-manager`
- **Repository:** `Yis-company/skills-manager`, confirmed from `git remote -v`
- **Method:** Matt Pocock's `retro` skill (upstream SKILL.md) plus `writing-for-agents` (local copy). Read-only. No CodeGraph index (`.codegraph/` absent).

## Activity

All in-window work happened on **2026-11-01, 08:05–23:53 UTC**. No messages from Oct 26–31, and none after the window in these threads. Threads from 2026-10-05 to 10-15 (`f5f4e997…`, `123bacd2…` and their children) had their last activity on Oct 15, so they are out of window.

## Coverage

| Source | Covered | Notes |
|---|---|---|
| T3 threads (roots + children) | **30/30** | 6 roots, 7 delegated-task children, 17 native Codex subagents. Every one has in-window messages, checked through `statev2.sqlite` (`mode=ro`, `query_only`). 414 messages, 65 Codex runs + 1 Claude run. |
| Codex provider rollouts | **62** files | Grouped by session/tool counts (1,948 tool calls; 1,809 `exec`). These rollouts are where the full native-subagent transcripts live. |
| PRs with in-window activity | **8/8** (#98–#105) | Paginated GraphQL search sorted by updatedAt. Read the body, all comments, all commits and each commit's check rollup. Reviews and review threads: **0 on every PR**. Failed CI logs read for main and #105. |
| PRs referenced by sessions | #99, #100, #104, #105 | All already in the set above. |
| main CI runs | 14 in window | 5 failed. |

**Gaps:**
- PR #101 (Bun migration, `chore/bun-migration`) came from no T3 thread on this device. Its source session is either on another device or outside T3, so it isn't covered here.
- Linear SKI tickets were not read.
- Native subagent transcripts were aggregated, not read message by message.
- Windows/Linux job logs for #105 `10a2e4e` were too noisy to grep. Their root causes come from the session's own diagnosis (23:27, 23:43) and the fix commits.

## Findings (by severity)

### 1. High: nothing stops a red PR from merging, so main went red and stayed red (partially fixed)

- **Evidence:**
  - The `main` ruleset contains only `deletion`, `non_fast_forward` and `pull_request`. It has no `required_status_checks`, and the branch has no protection (API returns 404).
  - [PR #99](https://github.com/Yis-company/skills-manager/pull/99) was titled `docs: initialize Impeccable product context`. Its body says "Documentation and configuration only; no app code changed".
  - Commit `f79baf8` was still pushed onto #99: "feat: clarify preset changes and review Sync conflicts", 50 files, +2,840/−570. Zero CI runs happened on that SHA.
  - Thread <t3-thread://v2/17d3cb3b-7d63-4119-8d4f-ab0f49e7b15d/2117c26e-b3cc-489a-ab3c-fa4affa15bca>, 10:11:44: the agent pushed merge commit `d18f8c4` and reported "GitHub CI is still running". Its local check was "Rust compilation" only: no `cargo test`, no clippy.
  - The PR merged at **10:12:38**. Afterwards, Rust check (Linux) failed on clippy at 10:13:36 ("iterating on a map's keys" and "very complex type"), and Rust tests (Windows) failed at 10:17 (`change_review.rs:736`).
  - main `Test` run 41296472336 on `3f130b8` failed. `.github/workflows/test.yml:6-12` only runs on `src-tauri/**`, so the later merges #103 and #104 never re-ran Rust CI and main stayed red without anyone seeing it.
- **Root cause:** The merge gate exists only as habit. The `paths` filter also means a required Rust check would never report on PRs that don't touch Rust.
- **Status:** Partially fixed. Open [PR #105](https://github.com/Yis-company/skills-manager/pull/105) (`10a2e4e`) fixes the clippy and Windows test failures. main stays red until #105 merges, and the gate is still missing.
- **Smallest durable change:**
  - Add `required_status_checks` to the `main` ruleset: Lint/typecheck/test, E2E tests, Changeset present, Rust check (Linux), Rust tests (macOS/Windows).
  - Make the Test workflow always report. Either drop the `paths` filter, or add a cheap always-run job that skips Rust work when `src-tauri/**` is unchanged.
- **Verify:** `gh api repos/Yis-company/skills-manager/rulesets/<main-id>` lists the checks. A PR with a red or pending required check shows "Merging is blocked".

### 2. High: a red clippy baseline became normal across the SKI-19 slices (open as a guardrail; this instance fixed in #105)

- **Evidence:**
  - SKI-23 child <t3-thread://v2/17d3cb3b-7d63-4119-8d4f-ab0f49e7b15d/thread:delegated-task:command%3Amcp%3A77956374-3917-44a1-a5e6-726f64194f5b%3Adelegate-task%3Afe9b3690-e80d-4f44-b17a-62445ac5c080>, 19:38:43: "Strict Clippy reports four diagnostics already present at the starting HEAD".
  - Root <t3-thread://v2/17d3cb3b-7d63-4119-8d4f-ab0f49e7b15d/0237155e-923f-450e-bbbb-e0bee66b51d0>, 19:53:55: "four diagnostics predate this slice, including two in earlier operation code". So SKI-20 to SKI-22 introduced two new ones that nothing blocked.
  - The SKI-24 and SKI-25 prompts (19:55, 20:43) told implementers to leave the 4 "baseline diagnostics" alone.
  - At 23:14 the root opened #105 and disclosed "the two existing Clippy diagnostics" instead of fixing them. CI on `d85ff2b` then failed 4 jobs.
  - The pre-push guard session <t3-thread://v2/17d3cb3b-7d63-4119-8d4f-ab0f49e7b15d/mcp:launch:7133b989d1a8dddae4749e2ae63fa6a5c89622e7205ae5d0b59a4519c6c4d166>, 15:34:41, left clippy and Rust tests out because "their local cost has not been measured".
- **Root cause:**
  - `scripts/pre-push.sh` runs `cargo fmt --check` but not `cargo clippy --all-targets -- -D warnings`.
  - Once main was red (finding 1), every agent treated red as the baseline.
- **Status:** Open. `scripts/pre-push.sh` (lines 5–9) still has no clippy. #105 fixes the current diagnostics.
- **Smallest durable change:**
  1. Time `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` on a warm cache.
  2. If it's acceptable, add it to `scripts/pre-push.sh`. Leave `cargo test` in CI.
- **Verify:** Add a deliberate clippy warning and `git push` exits non-zero.

### 3. High: the release/Version workflow has been broken on main since the Bun migration (open)

- **Evidence:**
  - `.github/workflows/version.yml:49` reads `version-script: bunx changeset:version`. It was introduced by `9d2e2fe` ([PR #101](https://github.com/Yis-company/skills-manager/pull/101)).
  - bunx reads `changeset:version` as a GitHub shorthand, which gives `"git clone" for "@changeset:version" failed`.
  - main Version runs failed at 09:30, 10:12, 15:22 and 15:48 UTC (runs 41293438614, 41296472428, 41320885265, 41323050293).
  - [PR #102](https://github.com/Yis-company/skills-manager/pull/102) "chore: version packages" hasn't changed since 09:28. Changesets from #101, #99, #103 and #104 are piling up unreleased.
  - **Zero** session messages mention it.
- **Root cause:**
  - The script only runs after a push to main, and no PR check exercises it.
  - Agents stop watching at merge, so nobody sees failures after the merge.
- **Status:** Open. The same line is on main and on the #105 branch.
- **Smallest durable change:**
  - Use `version-script: bun run changeset:version`, which runs the existing `package.json` script.
  - Optionally have the Changeset job run the version script in its throwaway checkout, so PRs exercise it.
- **Verify:** The next main push's Version job succeeds and #102 updates with the pending changesets.

### 4. Medium: E2E specs depend on the machine (/tmp screenshots) and the standards review didn't catch it (partially fixed)

- **Evidence:**
  - #105 E2E on `d85ff2b` (23:22) failed with `ENOENT … '/private/tmp/ski-23-checking-light.png'`. Six of these, from SKI-23/24/25.
  - The same run also hit two other test-hygiene failures that the session diagnosed at 23:27–23:31: a macOS test that asserted a transient scheduler state, and a fixture that relied on the global Git identity (Windows).
  - Final standards review <t3-thread://v2/17d3cb3b-7d63-4119-8d4f-ab0f49e7b15d/thread:provider:codex:native-thread:01a198fb-3d7d-7ef2-8833-af9201bdab69>, 21:22:07, reported "0 documented-standard violations". The repo has no `CODING_STANDARDS.md`.
  - main still has `e2e/specs/workspace-matrix.spec.ts:170` and `:450` writing `/tmp/agents-manager-matrix-*.png` (from #103).
- **Root cause:**
  - These are mechanical patterns with no lint rule.
  - The judgement-call test rules aren't written anywhere the reviewer reads.
- **Status:** Partially fixed. `10a2e4e` moved 3 specs to Playwright output folders. `workspace-matrix.spec.ts` remains.
- **Smallest durable change:**
  - Add an oxlint `no-restricted-syntax` rule (`.oxlintrc.json`, scoped to `e2e/**`) that bans string or template literals starting with `/tmp` or `/private/tmp` in `screenshot({ path })`. The alternative is to require `testInfo.outputPath()`.
  - Add a short `CODING_STANDARDS.md` with two review-time judgement rules: assert observed events, not momentary state; test fixtures set their own Git identity and environment.
- **Verify:** `bun run lint` fails on the current `workspace-matrix.spec.ts:170`.

### 5. Low: the shared pre-push hook breaks pushes from branches older than the hook (open, self-resolved this time)

- **Evidence:**
  - Root thread, 23:15:37: "The push hit a repository hook that expects a script added to `main` after this branch started". The agent merged main to get past it.
  - `scripts/install-hooks.mjs` writes into the common `.git/hooks`, which every worktree shares.
  - `scripts/pre-push-hook.sh` runs `exec sh "$root/scripts/pre-push.sh"` without checking that the file exists.
- **Smallest durable change:** In `scripts/pre-push-hook.sh`, if `scripts/pre-push.sh` is missing, print a one-line notice and exit 0. Or exit 1 with "merge main to get the guard".
- **Verify:** Pushing from a pre-`bc236d6` branch in a worktree gives the defined behaviour.

### 6. Low: the changeset requirement is only enforced in CI (fixed for that PR, guardrail open)

- **Evidence:** [PR #104](https://github.com/Yis-company/skills-manager/pull/104) `bc236d6` failed "Changeset present" (15:40). It was fixed by `a18282d` at 15:46, after the T3 PR watch reported it.
- **Smallest durable change:** Add `bunx changeset status --since=origin/main` to `scripts/pre-push.sh`, the same command as `.github/workflows/changeset-check.yml:45`.
- **Verify:** Pushing an app change without a changeset fails locally.

## Recognised in-week fixes

- `ad60b38` fixed the Bun E2E dev-server failure within 2 minutes (#101).
- #98 added `AGENTS.md` and `docs/agents/*`: 13 lines of navigation pointers only, with no sprawl or no-ops.
- #104 added the pre-push guard.
- #105 `10a2e4e` and `a1f6a64` fixed clippy, the Windows manifest/test, the transient test and 3 /tmp specs.

## Observations without proposals

- Tool economy: native "merge" subagents spent 32–69 tool calls each on fast-forward integrations (`/root/merge_final_fixes` 69, `merge_ski25` 41). That cost belongs to the user-level `implement-spec` workflow, not this repo. Flagged only.
- None of the 8 PRs has a GitHub review. Merges came 1–36 minutes after creation. Review happened only in-session, for #105.

## Highest-value actions (recommendations; nothing implemented)

1. **Gate merges on CI** (finding 1): required status checks on `main`, plus a Test workflow that always reports.
2. **Fix `version.yml:49`** to `bun run changeset:version` (finding 3). It's one line and unblocks releases.
3. **Add strict clippy to `scripts/pre-push.sh`** after timing it, and add the `/tmp` e2e lint rule (findings 2 and 4).
