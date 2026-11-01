# A+C workspace implementation

Implement the approved A+C direction: a stable workspace identity with resource descriptions and agent presence in the same collection. Content inspection stays optional. Owner: Codex.

## Decisions already made

- Keep the restrained visual identity, compact typography and existing resource vocabulary.
- Keep host, Library / global agent / project identity, and resource tabs visible across Skills, Instructions and MCPs.
- Make the agent matrix the primary collection view. Keep existing grid/list choices available where they already exist; respect deliberately saved preferences.
- Separate viewing a preset, switching the shared active preset, and adding preset skills to a project. Use explicit actions and a named scope.
- Stage matrix target changes, review them, then apply. Source/content inspection is an optional action rather than a permanent inspector.
- Reference: prototypes/2026-11-01-workspace-consistency/combined-prototype.html. It establishes layout and interaction intent; its sample data and simulated backend behavior are not implementation contracts.

## 1. Establish the common workspace structure

Dependency: approval of this plan. Owner: Codex.

Extract a small shared page header and collection control pattern. Give ResourceWorkspace a persistent header outside the resource-specific subtree. Library, single-agent, All Agents, linked and project workspaces retain their own identity and capabilities. Use accessible resource tabs with history and reload behavior intact.

Move search, filters, display controls and resource actions into consistent positions. Keep tags, sorting, grouping, preset membership editing, imports, updates and existing bulk workflows available. Keep the host banner and captured host/project boundaries.

Acceptance: switching resource tabs preserves the page heading and path; browser history and reload select the same resource; the 1100px desktop minimum and larger text settings remain usable.

## 2. Implement resource-to-agent presence controls

Dependency: common structure. Owner: Codex.

Use one small matrix presentation for names, descriptions, status, optional details and named agent columns. Keep the resource column readable when many agents require contained horizontal scrolling. Actual presence is separate from preset membership and per-agent preset toggles. A disabled project copy is still present.

Adapt Skills to existing Library targets, global scans and project variants. Preserve local-only entries, nested paths, vendored-copy dependencies, manual agent assignments and linked-workspace constraints. Do not infer identity solely from a display name. An unavailable inventory is an error or unknown state, not an empty cell.

Adapt Instructions and MCPs to their existing inventory/capability APIs and reviewed operations. Show unsupported targets with the reason. Keep editors, content, file trees, import/catalog tools and credentials-reference fields available on demand rather than occupying the collection by default. Preserve instruction worktree/directory scope, MCP overrides and explicit conflict choices. Adding an MCP definition never implies authentication or a verified connection.

Acceptance: keyboard users can identify resource, target and current state; cell changes only stage work; local-only/disabled/vendored/unsupported states remain distinguishable; opening details performs no mutation.

## 3. Review and execute changes with explicit scope

Dependency: resource adapters. Owner: Codex.

Keep staged work when searching or filtering, including hidden rows. Ask before discarding it when changing host, project, agent, resource or viewed preset. Review names the host, global/project destination, resources, agents, additions, removals and preserved local edits.

Capture the reviewed host and target for the complete operation. Recheck relevant inventory and existing removal protections before mutation; changed or unsupported targets require a refreshed review. Reuse existing deployment previews, conflict resolution and edit-preservation logic. Execute independent operations sequentially where the current APIs require it; do not claim atomic batch rollback. Partial failure keeps failed/unattempted changes available for retry and does not replay successful operations. Prevent duplicate submission and preserve review on failure.

Acceptance: no mutating backend call occurs before approval; cancel keeps the draft; failure and retry retain correct targets; switching hosts cannot send queued work to the new host; edited copies and vendored dependencies retain existing protections.

## 4. Make preset actions explicit and finish visual consistency

Dependency: common structure and reviewed operations. Owner: Codex.

Keep viewed and shared active preset visible with different meanings. Replace mutation-like preset pills with explicit Switch shared preset and Add preset skills to project actions. Preserve current shared preset, participating-host and per-agent semantics. Review additions as well as removals before applying a shared switch; project additions leave the shared active preset unchanged.

Use the existing modal and status patterns for pending, success, error and recovery. Replace undefined app-button references with the shared variants. Separate readable action foreground/background colors from accent colors, and give named controls consistent selected, focus and disabled states. Keep changes within the approved workspace/preset scope; no unrelated settings or Sync redesign.

Acceptance: browsing a preset never switches agents; applying a project preset never switches the shared preset; shared switches require a concrete impact review; primary controls remain legible in both themes.

## Verification and completion

- Add focused behavior tests for staged operations, failed-only retry, stale inventory and captured host/target execution. Reuse current fixtures and existing helpers.
- Extend browser coverage for all three resources across Library, agent and project workspaces; cover header persistence, optional details, explicit preset actions, filter-hidden drafts, guarded navigation, capability errors, project edit protection and keyboard operation.
- Run bun run build, bun run lint, bun run test, formatting checks for changed files, and the affected Playwright suites. Resolve failures caused by this change; report unrelated or environment blockers precisely.
- Inspect desktop layouts at 1440px and 1100px in light/dark themes and default/large text. Run the Impeccable detector on changed UI after completion and assess advisories in the context of a compact desktop tool.
- Browser fixtures prove interface behavior only. Report real Tauri/filesystem/SSH limitations separately. Preserve the approved prototypes and unrelated working-tree changes. No push, PR, release or deployment is included.

## Source and operation mappings

- src/components/ResourceWorkspace.tsx: current resource tabs, capability queries, instruction deployments, MCP previews and optional editors.
- src/views/MySkills.tsx and LibrarySkillRow/Card.tsx: Library queries, actual skill targets, sorting/grouping, preset membership and immediate target actions to adapt.
- src/views/WorkspaceView.tsx: global scans, managed/local-only operations, agent categories and existing preset switch hook.
- src/views/ProjectDetail.tsx: project grouping, manual agent assignments, vendored locks and fresh-disk removal checks.
- src/hooks/usePresetSwitch.tsx and src/components/PresetBar.tsx: shared switch impact review and project add/remove semantics.
- src/lib/hostCall.ts, src/components/CloseActionGuard.tsx and router: captured host invocation, pending-work integration and guarded navigation.
- e2e/specs/resource-workspace.spec.ts, workspace-bulk.spec.ts, critique-remediation.spec.ts, project-detail.spec.ts and library.spec.ts: regression coverage to preserve and extend.
- CONTEXT.md is the terminology authority. Existing backend contracts remain authoritative; no schema migration, new credentials, hashing or backend redesign is planned.
