# Interactive operations (SKI-20 through SKI-22)

Desktop `operation_request({ request })` and the host dispatcher run the same
backend-owned `OperationService` on `HostCtx`. React observes authoritative
snapshots; it never executes an item queue. `invokeHost` captures the originating
host. Installation adds only to that host's library. Add to agent and project
export remain separate actions.

## Requests

Fields inside `request` use snake_case.

| action | fields | result |
| --- | --- | --- |
| `submit_project_update` | `operation_id`, `host_id` (null for Local), `project_id`, optional `skill_paths` | snapshot |
| `submit_install` | `operation_id`, `host_id`, `sources` | snapshot |
| `confirm_install` | original Git preview's `operation_id`, fresh `attempt_id`, selected `items: [{ rel_path, name }]` | snapshot |
| `read` / `reconcile` | `operation_id` | authoritative snapshot / correlated receipt; never replays a write |
| `list` | none | session-owned snapshots |
| `cancel` | `operation_id` | acknowledgement; inspect `settled` before assuming stopping finished |
| `retry_failed` | `operation_id`, fresh `attempt_id` | same operation; only failed items execute again |

Installation sources are tagged objects:

- `{ kind: "marketplace", source: "owner/repository", skill_id: "skill" }`
- `{ kind: "git", url: "https://…" }` prepares one repository for explicit review.
- `{ kind: "local", path: "…", name?: "…", origin?: "discovered" }` imports a
  folder or supported `.zip` / `.skill` archive. Discovered sources retain
  `import` metadata; ordinary local sources retain `local` metadata.
- `{ kind: "local_batch", path: "…" }` captures valid immediate skill folders,
  skips already-recorded prospective library paths, and admits per-skill work.
  It reuses the direct batch import's depth-one validation. No automatic agent scan
  is introduced by the operation service.

Git preparation is submitted separately from other sources. Independent Git
preparations can run concurrently; a single request does not mix preview sessions.
A ready preview is `needs_review`, with `preview: [{ rel_path, name, description }]`.
It holds no worker or library lock while the user selects and renames skills.
`confirm_install` accepts only entries belonging to that live operation's preview,
records a new attempt and queues the selected items. Changed or expired evidence
requires preparing and reviewing again. A receipt alone cannot authorize confirmation.

Project requests remain compatible. Omitting `skill_paths` captures all eligible
copies, including filtered-out copies. Explicit paths select logical skills and
eligible agent copies. Edited copies return to review; current/project-only copies
are not admitted. Copy, link, linked workspace and vendored aliases retain their
existing semantics.

## Snapshots, identity and observation

Rust public types in `core/operations.rs` are `OperationRequest`,
`OperationSnapshot`, `OperationItem`, `OperationWork`, `Destination`, `InstallSource`,
`LocalOrigin` and `Phase`. TypeScript mirrors live in `src/lib/operations.ts`.

New snapshots identify `work: "project_update" | "installation"` and a tagged
`destination: { kind: "project", id, path } | { kind: "library", path }`.
Project snapshots retain `project_id`, `project_path` and item `agent` strings.
Library snapshots use **null** for these project/agent fields, never a fabricated
project or agent identity. `sources` captures sanitized original installation intent;
items retain their specific `source` and per-item outcomes. A batch with no new
folders has an empty settled item set. Older project receipts deserialize with the
project-work default and optional new fields; clients tolerate their absence.

Every snapshot has operation/attempt identity, original `host_id`, monotonically
increasing `revision`, `settled`, `cancel_requested` and `items`. Item identities
remain stable across failed-only retry. Successful/review/unknown siblings keep their
outcome and attempt. Repeated submit/confirm/retry identities reattach rather than
replay. Equivalent active install selections return the existing operation identity;
callers follow that returned identity. A repeated source with a session-owned unknown
result reattaches to that result, and marketplace controls do not offer ordinary
resubmission for it. Automatic verification remains SKI-23. Conflicting destinations serialize, and an
already-matching recorded skill reports `unchanged` without another replacement.

Active phases: `queued`, `preparing`, `waiting`, `applying`, `stopping`,
`checking_result`. Settled outcomes: `succeeded`, `unchanged`, `failed`, `canceled`,
`needs_review`, `outcome_unknown`. No download percentages are invented.

`operation-changed` is a hint. Authoritative read/list polling repairs missed events
and remounts, rejecting older revisions. The provider survives page/project navigation.
Install Skills navigation, source tabs, marketplace items and source summaries expose
the viewed host's work; source summaries remain visible when item filters hide it.
Git review uses the existing shared modal and restores focus to its source operation.

On settlement, refreshes are coalesced through existing query facilities. Installation
refreshes only the original host's managed-library query; it does not rescan projects,
add preset membership or deploy to agents. Project refreshes update only the original
project's skills and derive its sidebar health. A failed refresh stays separate from
successful writes. Refresh retries reads, never mutations.

## Execution and protection

`MAX_ACTIVE_ITEMS = 2` bounds actual item execution across both work variants.
One coordinator admits independent destinations and rotates among records. Library
lock waits hold no worker. Git cache contention yields back to that coordinator,
with a contextual waiting reason, rather than blocking both slots. Existing cache
locks remain cross-process locks; legacy blocking Git callers now check cancellation
between lock attempts. A waiting install cannot starve an unrelated project.

Install preparation runs **without a project shared-reader guard**. Local/archive
staging and Git checkout preparation occur outside the exclusive repository guard.
Apply admission takes the existing exclusive library `RepoLock` through a nonblocking
try-acquire. The unlocked installer/store bodies execute under that writer; there is
no nested shared-to-exclusive lock upgrade. Prepared checkout/staging data is isolated
from the shared cache. Completed/canceled staging is cleaned; unresolved staging remains
available for session retry/review and is cleaned when the service ends. This does not
promise resumable previews after a process restart.

Local source identity and incumbent content evidence, selected Git checkout evidence,
original library path and intended target are revalidated before writing. An edited,
removed, redirected or newly occupied target returns to review as appropriate. Existing
marketplace records must still own their captured path and source and match recorded
content before replacement. Symlink targets and paths escaping the original library
are refused. Collision-safe naming, archive extraction, source containment, copy/link,
removal/ownership and project local-edit protection remain in the shared Rust paths.
No new hash scheme is introduced; evidence reuses the incumbent installer/project
content comparison and Git revision facilities.

Project updates still take a shared repository reader and a cross-process physical
destination guard. Independent copies can read concurrently; library writers remain
exclusive. Physical parent aliases share the existing reversible, chunked path-key
encoding under the host base, outside projects. Direct project update, toggle/removal
and project import guards remain intact.

Cancel removes queued/waiting work immediately. Preparing/applying workers acknowledge
`stopping`, check cancellation before the next safe apply boundary, and settle only
when their work stops or completes. Synchronous replacements are never interrupted
mid-write; completed changes are retained. Item failure does not stop its siblings.
Failed-only retry retains original evidence, so changed sources/targets return to review.
Review/unknown items are not admitted by that shortcut.

## Receipts and slice boundaries

Host-local SQLite `resource_state` kinds `operation_receipt` and `operation_attempt`
retain correlated intent and prior attempts outside portable Sync metadata. Intent
persistence precedes writes. Result persistence failure becomes `outcome_unknown`;
there is no exactly-once claim across a crash between mutation and receipt persistence.

Git URL user-info passwords, HTTP user-info, query and fragment data are excluded from
captured source intent before preparation. Git uses the host's configured authentication;
URL-embedded authentication/query tokens are not retained as operation credentials.
Receipts contain sanitized source, local evidence and per-item results, never persistent
confirmation authority. Explicit read/reconcile of an unowned unfinished receipt returns
unknown without replay; unowned Git previews cannot be confirmed.

## Viewed host and retained sessions

Host switching does not cancel operations. `RemoteSessions` keeps one live connection
per host needed by the viewed context, in-flight calls, active operations or live
failed/review/unknown evidence. Selecting Local or another host releases idle connections;
a failed target connection preserves the previous view and its connection. Parallel
connects for the same host share one attempt. Version/protocol checks remain in the
existing connection path. Ordinary calls never open a connection themselves.

An operation request holds its original connection before its admission response is
known. Observed snapshots update only that connection's operation records, rejecting
older/duplicate revisions. Settlement retains the connection through the original
destination's refresh. `remote_host_release({hostId})` acknowledges that refresh and
releases only idle off-screen connections; concurrent calls, active work and retry/review
evidence still retain their session. Refresh failures retain the connection for a read
retry. `remote_host_disconnect()` selects Local; its optional `hostId` explicitly closes
one idle host for reconnect and refuses to tear down live operation evidence.

`remote_host_retained()` lists existing session-owned hosts so a remounted webview can
observe them without switching views or opening connections. `operation-changed` events
are forwarded with the transport's originating host identity. `OperationsProvider`
independently polls every originating host that still needs observation, including pending
admissions. Events are hints; snapshots remain authoritative. A slow or failed host read
does not await or block another host's reads or actions. Connection unavailability is
shown in host navigation without inventing an operation outcome.

The provider exposes viewed-host `operations` and contextual-navigation `allOperations`.
`act(originHostId, request)` requires the action's owning host; `install(sources, origin)`
accepts an explicit host when resubmitting original evidence. Project and library actions,
Git confirmations and late admission replies retain that identity. Ordinary view queries
and the existing host-keyed page remount stay isolated. Operation caches merge by host,
operation and monotonic revision; refresh markers/errors use a tuple of host, operation
and attempt. Host navigation reveals active work/attention and a compact off-screen
shortcut to the original project or installation source, without a mixed Activity page.

A transport failure conservatively retains its host's operation evidence. The incumbent
viewed-host quiet connection retry is unchanged; automatic operation response-loss
reconciliation and a definitive unknown-result/retry decision remain SKI-23. Reconnecting
a dead transport can start a new server and cannot itself prove what the previous server
applied. No automatic replay is introduced. The retained-host listing is not restart
restoration, persistence/pruning (SKI-24), or coordinated native shutdown (SKI-25).

SKI-23 owns automatic dropped-response reconciliation; SKI-24 owns complete restart
result restoration, retention/pruning and restored retry; SKI-25 owns complete quit,
native/tray and restart coordination. List remains session-owned. No automatic replay,
full restart restoration, run-after-exit behavior, or quit coordination is claimed here.

Real contract/CLI tests use temporary stores, libraries/projects and local Git fixtures.
Browser fixtures return controlled snapshots and are not a second scheduler or proof of
filesystem or SSH safety. Supported native/platform behavior still requires platform CI.
