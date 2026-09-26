# Integrate upstream v1.40.1

Bring upstream's data-protection and library-move fixes into this fork while preserving its architecture and release process. This proposal requires Yi's approval before implementation.

## Evidence and scope

The assessed fork head is `54dea85f3cf01533bb1a1e31fdcf1cc419876fcd`; upstream is `1d41f1878b8d4506d57e1c2f19de866ea52febc7`, with common ancestor `6ae02e39d9efea0faf75e643b8205f97833a593d`. The fork is 165 commits ahead and 11 behind. The merge simulation reports 14 conflicting files; the upstream delta touches 27 files.

Integrate the complete upstream delta through that pinned commit. Preserve remote SSH hosts and host context, vendored-copy deployments, extracted modules, pnpm, deliberate documentation/locale deletions, and existing Changesets/release workflows. Do not restore obsolete monolithic source files, npm lockfiles, or deleted promotional assets. Avoid unrelated refactors and new hashing.

## Required outcomes

- Confirm deployment ownership before removal; preserve user replacements and unrelated targets.
- Disabling one agent or preset preserves deployments still needed by another agent.
- Nested skill imports preserve category paths and existing siblings.
- CLI dry runs detect actual deployment conflicts without modifying files.
- Library relocation handles pending moves, link repair, retries, and concurrent processes while retaining accurate local restart and remote reconnect behavior.

## Conflict resolution map

| Upstream source | Fork destination and decision |
| --- | --- |
| `src-tauri/src/bin/skills-manager-cli.rs` | Adapt into `src-tauri/src/bin/skills-manager-cli/main.rs`, `repo.rs`, `presets.rs`, and `skills/deploy.rs` according to responsibility. Preserve CLI JSON contracts. |
| `src-tauri/src/commands/skills.rs` | Port deletion changes into `src-tauri/src/commands/skills/library.rs`; retain the extracted layout and `HostCtx` boundary. |
| `src/views/Settings.tsx` | Adapt library-path behavior in `src/views/settings/LibrarySection.tsx`; retain host-aware reconnect messaging. |
| Sync, tools, central repository, scenario service | Resolve each behavior at its current owner. Inspect clean merges too, including app state, CLI bridge, startup, API bindings, and English strings. |
| Version, changelog, deleted files | Add a patch Changeset for `skills-manager`; let the fork's version workflow update version files. Retain deliberate deletions. |

## Implementation sequence

1. Record structured approval. Recheck branch, worktree status, and pinned commit availability; preserve unrelated work. Stop for unexpected history changes or an existing merge/rebase. Do not create another worktree to bypass a conflict.
2. Manually merge the pinned upstream commit on the current integration branch. Resolve known conflicts semantically and inspect clean merges for assumptions broken by the fork's module and host boundaries.
3. Adapt upstream regression tests and add fork-specific coverage where architecture changes behavior. Add the Changeset and review the complete diff against both parents.
4. Run the verification below. Fix integration regressions, report independent failures separately, and complete any repository-required review.
5. Present the concrete diff and validation evidence. Publication, GitHub merge, and release remain separate actions requiring authorization. No live library migration belongs in validation.

Codex owns implementation and evidence collection; Yi owns approval and unresolved product decisions. Conflicting intended behavior returns to Yi instead of silently discarding either side.

## Verification and acceptance

Use temporary filesystem fixtures. Retain meaningful upstream tests; extend them for ownership checks across agent removal, skill deletion, and preset toggling; shared paths across supported copy/link modes; nested imports; and non-mutating CLI conflict previews. Cover relocation pending state, retry/failure behavior, link repair, and concurrent access. Check local restart and remote reconnect routing with mocks or temporary fixtures, without moving the user's real library. Existing CLI JSON, host-routing, and vendored-copy tests must remain green. Add focused UI coverage for changed relocation state or messaging.

Run `pnpm install --frozen-lockfile` if installation is needed, then `pnpm lint`, `pnpm build`, `pnpm test`, and `pnpm e2e`. Run `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, and `cargo test --manifest-path src-tauri/Cargo.toml`. Run `pnpm changeset status --since=origin/main` against the verified PR base. Check the final diff for whitespace errors and unresolved conflict markers.

Acceptance requires the intended behaviors, no unrelated changes or restored deleted files, and transparent results for every gate. Repository Rust CI tests macOS and Windows; local Linux checks cannot prove those platforms. Report remote CI, review results, publication, and release separately from local validation. Missing system libraries or browser dependencies block particular checks; they are not passing evidence.

## Review decision

`plan.md` is canonical and `plan.html` is its rendering. Open the HTML with Plannotator using `--gate --json --require-approval --result-file review.json`. Record the structured decision. If changes are requested, revise both files and repeat review. Start implementation only after approval.
