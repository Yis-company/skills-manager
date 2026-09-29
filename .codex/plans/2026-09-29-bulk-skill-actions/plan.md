# Bulk skill install and removal

Review proposal · 29 September 2026

## Intended result
Select several skills in Install Skills and install them together. Make bulk removal easy to find in project and global agent workspaces. Reuse existing operations and selection components.

## Current behavior and scope
- Catalog installs are single-item today: src/views/InstallSkills.tsx:212 and src/components/MarketTab.tsx:233. Add bulk selection here.
- Git preview already selects multiple skill folders; local import already supports a folder of skills. Preserve these flows without adding another batch API.
- Project bulk deletion already exists, but is hidden under More actions (src/views/ProjectDetail.tsx:1094). Promote it to the visible selection toolbar as Remove selected (N).
- Global agent workspaces already provide separate batch actions for managed links and local files (src/views/WorkspaceView.tsx:1103). Make their entry point and labels explicit; preserve their different effects.
- Global scope for this proposal means the skill list inside one agent workspace. The global overview currently contains agent cards, not a combined skill list. Cross-agent selection would require a separate scope decision; the clarification is pending.

## Recommended interaction
Use the existing Select mode, checkboxes, count, select-all control and toolbar. Label the entry point Select skills on the affected screens. Project and global selections work in grid and list views. Existing single-item actions continue to work outside selection mode.

Catalog: Select skills → choose cards → Install selected (N). Already installed items are visibly disabled for selection, using the existing source/skill identity. Select all on this page selects only eligible cards on the current 24-item page. Moving pages clears selection; search/source/leaderboard changes prune or clear selection so hidden cards cannot be installed accidentally. Changing host or leaving the source tab clears selection.

Project: Select skills → choose rows → Remove selected (N), directly visible. The confirmation names the project and selected skills, explains that project copies across the row's agent variants will be removed, and that central library copies remain. Local-only project files are permanently deleted. Preserve existing grouped/vendored-copy behavior and single-item deletion semantics.

Global agent workspace: Select skills → choose rows → Remove from [agent] (N) for managed library deployments, and Delete local files (N) for unmanaged copies. Both remain inline when a mixed selection is present, each with its own matching count and confirmation. Confirmations identify the agent and affected skills; local-file deletion explicitly says permanent, while managed removal preserves the library. A finished action clears only its successful items so the other selected category remains available.

## Batch execution and results
- Reuse existing single-skill install and removal commands. Run catalog installs sequentially; do not loop the current UI handler because it catches errors and refreshes after every item. Extract only the small operation needed to return each result and reuse single-install progress/cancellation where practical.
- Snapshot selected identities and the originating host/project/agent when confirming or starting. Bind all later commands and refreshes to that host using existing invokeHost/query-key patterns. Never let a host switch redirect the remaining queue.
- Disable conflicting actions and selection changes while running. Show completed/total progress for catalog installs. Stop installation cancels the current install via the existing cancel key and prevents later items from starting; previously completed installs remain.
- Continue after individual failures. Show one summary with installed/removed, failed, and stopped/not-started counts where applicable. Retain a compact per-item failure list with error text and retry through the normal action, limited to failed or unstarted items.
- Refresh authoritative state after the batch, including partial failure/cancellation. A refresh failure must not label a successful mutation as failed or cause it to be retried. Remove successful items from selection; retain failures when still in the originating view. Suppress stale view updates after navigation.
- Preserve backend collision, shared-path, and persistent unsync protections. Do not add forced overwrite or rollback promises. No new hashing, schema, CLI command, or batch backend is needed.

## Alternatives considered
- Recommended: reuse existing selection UI and operations, expose removal clearly, add catalog batching. Small scope and consistent behavior.
- Combine managed and unmanaged global removal into one action. Fewer clicks, but combines unlinking with permanent file deletion; keep explicit actions.
- Add a cross-agent global skill table and a new batch backend. Broader navigation and data model work than required for the per-agent scope; defer unless that is the requested global workflow.

## Implementation slices after approval
1. Catalog selection and execution: InstallSkills.tsx, MarketTab.tsx, existing selection/toolbar helpers as needed, localized labels in src/i18n/en.json. Add a small operation helper only if needed for reliable results and reuse.
2. Workspace controls: ProjectDetail.tsx and WorkspaceView.tsx. Promote project removal, improve scope copy, retain the two global effects, preserve remaining selection, and capture host scope for touched batch operations. Keep shared helper changes narrow and compatible with My Skills.
3. Tests and release note: extend e2e/specs/market.spec.ts and focused project/global specs plus fake-backend behavior where needed. Add a changeset per repository requirements. No real user skill installs or deletions during validation.

## Acceptance checks
- Select two catalog skills and install both; installed cards cannot be queued again. Select all applies only to the visible page. Search, source, page and host changes cannot carry invisible or foreign-scope selections.
- A middle install failure does not prevent later installs. Successful entries are not retried. Stop prevents unstarted work and produces an accurate summary. Double clicks cannot start duplicate batches.
- Project Remove selected is directly visible; cancel makes no calls. Confirmation scopes deletion to that project. Grouped variants and vendored aliases retain existing correct behavior under partial failures.
- Global managed removal keeps the library and other agents intact. Local removal uses only the local-delete operation. A mixed selection displays the separate counts, executes only the confirmed category, and preserves the other category's selection.
- Host/agent/project switches during delayed operations keep all writes and refreshes on the original target. Failures remain visible and actionable; stale completion does not change the next workspace.
- Keyboard selection and Escape remain usable; a dialog owns Escape before selection mode. Toolbars wrap at narrow widths without hiding the removal action.

## Validation and approval boundary
After approval, run pnpm lint, pnpm build, pnpm test, and pnpm e2e, installing dependencies with the frozen lockfile if needed. Use fake backend fixtures and temporary paths. Report browser evidence separately from native desktop or real SSH validation. Rust checks are needed only if backend code changes become necessary.

This is a proposal; no application implementation has started. The user-provided AGENTS.md Visual Plan Review instructions and html-plan skill require a structured approved Plannotator decision before implementation. Record it in review.json; revise and repeat for requested changes. No commit, push, release, or real-user skill modification is included.
