# A+C workspace validation

The user approved this implementation plan in chat on 2026-11-01. The implementation preserves the approved standalone prototypes.

## Implemented behavior

- A persistent host/workspace heading and resource tabs across Library, global agent, All Agents, linked and project workspaces.
- Shared agent matrix presentation for Skills, Instructions and MCP definitions, with optional source/actions or native editors. Existing grid/list workflows remain available.
- Explicit distinction between viewed presets, shared active preset switches and project preset additions/removals. Shared switches now always show their concrete impact review.
- Matrix changes stage without applying. Review captures destinations and target pairs; navigation, host switches, viewed-preset changes and actual app exit protect pending work. Hiding to the tray preserves the draft.
- Fresh target validation before the first write, captured host invocation, failed-only retry, preserved user intent after inventory changes, unavailable/unsupported states and existing edit/vendored protections.
- Instructions/MCP cells describe managed deployments; native files, imports, conflict choices, worktrees, recovery and credential-reference configuration remain in their optional editors.
- Readable primary action colors in both themes and an inline project onboarding hint that no longer obstructs preset actions.

## Checks

- `bun run build`: passed. Existing large-bundle and stale Browserslist advisories remain; no build error.
- `bun run lint`: passed without diagnostics.
- `bun run test`: 25 files, 218 tests passed.
- `bun run e2e -- --workers=4`: all 138 tests passed in the final full run. This includes 17 matrix scenarios and retained Library/project/resource/bulk/host/query workflows.
- `bunx oxfmt --check <changed source and test files>`: passed for all 29 files.
- `git diff --check`: passed.
- Impeccable detector over changed UI targets: zero source findings. This source scan is not an accessibility compliance claim.

## Rendered evidence

Browser scenarios exercise 1440px and 1100px desktops, light/dark themes, and default/xlarge text. Computed primary-button text contrast is at least 4.5:1 at rest and on hover in both themes. Keyboard staging, dialog focus, resource navigation, empty filters, inventory failure, failed-only retry, stale review and host guarding are covered.

The existing Library screenshots under `.impeccable/review/2026-11-01-remediation/` were refreshed by their regression scenarios. Additional temporary captures: `/tmp/agents-manager-matrix-light-large.png` and `/tmp/agents-manager-matrix-dark-large.png`.

## Evidence limits

Browser tests use the repository fake backend. Real Tauri window close/tray behavior, filesystem mutations and SSH host operations were not executed against live data. Existing backend review/precondition/edit-preservation contracts are reused; there is no new schema, credential use, dependency or hashing. No push, PR, release or deployment was performed.
