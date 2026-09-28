# Validation — workspace updates and persistent removal

Approval: `review-network.json` records `{"decision":"approved"}`.

## Delivered
- Project Update All processes all eligible deployed effective copies, independent of selection and filters. Local-newer and diverged copies are skipped; mixed groups retain per-agent identity.
- Individual, selected, and all-project pulls share the safe candidate policy. Commands and refreshes stay bound to the originating host/project. Pending state and partial-failure feedback are covered.
- Global enable/disable writes per-agent scenario state to sync metadata under the existing repository lock. Removal I/O errors propagate without dropping the target record.
- Project pull rejects local-newer and diverged content at the backend, including vendored copies.

## Evidence
- Regression with the disable metadata write omitted failed after reindex: agent_a was enabled instead of disabled. Restoring the write passed.
- Same-database restart tests cover removing one shared agent, retaining its peer, removing the final agent, startup without restoration, and explicit re-enable surviving restart.
- Failure tests cover filesystem removal errors, metadata write errors, and preservation of independently replaced content.
- Browser tests cover hidden skills, mixed-status groups, partial failures, pending actions, and switching hosts during an update sequence.

## Passed checks
- `pnpm test`: 19 files, 189 tests.
- `pnpm e2e --workers=2`: 58 tests.
- `pnpm lint` and `pnpm build`.
- `pnpm exec vitest run src/lib/projectSkillGroups.test.ts`: 25 tests after the final helper reuse change.
- Rust sync command tests: 9 passed.
- Rust project command tests: 21 passed.
- Rust Clippy (`--lib --tests -- -D warnings`) and formatting checks.
- Changeset validation and `git diff --check`.

## Boundaries
Validation used Linux and isolated test data. The installed desktop app, actual user skill directories, remote hosts, macOS/Windows runtime, and remote CI were not exercised. No release or deployment was performed. The startup source-reference backfill was left unchanged because the reproduced failure was stale scenario metadata.
