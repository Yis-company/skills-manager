# Port selected upstream fixes

## Decision
Create one PR against Yis-company/skills-manager main from feature/review-fork-parent-changes. Adapt selected upstream fixes through fccb4e8 to the fork at 241a807. Codex owns implementation, regression coverage, validation and the PR; Yi approves this scope before code changes.

## Scope and sequence
1. Safer workspace removal: add a confirmation before the library detail's Added chip removes a workspace skill. Escape cancels only the confirmation. Preserve vendored-copy restrictions and host-aware API dispatch; protect locally edited project copies with a fresh status check where the upstream deletion safeguards apply.
2. Safer library relocation: reject a non-empty destination or a destination inside the library when saving Settings. Resolve symlinked ancestors in both save-time and startup checks. Keep configuration unchanged on refusal and show a useful English message. Preserve pending-move and remote-host behavior.
3. Accurate monorepo updates: compare the installed skill subtree and ancestor .gitattributes, so unrelated repository commits do not offer skill updates. Batch comparisons per remote outside the repository lock. A failed comparison is a failed check; root installs and already-current revisions keep their behavior. Adapt to core/skill_update_check.rs and commands/skills/update_check.rs.
4. Small compatibility fixes: normalize owner/repo.git, ssh:// and case-insensitive Git schemes; strip CRLF, empty and EOF-terminated Markdown frontmatter; fall back to plain Markdown on engines without regex lookbehind.

## Dependencies and exclusions
Use the existing confirmation dialog, pnpm, Vitest, Playwright fake backend, Rust core and Changesets. Include the selected upstream follow-ups and source attribution. No upstream merge, database migration, hash cache, new agent adapters, locales, UI redesign, Sync/join replacement or release-workflow changes. Existing host routing and edited-copy protection remain authoritative. Add a patch changeset without changing the package version.

## Acceptance
- Cancellation leaves the workspace intact; confirmation removes only the intended skill; Escape preserves the detail sheet. Existing vendored-copy behavior stays intact.
- Refused relocation leaves config unchanged. Empty or absent destinations are accepted; targets nested through symlinks are refused at save and startup.
- Unrelated monorepo commits stay up to date; skill changes and ancestor .gitattributes changes offer updates; failed subtree comparisons report errors and retry. Single and batch checks stay off-lock during network I/O.
- URL and Markdown regressions have focused behavior tests, including supported-engine GFM rendering.
- Run pnpm format:check, pnpm lint, pnpm test, pnpm build, relevant Playwright specs, cargo fmt --check, cargo test and cargo clippy --all-targets -- -D warnings where supported. Report dependency, platform or runtime blockers precisely; remote CI supplies Windows/Linux evidence.
- Review the final diff, commit and push the branch, open and link one PR. Do not merge it.

## Review evidence
The fork has split upstream's skill/project modules, replaced clone/reclone with Sync join, and uses database schema 12. Upstream's schema-9 cache and npm/release changes conflict with those choices. Local dependency directories are absent, so validation requires installing the pinned toolchain dependencies. Upstream Rust CI passed for fccb4e8; the adapted fork still needs its own checks.

## Sources
- Workspace removal: upstream 95d7d6d, e5f9683d, 811a1f56 and 1a814f3d.
- Library relocation: upstream 3c6995de.
- Monorepo checks: upstream 53dc88c5, a14e3211 and fccb4e8a.
- URL and Markdown: upstream 077eed84, 282f0fd6 and 478035a7.
