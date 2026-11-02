# Interactive operations (SKI-20 through SKI-25)

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
resubmission for it. Response-loss verification is described below. Conflicting destinations serialize, and an
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

## Lost responses and outcome verification

A lost operation response or an active originating host's transport loss enters
**Connection lost — checking result** in the original host/project/source. The
observer attempts three quiet reconnect/read rounds (0, 500, 1500 ms delays), with
a 12-second overall deadline. Checking counts as unfinished work (`settled: false`)
until verified or explicitly uncertain. Other hosts and Local remain usable.

`remote_host_connect({hostId, select: false})` reconnects without selecting that host.
Omitting `select` retains the existing host-switch behavior. Both paths share one
same-host connection attempt and the existing version/protocol checks. Switching
views cancels superseded view selections, not off-screen recovery. A reconnect hold
lasts through reconciliation; verified settlement retains its destination-refresh hold.
Lost mutations retain operation/expected-attempt correlation even before admission is
known. Only a corresponding authoritative result clears that uncertainty; an empty
list, unrelated receipt or stale attempt does not.

`reconcile` never submits, confirms, retries, cancels or applies. It first reads the
correlated live operation/receipt. Recorded successful siblings remain successful.
For an unowned unfinished receipt or a live unknown item, it can additionally verify
that the original destination matches captured intent under the incumbent nonblocking
repository/destination protection. Project verification checks project, deployment,
source identity/content, target identity and actual content. Installation checks the
original library, non-symlink target, source identity/content for local inputs, managed
record ownership/source metadata and actual content against the captured preparation.
The comparison reuses existing content/revision evidence; no new hash scheme is used.

A destination verified from incomplete result evidence reports `unchanged` with an
explanation that intended content is present and no write was repeated. This does not
claim which process performed the write or exactly-once execution. Busy, missing,
changed, redirected or insufficient evidence remains unknown. Verification of a live
unknown can save its repaired result once result storage works again. Read-only
verification of another process's unfinished receipt does not adopt its workers,
overwrite its receipt or grant Git confirmation authority. Unowned previews require
fresh preparation/review. The original process may still be running after a replacement
session lists no work.

The frontend's optional `recovery` (`checking` / `unknown`) and `recovery_attempt`
fields are app-session observation overlays, not host receipts or mutation authority.
Pending admission preserves the original submitted identity and context, with an
unknown item set rather than invented item counts. Equal-revision verified evidence
can repair a lost response; late lower revisions, wrong hosts and mismatched pending
attempts cannot replace it. Polling preserves original cache entries when a replacement
server lists nothing, and later authoritative settlement can still repair uncertainty.

An unverifiable bounded attempt settles as **Outcome unknown**, names that work may
still be running, and offers **Reconnect and check result**. This action performs only
reconnect/read verification. There is no ordinary Retry/confirmation/cancel control for
uncertain work, no automatic mutation replay, and uncertain sources/project updates
cannot silently be resubmitted with a fresh identity. Successful verification refreshes
only the original project/library through the existing scoped query path. Refresh errors
remain separate from successful writes.

## Local result retention and restoration

The desktop always calls `operation_results_request` with `invokeLocal`, including
summaries originating on SSH hosts. The public Rust/store/CLI seam is
`core::operations::results::dispatch`; the thin desktop adapter uses that same body.
Requests are `{ action: "read" }` and
`{ action: "write", version, results: OperationSnapshot[] }`. Responses contain
`{ version, results }`. Host-local SQLite `resource_state` kind `operation_results`,
ID `desktop`, stores this document outside portable Sync metadata. It is separate
from host execution receipts and contains no worker/staging evidence.

The writer retains failures, review requests and unknown outcomes, with successful
and canceled sibling details explaining partial batches. Standalone successful
summaries stay in the session cache until dismissed or replaced by the incumbent
contextual view; they are omitted from disk. Dismissal of a settled summary without
attention is session-local and suppresses later stale lists for that attempt.
Resolved results disappear from the saved document. Empty versioned documents are
kept so a stale writer cannot revive a resolved/dismissed result. SQLite compares
and replaces the document version atomically across connections. A version conflict
is reported instead of overwriting newer state; reload is required to resolve a
conflicting app session. Frontend saves are serialized and drain changes arriving
while a write is in flight.

Saved display `origin` contains original `host_name`, `ssh_target`, host creation
time and project name. Snapshot host/project IDs, original project/library paths,
per-item agent/source identities and outcomes remain intact. Names alone never
select a host or project. Offline/removed origins remain readable from their host
shortcut. A replacement saved host with the same ID must also match its original
SSH target and creation time. Project results and shortcut navigation compare the
original path as well as ID, including after explicit reconnection. A missing or
reused context stays inspectable without navigating to its replacement.

Restoration first attempts to load local results, before operation polling can replace
the cache. If the initial local read fails, current operation observation stays usable;
writes to the result document remain blocked until a successful read and merge. It does not connect hosts, submit work, restore a queue, cancel, retry or
confirm anything. `restored` is an in-memory display marker. Unfinished snapshots,
pending admissions and session `checking` overlays restore as settled uncertainty,
with their original operation and expected attempt correlation. They never imply
that original work stopped. A pending submission with no response retains an empty,
unknown item set instead of invented counts. Manual **Reconnect and check result**
performs the same bounded read-only reconciliation on Local or the exact original
SSH host. Current correlated evidence can repair cached uncertainty; older revisions,
other hosts/attempts, missing receipts and empty lists cannot prove completion.

URL-embedded credentials, query and fragment data are removed using incumbent source
sanitization. Persisted summaries omit raw exception messages and all previews. They
retain no transport secrets, temporary clone contents or confirmation authority.
In-memory results survive storage errors, and the contextual host control offers
**Retry saving results** with a generic error, without exposing database/remote stderr.

Restored project `retry_failed` explicitly creates a fresh attempt from durable
project evidence and revalidates the original source, target, deployment and project
identity through the existing guarded apply body. Successful/review/unknown siblings
are not queued. An unowned installation receipt cannot use that shortcut: it requires
`prepare_review { operation_id, attempt_id, item_ids? }`. Optional item IDs select
only original failed/review items; the restored **Retry failed** control supplies
failed IDs only. Repeated preparation identities reattach without replay.

`prepare_review` uses the existing bounded coordinator, rebuilds staging and current
source/destination evidence, then settles at `needs_review` with a fresh preview.
No write occurs during preparation. Only a fresh explicit `confirm_install` can queue
selected prepared items; successful and unselected siblings keep their outcomes.
All installation variants use this path, including marketplace, Git, folder/archive,
expanded batch and discovered sources. A previously selected Git failure retains its
original relative entry and chosen name; fresh preparation cannot add unselected
repository entries. Changed/missing sources, stale approvals, expired/missing staging
and newly occupied destinations cannot authorize a write from the cached summary.
Unchanged confirmation names retain the freshly reviewed target; user renaming captures
a new explicit target. Existing apply guards revalidate again before replacement.

`useOperations()` exposes `persistenceStatus: "loading" | "saving" | "saved" | "error"`
and `flush(): Promise<void>`. Always await `flush()` for a durability decision; a
rendered `saved` status alone is not an exit permission. Flush loads/repairs a failed
initial read when retried, includes in-flight saves and newer cache changes, and rejects
on storage failure/conflict while retaining memory. Operation admission intent is
captured before the host request. This seam neither stops work nor closes admission.
**SKI-25** must coordinate native/tray/app quit and restart, drain/seal admission as
appropriate, then await successful flush before exit. No shutdown coordination,
automatic quit/resume, detached worker or run-after-exit support is implemented here.

Real contract/CLI tests use temporary stores, libraries/projects and local Git fixtures.
Browser fixtures return controlled snapshots and are not a second scheduler or proof of
filesystem or SSH safety. Supported native/platform behavior still requires platform CI.

## Shared shutdown boundary (SKI-25)

Real quit and restart enter `shutdown_request`, a desktop-local decision shared by
native `RunEvent::ExitRequested`, tray/menu Quit, `app_exit`, `restart_app`, and
window-close paths configured to quit (including the disabled-tray fallback).
Closing to tray/minimizing keeps operations and sessions running. There is no
process plugin installed or permitted; no alternate process-plugin exit/relaunch
route is introduced.

`shutdown-requested` carries `{id,target,mode}`; `read` repairs missed notification
or observer remount. Targets are `quit`, `restart`, and `install_update` (the Windows
installer exits the process). `inspect` conservatively checks actual local work and
retained sessions before presenting the three choices. The existing staged-work
confirmation runs for native/app/restart requests and again at final settlement.

- Keep app open invalidates the request ID and resumes admission. Late RPC/save
  replies cannot authorize that canceled request. It does not undo cancellations
  already requested by an earlier Cancel-and-quit choice.
- `choose {id,mode:"cancel"}` refuses new mutation admission. The next preparation
  cancels owned queued/waiting work and requests safe stopping of workers, including
  owned work whose admission reply is still outstanding. Completed changes stay.
- `choose {id,mode:"drain"}` accepts subsequent work until the final idle seal.
  Failed, needs-review, canceled and unknown settled outcomes do not veto quitting.

The observer captures pending intent before storage/RPC, counts pending actions and
bounded recovery, then calls `prepare {id}`. The desktop IPC boundary counts requests
before spawning/awaiting them. Preparation checks the real `OperationService` worker,
request and coordinator state under its existing lock, and asks each retained original
host process to seal its own service. It never infers safety from an idle frontend
snapshot, a gap between queue items, or an empty replacement-process list. A partial
seal is resumed when another host/call is busy. The service remains the only scheduler.

Host operation requests `shutdown_status`, `shutdown_cancel`, `shutdown_seal` and
`shutdown_resume` return `{ready,snapshots}`. Remote seals/resumes carry monotonically
ordered `epoch` values, so a deadline-expired RPC cannot override a newer Keep open or
seal. Checking a host has a 12-second overall deadline even if keepalives continue.
Unreachable original work remains explicitly unknown, preserving completed siblings;
unknown does not assert stopped, canceled, or permission to replay. Cancellation only
addresses the receiving service's owned records, never an unowned receipt.

A ready preparation seals frontend admission and backend calls/connections, merges
fresh original-host snapshots, and **awaits `OperationsProvider.flush()`** before
`finish {id}`. Rendered `saved` status is never exit authority. A focused saving dialog
protects the final unsaved-work check and flush; Keep open still revokes the request.
Storage failure/conflict retains memory, resumes the boundary, and exposes retry or
Keep app open. Sessions, windows and exit-time backup remain intact until final
worker settlement and persistence have succeeded.

Only `finish` authorizes teardown and the native exit/restart call. Restart retains
`request_restart` so the singleton's Exit cleanup runs before relaunch. Windows
installation uses a guarded backend updater command because the upstream installer
calls `process::exit` directly; it preserves local proxy, updater verification,
cleanup and exit-time backup. Keep open during the download invalidates the final
installation check; no late download completion authorizes that canceled request.
Other incumbent update/platform restrictions remain.
The process-group safety rule is unchanged: no parent/developer process group is
signaled. No detached service, run-after-exit support or automatic replay is added.

Native adapter checks use Tauri's mock runtime, not a shipped GUI. Actual host tests
use temporary libraries and real CLI stdio processes with externally held repository
locks. Browser scenarios prove controlled IPC/UI behavior only. These tests do not
establish Windows/Linux native installer, shipped desktop, or live SSH behavior.
