# Config sync prior art: machine-specific settings and conflicting edits

Date: 2026-10-10
Ticket: SID-97

## Question

How do established multi-machine config sync tools handle four things?

- Separating shared settings from machine-specific ones.
- Merging concurrent edits.
- A machine that joins with existing state.
- Showing what changed.

This feeds two Agents Manager decisions:

1. The merge policy for presets, memberships and the active preset across hosts, including clock skew.
2. Which app settings are shared and which are per host, and how settings that hold paths map across machines.

## Sources and versions

Only primary sources are used: official docs and source repositories. Code links are pinned to the commits read on 2026-10-10.

- **VS Code:** docs at code.visualstudio.com (settings-sync page `DateApproved: 10/7/2026`) and `microsoft/vscode` at [`95903124`][vsc-src].
- **chezmoi:** docs at chezmoi.io (latest release v2.73.0) and `twpayne/chezmoi` at [`0f1c8bd6`][cz-src].
- **Syncthing:** docs at docs.syncthing.net ("Written for v2.1.0") and `syncthing/syncthing` at [`a0fd6376`][st-src].

### Why Syncthing is the third tool

Syncthing fits the two decisions better than the alternatives.

- **Concurrency and clocks (decision 1).** Syncthing first detects whether edits were concurrent, using version vectors. Only then does it pick a winner, by modification time. It also documents what happens to the losing copy. This matches the "newest committer time wins" question.
- **Paths (decision 2).** A folder shares one ID across devices, but each device stores it at its own local path, and that path is "not sent to other devices". This is the clearest prior art for path-bearing settings.

Alternatives considered:

- **JetBrains Settings Sync** follows the same cloud-settings model as VS Code, so it would mostly repeat that section.
- **Nix home-manager** defines each host declaratively and leaves merging to git. It has less to show about concurrent edits than chezmoi, which already covers the git-backed case.

## Summary

- **Shared vs machine-specific.**
  - VS Code marks settings with a scope (`machine`, `machine-overridable`) and keeps a synced ignore list.
  - chezmoi keeps per-machine values in a config file outside the repo, and uses templates to render shared files per host.
  - Syncthing shares a folder identity but keeps the path, the ignore file and the policies on each device.
  - None of the three rewrites paths. They avoid the problem by keeping paths local or relative to a per-machine root.
- **Concurrent edits.**
  - VS Code runs a three-way merge against the last-synced base, with no clocks. Settings merge per key; MCP and tasks merge as whole files and stop for review when both sides changed.
  - chezmoi leaves this to git.
  - Syncthing detects concurrency causally, then lets the newer mtime win. It always keeps the loser as a visible `.sync-conflict` file, so clock skew changes which copy is primary but never loses data.
- **Joining with existing state.**
  - VS Code merges automatically and raises conflicts, backing up local data before any overwrite.
  - chezmoi's default `apply` overwrites pre-existing files without prompting.
  - Syncthing treats pre-existing differing files as ordinary conflicts.
- **Change visibility.**
  - VS Code shows remote versions labelled with the machine that wrote them, with compare and restore.
  - chezmoi's model is review-before-apply (`diff`, `status`, `--dry-run`).
  - Syncthing shows recent changes, locally changed items and conflict files.

---

## VS Code Settings Sync

### Shared vs machine-specific

**What syncs.** The docs list Settings, Keyboard Shortcuts, Snippets, Tasks, MCP Servers, UI State, Extensions, Profiles, and Prompts and Instructions. Profiles are capped at 20 ([docs][vsc-docs]).

- MCP server sync arrived in [1.102][vsc-1102]. Each profile keeps its own `mcp.json` ([`mcpSync.ts`][vsc-mcpsync]).
- User prompt files arrived in [1.99][vsc-199] and became "Prompts and Instructions" in [1.100][vsc-1100].
- Only `*.prompt.md`, `*.instructions.md`, `*.chatmode.md` and `*.agent.md` files in the profile's prompts folder sync ([`promptsSync.ts`][vsc-promptssync]).
- Folders such as `~/.claude/rules` "do not roam through Settings Sync" ([custom instructions docs][vsc-instructions]).

**Scope tags.** Each setting is declared with a scope, and two scopes stay off the wire by default.

- The contribution-point docs say the `machine` scope is for settings like "an installation path which shouldn't be shared across machines. The value of these settings will not be synchronized." `machine-overridable` is excluded too ([contribution points][vsc-contrib]).
- `application` scope *does* sync.
- In code, `getIgnoredSettings` drops any setting with scope `MACHINE`, scope `MACHINE_OVERRIDABLE`, or `ignoreSync` ([`userDataSync.ts` L54-69][vsc-uds-ignored]).

**User ignore list.**

- `settingsSync.ignoredSettings` adds to those defaults. A `-` prefix forces a default-ignored setting to sync ([`settingsMerge.ts` `getIgnoredSettings`][vsc-settingsmerge-ignored]).
- On write, `updateIgnoredSettings` swaps each ignored key for the *other side's* value. A machine's local value never reaches the cloud, and it never erases the cloud's value for that key ([`settingsMerge.ts` L70][vsc-settingsmerge-update]).
- The ignore lists themselves always sync. `settingsSync.ignoredSettings` and `settingsSync.ignoredExtensions` are `scope: APPLICATION` with `disallowSyncIgnore: true` ([`userDataSync.ts` L89-120][vsc-uds-registry]).

**Per-platform sections in one shared blob.**

- `settingsSync.keybindingsPerPlatform` (default on) stores separate `mac`, `linux` and `windows` keybinding sections in the remote record ([docs][vsc-docs], [`keybindingsSync.ts` L42][vsc-keybindingssync]).

**Per-machine extensions.**

- Extensions installed with "Install (Do not Sync)" are machine-scoped and ignored ([`ignoredExtensions.ts` L68][vsc-ignoredext]).
- Extensions do not sync to or from remote (SSH, container, WSL) windows ([docs][vsc-docs]).
- UI state syncs only storage keys marked `StorageTarget.USER`, not `MACHINE` ([`globalStateSync.ts` L296-302][vsc-globalstatesync]).

**Paths.** No path translation appears in the merge code. Path values sync verbatim unless they are machine-scoped or ignored. This is inferred from reading `settingsMerge.ts`.

### Concurrent edits

**Three-way merge against a stored base.** Each resource is merged three ways: local, remote, and the last-synced remote copy (`lastSyncUserData`) kept on the machine ([`settingsMerge.ts` `merge` L104][vsc-settingsmerge-merge]). If there is no base and this machine wrote the remote copy, the remote copy serves as base ([`settingsSync.ts` L91][vsc-settingssync-91]).

**Granularity by resource:**

| Resource | Unit | Both sides changed |
| --- | --- | --- |
| Settings | JSON key | Conflict if the values differ. Delete-vs-update is also a conflict. |
| Keybindings | command | Conflict ([`keybindingsMerge.ts`][vsc-keybindingsmerge]) |
| Snippets, prompts | file | Conflict ([`snippetsMerge.ts`][vsc-snippetsmerge]) |
| Tasks, MCP | whole file | Always a conflict: `hasConflicts: true` ([`abstractJsonSynchronizer.ts` L234-274][vsc-absjson]) |
| Extensions | extension | Never a conflict. Installed is the union; the higher version wins ([`extensionsMerge.ts`][vsc-extmerge], [`extensionsSync.ts` L220][vsc-extsync]) |
| UI state | storage key | Never a conflict. Local wins; on first sync, remote wins ([`globalStateMerge.ts`][vsc-globalstatemerge]) |

**No clocks.** No merge file uses timestamps or mtimes. The only `new Date()` names local backup files ([`abstractSynchronizer.ts` `backupLocal`][vsc-abs-backup]).

**Optimistic concurrency.** Ordering comes from server refs, not from time.

- Remote writes send `If-Match: <ref>` ([`userDataSyncStoreService.ts` L385-399][vsc-store]).
- A `412`/`409` makes the synchronizer refetch and merge again ([`abstractSynchronizer.ts` L339-351][vsc-abs-retry]).
- Local file writes use the previously read content as a precondition, so they retry if the file changed underneath.

**Conflict UX.**

- A resource in conflict stops syncing until it is resolved ([`abstractSynchronizer.ts` L219][vsc-abs-skip]).
- The user can choose Accept Local, Replace Remote, Accept Remote, Replace Local, or Show Conflicts ([docs][vsc-docs]).
- In current code, Show Conflicts opens the three-way merge editor (Remote / Local / base, with the result as a preview) ([`userDataSyncConflictsView.ts` L182-199][vsc-conflictsview]). The docs still say "diff editor".

### Joining with existing state

- **Automatic merge.** The docs say: "Settings Sync automatically merges your local and cloud data… If it cannot merge the data, you are prompted to resolve the conflicts" ([docs][vsc-docs]).
- **Turn-on flow in code.** Turning sync on calls `manualSyncTask.merge()`. On conflict it shows "Conflicts Detected" with Show Conflicts, Accept Remote and Accept Local ([`userDataSyncWorkbenchService.ts` L393-446][vsc-turnon]).
- **Old dialog removed.** The older Merge / Replace Local / Merge Manually dialog is gone ([old docs][vsc-old-docs]).
- **First sync (no base):**
  - Settings keys present on one side only are added to the other.
  - The same key with different values is a conflict.
  - Extensions are unioned.
  - UI state takes remote.
- **Backup first.** Before overwriting local data, every synchronizer calls `backupLocal` ([`settingsSync.ts` L236][vsc-settingssync-236]).

### Change visibility

- **Remote history.** "Settings Sync: Show Synced Data" lists remote versions per resource, each labelled with the machine that wrote it ([`userDataSyncViews.ts`][vsc-views]).
  - Each version offers "Compare with Local", raw JSON, and Restore.
  - Restore forces both a local and a remote write, so it propagates to other machines.
- **Machine management.** A Synced Machines view lets you rename a machine or turn sync off for it remotely.
- **Retention.** "Local backups are deleted after 30 days. For remote backups, the latest 20 versions of each data category are retained" ([docs][vsc-docs]).
  - The local window comes from an unregistered `sync.localBackupDuration`, and at least 10 files are kept ([`userDataSyncLocalStoreService.ts` L113-130][vsc-localstore]).
  - The 20 remote versions are a server-side setting; the docs are the only source for that number.
- **No preview for clean merges.** Clean merges apply without a preview. The merge preview is only shown when there are conflicts.

---

## chezmoi

### Shared vs machine-specific

**Two stores.**

- The shared source state lives in a git repo (`~/.local/share/chezmoi`).
- A per-machine config file, `~/.config/chezmoi/chezmoi.$FORMAT`, is *not* in the repo. It holds "variables that might vary from machine to machine" under `[data]` ([machine differences][cz-machine]).
- Template data is layered, with "later data overwrite earlier ones":
  1. built-ins (`.chezmoi.hostname`, `.chezmoi.os`, `.chezmoi.homeDir`, …)
  2. shared `.chezmoidata.*`
  3. the machine's `[data]`
- Sources: [templating][cz-templating], [variables][cz-vars].

**Ask once, at init.**

- `chezmoi init` renders `.chezmoi.$FORMAT.tmpl` from the repo to generate the local config.
- `promptStringOnce` and `promptBoolOnce` "return the value… if it exists… otherwise prompt", so each machine answers once ([init template][cz-inittmpl], [promptStringOnce][cz-prompt]).
- chezmoi warns when the config template has changed since the local config was generated ([setup][cz-setup]).

**Rendering per host.**

- Shared files are templates evaluated against host data.
- `.chezmoiignore` is always a template, which "allows different files to be ignored on different machines" ([chezmoiignore][cz-ignore]).

**Paths.**

- Targets are relative to `destDir` (default `$HOME`), so a different home directory needs no mapping.
- For OS-specific locations, the docs recommend one shared body in `.chezmoitemplates/`, one small template per location, and a `.chezmoiignore` condition on `.chezmoi.os` that drops the wrong one ([machine differences][cz-machine]).

**Seed-once prefixes.**

- `create_`: "If the file in the destination state already exists then its contents will be left unchanged."
- `modify_`: transforms the current file instead of replacing it.
- `run_once_` and `run_onchange_`: gate scripts.
- Source: [target types][cz-targets].

**Per-machine state.**

- Kept in `chezmoistate.boltdb` next to the config.
- It stores the SHA256 of what chezmoi last wrote (not the contents), "to avoid storing secrets" ([architecture][cz-arch], [`entrystate.go`][cz-entrystate]).
- This is noted only as a description of the mechanism, not as something to adopt.

### Concurrent edits

**Across machines it is git.**

- "chezmoi relies on your version control system and hosted repo to share changes across multiple machines" ([setup][cz-setup]).
- `chezmoi update` runs `git pull --autostash --rebase [--recurse-submodules]` and then applies ([update][cz-update]).
- The daily-operations guide suggests `git pull` then `chezmoi diff` before `apply`, to review first ([daily ops][cz-daily]).
- chezmoi has no merge logic of its own across machines. Rebase conflicts are ordinary git conflicts.

**On one machine, drift vs source.**

- `apply` prompts `"<path> has changed since chezmoi last wrote it"` with diff, overwrite, all-overwrite, skip or quit. It does so only when the file differs from chezmoi's last-written record ([`config.go` L1115-1196][cz-preapply]).
- `chezmoi merge` runs a "three-way merge between the destination state, the target state, and the source state" in an external tool ([merge][cz-merge]).
  - These three are *not* ancestor / ours / theirs. No common ancestor exists, because only hashes are stored.
- `re-add` copies drifted files back into the source ([re-add][cz-readd]).

### Joining with existing state

- **Init sequence.** `chezmoi init <repo>` clones the repo and generates the config. `--apply` then applies ([init][cz-init]).
- **Silent overwrite by default.**
  - On first apply there is no last-written record, so `lastWrittenEntryState == nil` selects `promptNone`.
  - Pre-existing files are overwritten without a prompt ([`config.go` L1143-1160][cz-preapply]).
  - The docs do not spell this out; it comes from reading the code.
- **Opting into prompts.**
  - `--less-interactive` prompts `"<path> already exists"` for pre-existing targets ("only overwrite what chezmoi has written").
  - `--interactive` prompts for every target ([global flags][cz-flags]).
- **Recommended safe flow.** `init`, then `diff`, then `apply` ([setup][cz-setup]).
  - `chezmoi add` imports a local version you want to keep.
  - `create_` seeds a file only when it is absent.
  - `-n -v` shows a dry run without touching anything.

### Change visibility

- **`chezmoi diff`** shows what `apply` would change ([diff][cz-diff]).
- **`chezmoi status`** has two columns ([status][cz-status]):
  1. "the difference between the last state written by chezmoi and the actual state"
  2. "the difference between the actual state and the target state, and what effect running chezmoi apply will have"
- **`chezmoi verify`** exits non-zero on any drift.
- **Shared history** is plain `git log` via `chezmoi git` ([git][cz-git]).

---

## Syncthing

### Shared vs machine-specific

**Shared identity, local path.**

- A folder is identified by an ID that every device shares.
- Its `path` is "the path to the directory where the folder is stored on this device; not sent to other devices". The `label` "may be different on each device" ([config][st-config]).
- Peers learn only `id`, `label` and `folder_type` ([BEP folder message][st-bep-folder]).

**Local policy.**

- "File versioning is configured per folder, on a per-device basis" ([versioning][st-versioning]).
- Folder type, `maxConflicts`, `ignoreDelete` and rescan settings live in each device's own `config.xml`. The docs never say "not synced" for each one, so this is inferred.

**Local ignore list with an opt-in shared part.**

- "The .stignore file itself will never be synced to other devices, although it can #include files that are synchronized between devices" ([ignoring][st-ignoring]).
- Each device keeps a local list. A shared, synced file can carry the common part.

**Authority per device via folder types** ([folder types][st-types]):

- **Send Only** ignores incoming changes.
- **Receive Only** keeps local edits but does not send them. In code they are flagged *invalid*, so they always lose to the cluster ([`bep_fileinfo.go` L210-229][st-wins]).

### Concurrent edits

**Detection is causal.**

- Each file carries a version vector. A conflict exists only when the vectors are concurrent *and* the content differs ([`InConflictWith` L190-207][st-inconflict]; [syncing][st-syncing]: "modified on two devices simultaneously and the content actually differs").
- The content check compares block hashes.

**The BEP spec text is outdated.**

- The spec calls the counter "a simple incrementing counter, starting at zero" ([BEP FileInfo][st-bep-fileinfo]).
- The code sets it to `max(current+1, now)`, with `now` the Unix time in seconds ([`vector.go` L127-139][st-vector]).
- The commit says this makes "version vector values clock based… In case the clocks are wrong… we fall back to just simple increment." The goal is that a reset or re-added device produces a *conflict* instead of looking like an old ancestor that gets silently overwritten ([PR #6605][st-pr6605]).
- Each device only increments its own counter, so skew cannot fake causality.

**Resolution uses wall-clock mtime.**

- Docs: "The file with the older modification time will be marked as the conflicting file and thus be renamed. If the modification times are equal, the file originating from the device which has the larger value of the first 63 bits for its device ID will be marked as the conflicting file" ([syncing][st-syncing]).
- Code: invalid loses first, then newer mtime wins, then the device-ID tie-break ([`WinsConflict`][st-wins]).
- A delete's mtime is the deletion time.
- **Clock skew therefore decides the winner, but only among truly concurrent edits.** The docs say nothing about clock sync.

**The loser is kept.**

- The loser is renamed `<name>.sync-conflict-<date>-<time>-<modifiedBy>.<ext>`, and it syncs like a normal file ([`folder_sendrecv.go` L2239][st-conflictname]).
- Delete-vs-modify has no special rule. If the delete wins, the file is moved to a conflict copy instead of being deleted ([L925][st-deleteconflict]). Data survives either way.
- `maxConflicts` (default 10; 0 disables copies) caps how many copies are kept ([config][st-config]).
- There is no content merge, because "we don't know which of the conflicting files is the 'best'" ([syncing][st-syncing]).

### Joining with existing state

- **No special join mode.** A joining device scans and stamps its own counters, which are concurrent with the cluster's.
- **Identical files are adopted** without transfer or conflict ([`folder_sendrecv.go` L392][st-shortcut]). The FAQ confirms "existing files will not be transferred" ([FAQ][st-faq-seed]).
- **Differing files** go through the normal rule: newer mtime wins and the loser becomes a conflict copy. This is inferred from code; the docs don't describe this case.
- **Explicit authority after the fact:**
  - **Revert Local Changes** (Receive Only): "added files will be deleted, modified or deleted files will be re-synced from the cluster".
  - **Override Changes** (Send Only): enforces "this host's current state on the rest of the cluster" ([folder types][st-types]).
  - Override does not depend on clocks. It merges the vectors and bumps the local one, so the local version is strictly newer ([`folder_sendonly.go` L131][st-override]).
- **Auto-accept.** `autoAcceptFolders` shares an existing local folder with the same ID instead of creating a second one ([config][st-config]).

### Change visibility

- **Recent changes.** The GUI shows "Recent Changes" since the last restart. `--audit` writes a persistent log ([FAQ][st-faq-history]).
- **Per-folder lists.** The GUI also shows Out of Sync Items, Failed Items and "Locally Changed Items" (receive-only), backed by `GET /rest/db/localchanged`.
- **Events.** `LocalChangeDetected` and `RemoteChangeDetected` include `modifiedBy` ([events][st-events]). There is no dedicated conflict event.
- **Conflict copies** are themselves the review surface: they are visible files next to the original.
- **Versioning** (Trash Can, Simple, Staggered, External) only archives changes *received from other devices*, not local edits ([versioning][st-versioning]).

---

## Comparison

| | Shared vs machine-specific | Concurrent-edit merge | Joining with existing state | Change visibility / review |
| --- | --- | --- | --- | --- |
| **VS Code Settings Sync** | Per-setting scope (`machine`, `machine-overridable` never sync). The user ignore list itself syncs, and ignored keys keep the other side's value. Per-platform sections in one blob for keybindings. Paths copied verbatim. | Three-way vs the last-synced base. Per key for settings. Whole file for tasks and MCP, where both-changed always conflicts. Union for extensions. Local-wins for UI state. No clocks; server ETag preconditions. | Automatic merge with a union of one-sided keys. A conflict blocks only that resource. Local is backed up before any overwrite. | Remote versions per resource labelled by machine, with compare and restore. Local backups 30 days; remote keeps 20 versions. Merge editor only for conflicts; no preview for clean merges. |
| **chezmoi** | Shared git repo plus a per-machine config file outside it. Prompt-once at init. Templates on hostname and OS. Templated ignore file. Paths relative to `destDir`. | Delegated to git (`pull --rebase`). Locally, it prompts if a file drifted from what chezmoi last wrote. `merge` is a 3-way between destination, target and source, with no ancestor. | `init --apply` silently overwrites pre-existing files by default. Opt-in prompts (`--less-interactive`). Recommended flow is `diff` before `apply`. `create_` seeds a file only if absent. | Review before apply: `diff`, two-column `status`, `--dry-run -v`, `verify`. History via `git log`. |
| **Syncthing** | Shared folder ID; path and label per device and never sent. Local `.stignore`, with a shared part via `#include`. Versioning and policies per device. Authority per device via folder type. | Version vectors detect concurrency (counter floored at wall time). Winner by newer mtime, then a device-ID tie-break. The loser is always kept as a `.sync-conflict` file. No content merge. | No special mode. Identical files are adopted; differing ones become ordinary conflicts. Explicit Revert / Override buttons, and Override does not use clocks. | Recent Changes, a Locally Changed Items list, conflict copies as files, an events API, an audit log, and versioning of remote-originated changes. |

---

## Patterns relevant to Agents Manager

These are patterns and trade-offs, not decisions.

### Decision 1: merge policy for presets, memberships and active preset

**1. Use time only after a base-relative check.**

- The current whole-file merge in `src-tauri/src/core/merge/decision.rs` (`merge_whole_files`, `newest_wins`) already does this. It goes three-way against the git merge base, and calls committer time only when both sides diverged, with commit id as a deterministic tie-break.
- That is the same structure as Syncthing (causal check, then mtime, then device-ID tie-break).
- VS Code goes further and never uses time. Both-changed whole files (tasks, MCP) become a conflict.
- Both tools build their causality check on data each machine already holds:
  - VS Code keeps the base content.
  - Syncthing keeps the vectors.
  - Agents Manager already has the equivalent in the git merge base, so it would need no new hashing.

**2. Clock skew decides only the winner, not whether data survives.**

- Syncthing accepts that a device whose clock runs ahead wins concurrent edits and deletes. It limits the damage by always keeping the loser as a visible conflict copy.
- The current newest-wins path drops the loser. That is the main gap against the prior art.
- Git-specific note: committer time is the committing host's clock, and rebase records the current time as committer date unless `--committer-date-is-author-date` is used ([git-rebase][git-rebase]). Any rewrite of a commit therefore moves its "newest" stamp.

**3. Granularity sets the conflict rate.**

- VS Code merges settings per key and keybindings per command. Only true same-key clashes conflict.
- A preset file that holds per-agent toggles behaves like VS Code's whole-file tasks and MCP: any two edits collide.
- Merging per preset entry or per agent toggle, as Agents Manager already does for skills per component, cuts false conflicts.

**4. Leave values for agents a host doesn't have.**

- VS Code's `updateIgnoredSettings` writes the *other side's* value for keys this machine does not own, so a machine never erases what it ignores.
- `keybindingsPerPlatform` keeps separate per-platform sections in one shared record.
- Syncthing's Receive Only marks non-authoritative local changes invalid, so they always lose.
- Each pattern stops a host from overwriting toggles for agents it does not have installed, which is the churn described in the ticket.

**5. Active preset is a shared scalar.**

- VS Code treats scalar UI-state keys as never-conflict: local wins, and remote wins on first sync.
- The alternative is a VS Code settings-style per-key three-way, which raises a conflict when both sides changed.
- The trade-off: never-conflict means silent last-sync-wins, while three-way means a prompt on rare simultaneous switches.

**6. Joining machine and per-machine "Default".**

Prior art avoids losing local data on join in three ways:
- **VS Code:** union of one-sided keys, conflicts for same-key differences, and a local backup before any overwrite.
- **Syncthing:** identical content is adopted silently; differing content keeps both copies.
- **chezmoi:** `create_` seeds a file only when absent, and `promptOnce` asks once per machine.

The negative example is chezmoi's default: it silently overwrites pre-existing files on first `apply`, which is what the ticket observes with dropped local presets. Its own fix is an opt-in "only overwrite what chezmoi has written" mode.

**7. Explicit override as an escape hatch.**

- Syncthing's Override/Revert and VS Code's Replace Local/Replace Remote/Restore let a user declare one side authoritative without relying on clocks.
- Both propagate the result to every other machine.

### Decision 2: shared vs per-host settings and path mapping

**1. Three mechanisms for "per-host".**

| Mechanism | How it works | Upside | Downside |
| --- | --- | --- | --- |
| Per-setting scope (VS Code `machine`) | Each setting is tagged as never synced | Defaults are clear | The list is fixed |
| Separate per-machine file outside the shared repo (chezmoi config, Syncthing `config.xml`) | Per-host values live in their own file | Simple | Every host must set its own values |
| Shared file with per-host or per-platform sections (VS Code keybindings) | One record holds a section per host or platform | One place to edit | The record grows with each host |

**2. The ignore list itself.**

- VS Code forces `ignoredSettings` and `ignoredExtensions` to *sync*, so the classification is consistent everywhere.
- Syncthing keeps `.stignore` local but offers `#include` of a synced file for the shared part.

**3. Path-bearing settings.**

None of the three rewrites paths. Each uses one of these instead:
- **Keep the path local** (Syncthing folder `path`, "not sent to other devices"; VS Code `machine` scope, whose canonical example is "an installation path").
- **Make it relative to a per-machine root** (chezmoi `destDir`/`homeDir`).
- **Render it per OS or host from a template** (chezmoi `.chezmoi.os`, `.chezmoitemplates`).

The Syncthing split of shared identity and local path fits custom agents directly: share the agent's identity and keep its path per host.

**4. Closest analogues for the listed settings (no recommendation).**

| App setting | Closest prior-art analogue |
| --- | --- |
| Library path | Syncthing folder `path` (local, never sent) |
| Custom agents and their paths | Shared identity plus per-host path (Syncthing folder ID / path) |
| Disabled agents | VS Code machine-scoped extensions plus the synced `ignoredExtensions` list; Syncthing local `.stignore` with an optional shared `#include` |
| Deploy mode (symlink/copy) | A per-host filesystem capability. Analogues are chezmoi templating on `.chezmoi.os` and VS Code per-platform sections |
| Proxy | Network-environment values, like VS Code's `machine` example of install paths |
| Auto-update | App-install behaviour, like VS Code `application`-scope settings, which *do* sync by default |
| Merge engine, automatic sync | Sync-control settings. VS Code forces its sync-control lists to sync and manages per-machine on/off from a Synced Machines view. Syncthing keeps folder type and versioning per device. |

The `decision.rs` header says both devices merging the same pair of commits "compute the same plan". That property depends on hosts running the same merge engine, which argues for treating the engine like VS Code's always-synced settings.

**5. Seeding on first run.**

- chezmoi's `promptStringOnce` gives each host a value once, at join time. It then keeps that value in the per-machine file, never in the shared repo.
- This is the pattern for path-bearing settings that a joining host must supply.

## Open uncertainty

- **VS Code:**
  - The 20 remote versions come only from the docs; the server is not public.
  - The no-path-translation claim covers only the merge code.
  - Settings may conflict on key order or comment differences alone (from reading `areSame`, not tested).
- **chezmoi:** the silent overwrite on first `apply` comes from reading the code. The docs don't state it.
- **Syncthing:**
  - What happens to differing pre-existing files on join is inferred from code.
  - That folder type, versioning and `maxConflicts` stay local is inferred from where they live in `config.xml`. The docs never say "not synced" for each one.
- **Not tested:** no tool was run to observe behaviour. All claims come from docs and code reading.

[vsc-src]: https://github.com/microsoft/vscode/tree/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common
[vsc-docs]: https://code.visualstudio.com/docs/configure/settings-sync
[vsc-contrib]: https://code.visualstudio.com/api/references/contribution-points#contributes.configuration
[vsc-1102]: https://code.visualstudio.com/updates/v1_102
[vsc-199]: https://code.visualstudio.com/updates/v1_99
[vsc-1100]: https://code.visualstudio.com/updates/v1_100
[vsc-instructions]: https://code.visualstudio.com/docs/agent-customization/custom-instructions
[vsc-old-docs]: https://github.com/microsoft/vscode-docs/blob/e1fac75b06a9382f6b8b1b70d2ea7e34e1bc4dd2/docs/editor/settings-sync.md#L42-L50
[vsc-mcpsync]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/mcpSync.ts#L47
[vsc-promptssync]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/promptsSync/promptsSync.ts#L75
[vsc-uds-ignored]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/userDataSync.ts#L54-L69
[vsc-uds-registry]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/userDataSync.ts#L89-L120
[vsc-settingsmerge-ignored]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/settingsMerge.ts#L23
[vsc-settingsmerge-update]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/settingsMerge.ts#L70
[vsc-settingsmerge-merge]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/settingsMerge.ts#L104
[vsc-settingssync-91]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/settingsSync.ts#L91
[vsc-settingssync-236]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/settingsSync.ts#L236
[vsc-keybindingssync]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/keybindingsSync.ts#L42
[vsc-keybindingsmerge]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/keybindingsMerge.ts#L35
[vsc-snippetsmerge]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/snippetsMerge.ts#L22
[vsc-absjson]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/abstractJsonSynchronizer.ts#L234-L274
[vsc-extmerge]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/extensionsMerge.ts#L18
[vsc-extsync]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/extensionsSync.ts#L220
[vsc-ignoredext]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/ignoredExtensions.ts#L68-L69
[vsc-globalstatesync]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/globalStateSync.ts#L296-L302
[vsc-globalstatemerge]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/globalStateMerge.ts
[vsc-abs-backup]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/abstractSynchronizer.ts#L745
[vsc-abs-retry]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/abstractSynchronizer.ts#L339-L351
[vsc-abs-skip]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/abstractSynchronizer.ts#L219
[vsc-store]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/userDataSyncStoreService.ts#L385-L399
[vsc-localstore]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/platform/userDataSync/common/userDataSyncLocalStoreService.ts#L113-L130
[vsc-conflictsview]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/workbench/contrib/userDataSync/browser/userDataSyncConflictsView.ts#L182-L199
[vsc-views]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/workbench/contrib/userDataSync/browser/userDataSyncViews.ts
[vsc-turnon]: https://github.com/microsoft/vscode/blob/959031245ebb1fe077e0d512e1397c0ae82006e4/src/vs/workbench/services/userDataSync/browser/userDataSyncWorkbenchService.ts#L393-L446
[cz-src]: https://github.com/twpayne/chezmoi/tree/0f1c8bd658e032e1ef5da382376c97fe9681e290
[cz-machine]: https://www.chezmoi.io/user-guide/manage-machine-to-machine-differences/
[cz-templating]: https://www.chezmoi.io/user-guide/templating/
[cz-vars]: https://www.chezmoi.io/reference/templates/variables/
[cz-inittmpl]: https://www.chezmoi.io/reference/special-files/chezmoi-format-tmpl/
[cz-prompt]: https://www.chezmoi.io/reference/templates/init-functions/promptStringOnce/
[cz-setup]: https://www.chezmoi.io/user-guide/setup/
[cz-ignore]: https://www.chezmoi.io/reference/special-files/chezmoiignore/
[cz-targets]: https://www.chezmoi.io/reference/target-types/
[cz-arch]: https://www.chezmoi.io/developer-guide/architecture/
[cz-entrystate]: https://github.com/twpayne/chezmoi/blob/0f1c8bd658e032e1ef5da382376c97fe9681e290/internal/chezmoi/entrystate.go
[cz-update]: https://www.chezmoi.io/reference/commands/update/
[cz-daily]: https://www.chezmoi.io/user-guide/daily-operations/
[cz-preapply]: https://github.com/twpayne/chezmoi/blob/0f1c8bd658e032e1ef5da382376c97fe9681e290/internal/cmd/config.go#L1115-L1196
[cz-merge]: https://www.chezmoi.io/reference/commands/merge/
[cz-readd]: https://www.chezmoi.io/reference/commands/re-add/
[cz-init]: https://www.chezmoi.io/reference/commands/init/
[cz-flags]: https://www.chezmoi.io/reference/command-line-flags/global/
[cz-diff]: https://www.chezmoi.io/reference/commands/diff/
[cz-status]: https://www.chezmoi.io/reference/commands/status/
[cz-git]: https://www.chezmoi.io/reference/commands/git/
[st-src]: https://github.com/syncthing/syncthing/tree/a0fd63763f153dd0969199ab889fdfb53de33c00
[st-config]: https://docs.syncthing.net/users/config.html#folder-element
[st-bep-folder]: https://docs.syncthing.net/specs/bep-v1.html#fields-folder-message
[st-bep-fileinfo]: https://docs.syncthing.net/specs/bep-v1.html#fields-fileinfo-message
[st-versioning]: https://docs.syncthing.net/users/versioning.html
[st-ignoring]: https://docs.syncthing.net/users/ignoring.html
[st-types]: https://docs.syncthing.net/users/foldertypes.html
[st-syncing]: https://docs.syncthing.net/users/syncing.html#conflicting-changes
[st-vector]: https://github.com/syncthing/syncthing/blob/a0fd63763f153dd0969199ab889fdfb53de33c00/lib/protocol/vector.go#L127-L139
[st-pr6605]: https://github.com/syncthing/syncthing/pull/6605
[st-inconflict]: https://github.com/syncthing/syncthing/blob/a0fd63763f153dd0969199ab889fdfb53de33c00/lib/protocol/bep_fileinfo.go#L190-L207
[st-wins]: https://github.com/syncthing/syncthing/blob/a0fd63763f153dd0969199ab889fdfb53de33c00/lib/protocol/bep_fileinfo.go#L210-L229
[st-conflictname]: https://github.com/syncthing/syncthing/blob/a0fd63763f153dd0969199ab889fdfb53de33c00/lib/model/folder_sendrecv.go#L2239
[st-deleteconflict]: https://github.com/syncthing/syncthing/blob/a0fd63763f153dd0969199ab889fdfb53de33c00/lib/model/folder_sendrecv.go#L925
[st-shortcut]: https://github.com/syncthing/syncthing/blob/a0fd63763f153dd0969199ab889fdfb53de33c00/lib/model/folder_sendrecv.go#L392
[st-override]: https://github.com/syncthing/syncthing/blob/a0fd63763f153dd0969199ab889fdfb53de33c00/lib/model/folder_sendonly.go#L131
[st-faq-seed]: https://docs.syncthing.net/users/faq.html#can-i-help-initial-sync-by-copying-files-manually
[st-faq-history]: https://docs.syncthing.net/users/faq.html#how-can-i-view-the-history-of-changes
[st-events]: https://docs.syncthing.net/dev/events.html
[git-rebase]: https://git-scm.com/docs/git-rebase#Documentation/git-rebase.txt---committer-date-is-author-date
