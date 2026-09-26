# TanStack Query for shared app data

Keep loaded content visible during refresh and make confirmed deletion visible immediately. Adopt one query cache for core backend reads and project skills, with strict separation between hosts.

## Approval and scope

This is the canonical implementation plan. Implementation starts only after the structured Plannotator review approves it. The implementer owns code and verification; Yi approves scope through review. No timeline is proposed.

Adopt Query for presets with active preset, tools, managed skills, projects, local remote-host metadata, and project skills. Keep AppContext as the consumer-facing facade. Keep host connection state, viewed-preset selection, dialogs and updater state outside the cache. Workspace-local scans, market, documents, backup and settings stay outside this migration; verify their existing behavior through the facade. No persistence, offline queue, generic cache framework, backend command or schema changes.

## Evidence

- `src/context/AppContext.tsx:121–210` manually owns shared backend data. Every refreshAppData sets global loading; app-files-changed triggers it. Managed-skill refresh also starts an unawaited project refresh, duplicating the app-wide path.
- `src/hooks/useProjectSkills.ts:5–33` sets loading on every load; `src/views/ProjectDetail.tsx:1019` then replaces the list. Its request counter protects project changes but does not identify hosts.
- `src/views/MySkills.tsx:397–477` retains content during deletion and coalesces single-delete refreshes for 300 ms. Preserve that useful visible-content behavior.
- `src/lib/hostCall.ts:5–48` routes through mutable active-host state. `AppContext.tsx:218–280` changes that state synchronously before awaiting refresh. Query keys alone cannot bind requests to the correct destination.
- `src-tauri/src/core/skill_delete.rs:16–55` returns failed IDs on fulfilled batch deletion, but can reject after partial writes. A rejected batch does not identify successful deletions.
- `e2e/fixtures.ts` provides seeded state, call recording, hold/release, failures and events; extend this fake only where needed.

## 1. Establish host ownership

Add one stable QueryClient above AppProvider and a small typed module for the concrete query keys/options. Use `['host', hostId, resource]`, with null meaning this computer; add project ID for project skills. Remote-host metadata uses a local-only key. Read presets and active preset together to preserve coupled behavior.

Bind each adopted query and deletion mutation to its captured destination host. Add the smallest explicit-host transport entry point needed, preserving remote_invoke payloads and local-only command rules. Never temporarily change the global active host to issue a request. Unrelated API callers keep their interface.

Host switching passes its destination explicitly to refresh work after enterHost, rather than using a prior render's captured host. Old results may finish into their original cache but cannot change current data, errors, selection or dialogs. Keep connection sequence guards. Render only destination-key data, without previous-host placeholders. Cancel or detach old observers; cancelling a query does not mean the Tauri backend operation stopped.

Configure desktop IPC deliberately: retry false, networkMode always, no focus/reconnect refetch, and staleTime zero so navigation refreshes while retaining same-key cached data. Mutations do not retry and use networkMode always. Stable providers/options must not cause repeated automatic requests.

## 2. Replace shared data state

Derive adopted data from queries and remove duplicate useState arrays. Retain the existing Promise-returning refreshAppData, refreshPresets, refreshTools, refreshManagedSkills, refreshProjects and refreshRemoteHosts functions as compatibility wrappers around exact query refreshes.

Await the work each wrapper promises. refreshManagedSkills also awaits project health refresh; refreshAppData schedules each resource once instead of nesting duplicate project refreshes. Preserve handled-error behavior at the facade while surfacing persistent query errors. Success for one resource must not clear another resource's failure. Concurrent same-key requests share work; a mutation or event arriving during an older read still requires a fresh authoritative read afterward.

Loading means initial acquisition with no usable data. Background refresh retains content and uses an existing small updating affordance where appropriate. Initial failures settle into visible error/retry states; background failures retain usable data with an error. Empty successful data remains the normal empty state.

Preserve per-host stored viewed-preset selection and the rule that an external active-preset change follows only when the viewer was on the former active preset. Preserve the once-per-launch app updater and current local-only skill-update behavior. Query rebinding must not restart startup work or accumulate event listeners.

## 3. Migrate project skills and deletion

Replace useProjectSkills state with the host/project query, preserving its awaited loadSkills API and separating initial loading from fetching. Never reuse another project's list. Wait for project metadata to settle before declaring the project missing.

After backend-confirmed deletion, cancel stale in-flight list reads, remove only confirmed rows from the originating host's cache, then revalidate affected data. Capture host and project when the operation starts. Selection/detail cleanup applies only to the still-relevant view. Do not remove rows before success.

Managed single deletion removes its confirmed ID. A fulfilled managed batch removes requested IDs absent from the backend failed-ID list, retaining failed rows and feedback. A rejected batch has unknown successes: retain rows until authoritative revalidation and show the error. For project deletion across copies, record each confirmed variant using its actual path/agent identity; partial failures must not remove unconfirmed copies. Preserve whole-skill semantics and existing summaries.

Managed deletion refreshes managed skills, presets and project health. Project deletion refreshes project skills and the project list. Preserve existing refresh paths for other mutations/events. Keep the single-delete debounce only if useful for coalescing; it cannot delay confirmed cache removal or let an old-host callback refresh the new host. Avoid speculative invalidation of every query.

## 4. Prove the boundaries

Use the existing Playwright fake backend and focused host/query unit tests; add only necessary fixtures.

- Hold post-delete reads: confirmed deleted rows disappear, unaffected rows stay visible, and neither shell nor project list becomes a loading-only screen. Cover managed single/batch and project deletion, including a failed variant.
- Reject deletion: rows remain and failure is visible. Reject a batch after partial writes and verify reconciliation without invented successful IDs.
- Hold background refresh: content remains; initial reads show loading. Initial and background failures have distinct usable states; successful empty data shows the empty state.
- Switch host/project with reads in flight and colliding IDs: old responses cannot replace current data or clear current errors. Deletion finishing after a switch updates only its originating cache. Assert remote_invoke receives the explicit destination.
- Trigger concurrent refreshes and file events: same-key work deduplicates, while changes arriving during old reads eventually appear. Check preset counts and project badges after deletion.
- Preserve viewed-preset behavior, updater call count and workspace consumers of shared data.

Run `pnpm test`, `pnpm lint`, `pnpm build` and relevant library/project/sidebar/preset Playwright suites plus the new host cases. Broaden once to the existing E2E suite if shared startup changes warrant it. Record exact outcomes and environment limitations; local checks do not establish native remote connectivity or release readiness.

## Risks and rollback

Primary risks are wrong-host execution during transitions, old reads restoring deleted rows, and consumers treating handled refresh errors as success. Explicit destination binding, cancellation with authoritative refetch, and focused error tests address these without backend changes. Audit refresh call sites before replacing each facade method.

Rollback reverts the frontend dependency/lockfile and migration changes. Backend commands, schema and on-disk data format remain unchanged.

## Design and sources

The review page uses repository zinc surfaces and emerald accents from src/index.css, system sans-serif for reading and monospace for code, and a restrained document layout. Both light and dark themes follow the OS with an explicit override. Numbered sections represent implementation dependency order, not a schedule.

TanStack documentation checked for assumptions: [important defaults](https://tanstack.com/query/latest/docs/framework/react/guides/important-defaults) and [query cancellation](https://tanstack.com/query/latest/docs/framework/react/guides/query-cancellation). Desktop IPC requires explicit defaults, and cancellation does not imply transport cancellation.
