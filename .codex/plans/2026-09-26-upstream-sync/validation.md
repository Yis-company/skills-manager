# Upstream integration validation

Integrated upstream `1d41f1878b8d4506d57e1c2f19de866ea52febc7` (v1.40.1)
into fork head `54dea85f3cf01533bb1a1e31fdcf1cc419876fcd` on
`t3code/check-fork-sync-value`. The structured plan decision is in `review.json`.

Preserved the fork's extracted modules, SSH host routing, project copy deployment,
pnpm setup, deliberate file deletions, and Changesets release process. Added a patch
Changeset instead of copying upstream's release version changes.

## Final local checks

- `pnpm lint`: passed.
- `pnpm build`: passed; existing bundle-size and Browserslist warnings remain.
- `pnpm test`: 181 passed.
- `CI=1 pnpm e2e`: 34 passed, including pending library-path display. These browser tests use the repository's fake Tauri backend.
- `cargo test --manifest-path src-tauri/Cargo.toml`: 627 passed, 6 ignored network tests.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`: passed.
- `pnpm changeset status --since=origin/main`: passed; patch release for `skills-manager`.
- Conflict-marker, unmerged-index, and diff-whitespace checks: clean.

Regression coverage includes actual preset-toggle command calls, user replacement
preservation, shared deployments, nested imports, all three CLI deployment dry-run
paths, and library relocation through real CLI/serve subprocesses on temporary
libraries. Remote pending-path reads route through the selected host. Reconnecting
a server applies its pending move; ordinary CLI reads do not. The subprocess test
also verifies that an active server lease prevents an explicit CLI move.

## Limits

These results are from Linux. macOS and Windows CI, remote CI, CodeRabbit review,
release packaging, and deployment were not run. The optional real SSH-to-localhost
smoke test is environment-gated; no external SSH host was validated. No live user
library was migrated. No push, PR, or release was performed.
