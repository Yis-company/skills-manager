# Interactive operation contract (SKI-20)

Desktop `operation_request({ request })` and `host_dispatch::dispatch(ctx,
"operation_request", args)` run the same backend service owned by `HostCtx`.
`invokeHost` captures the originating host. React owns observation, never an
item execution loop. This slice admits project updates only.

## Requests

All request fields use snake_case inside the `request` object.

| action | fields | result |
| --- | --- | --- |
| `submit_project_update` | `operation_id`, `host_id` (null for Local), `project_id`, optional `skill_paths` | `OperationSnapshot` |
| `read` | `operation_id` | authoritative snapshot |
| `list` | none | session-owned snapshots |
| `cancel` | `operation_id` | snapshot acknowledging the request; inspect `settled` before assuming stopping finished |
| `retry_failed` | `operation_id`, a fresh `attempt_id` | same operation, only failed items admitted again |
| `reconcile` | `operation_id` | live snapshot or correlated host-local receipt; never replays a write |

Omitting `skill_paths` captures every eligible project copy, including copies
hidden by view filters. Supplying paths selects logical skills, including their
eligible per-agent copies. Edited copies become `needs_review`; current and
project-only copies are not admitted. Vendored aliases follow their owned copy.

Callers supply an operation identity before submitting. Repeating it reattaches.
An equivalent active selection in the same project also returns the existing
operation. Partly overlapping batches and physical-folder aliases wait on the
same destination; once the destination already contains the captured source,
the later item reports `unchanged` without another write.

## Snapshots and items

Rust DTOs: `OperationSnapshot`, `OperationItem`, `Phase`, `OperationRequest` in
`src-tauri/src/core/operations.rs`. TypeScript mirrors are in `src/lib/operations.ts`.

A snapshot contains `operation_id`, current `attempt_id`, original `host_id`,
`project_id`, `project_path`, monotonically increasing `revision`, `settled`,
`cancel_requested`, and `items`. An item has a stable `id`, the `attempt_id`
that last executed it, `relative_path`, `agent`, `phase`, and optional `message`.
Successful siblings keep their attempt identity and outcome when failures retry.
Repeated retry identities do not rerun an attempt.

Active phases are `queued`, `preparing`, `waiting`, `applying`, `stopping`, and
`checking_result`. Settled outcomes are `succeeded`, `unchanged`, `failed`,
`canceled`, `needs_review`, and `outcome_unknown`. Preparation and checking-result
phases leave room for the installation and transport slices; project updates
currently prepare their captured selection before admission.

`operation-changed` carries a snapshot with its original scope and revision.
Events are hints: read/list repairs event loss and remounts. Consumers must reject
older revisions and preserve the original host/project identity. The UI polls
authoritative list snapshots, coalesces reads through React Query, and refreshes
only the original project's skills once each attempt settles. Project sidebar
health is recomputed from that read. Refresh failure is separate from mutation
outcome and its Refresh action does not retry a write.

## Execution and safety

`MAX_ACTIVE_ITEMS = 2` bounds actual item execution. One backend coordinator
admits independent destinations and revisits lock waits without consuming worker
slots. Cancellation cancels queued/waiting items immediately, checks again before
apply, and never interrupts a critical filesystem replacement. Current writes
finish before their outcomes settle; completed changes are retained.

`CapturedProjectUpdate` is an internal host receipt, not a client DTO. It captures
source and physical destination identity, project/deployment intent and existing
content-comparison evidence. Immediately before apply, the implementation checks
the current project, library match, source, destination and mode, then invokes
the existing guarded Rust update body. Changes return to review. Failed-only
retry does not refresh old confirmation authority. No new hash scheme is used;
comparison reuses the incumbent project scanner/content evidence.

The existing repository lock gains shared readers for project updates. Library
writers keep their existing exclusive lock, while independent project copies
can read concurrently. `ProjectUpdateLock` also takes a cross-process exclusive
destination lock. Its key resolves physical parent-folder aliases; a final link
is replaced at its own location. Lock files stay outside projects under the host
base directory. Keys use reversible chunked path encoding, not hashing. Both the
legacy direct update command and operation workers take this protection.
Project toggle/removal commands take the same destination protection; imports
from a project take the existing exclusive repository guard.
Existing copy/link/linked-workspace, vendored-link, path, local-edit and replacement
checks remain in the shared Rust command bodies.

Receipts use existing host-local SQLite `resource_state`, kind
`operation_receipt` keyed by operation identity. Intent is committed before
apply and results afterward. Before failed-only retry, kind `operation_attempt`
preserves the previous attempt's correlated snapshot and evidence. These records
are outside portable Sync metadata and contain local project/library evidence,
not transport credentials. Failure to persist intent prevents a write; failure
to persist a result yields `outcome_unknown`. There is no exactly-once claim
across a crash between filesystem mutation and receipt persistence.

An explicit read/reconcile can inspect a receipt without an in-memory owner.
Recorded settled outcomes remain inspectable; unfinished phases become
`outcome_unknown` in the response rather than being replayed or assumed active.
This is receipt groundwork, not automatic remote reconnection or result restoration.

## Slice boundary

SKI-21 adds installation-source admission to this service. SKI-22 owns retained
multi-host sessions and observation across host switches. SKI-23 owns dropped
transport-response reconciliation. SKI-24 owns restart result restoration and
retention policy. SKI-25 owns complete app quit/restart coordination. None of
those flows is simulated as production behavior here. List is session-owned;
receipts are available explicitly by identity, without automatic resume.

Tests execute the real contract against temporary stores/project folders and
through the real CLI stdio harness. Browser fixtures return controlled contract
snapshots; they do not implement a second scheduler or prove filesystem safety.
