# Make every agent change clear

Implementation proposal for all findings in the Agents Manager critique of 1 November 2026.

**Decision requested:** approve the scope, interaction changes, and verification below. Approval authorizes implementation and local fixture checks. Publishing, merging, releasing, or changing live libraries and hosts remains a separate action.

## Outcome and scope

A developer can tell what is in a preset, what an agent actually loads, which hosts a change reaches, and what will be replaced before confirming. All five priority findings and the smaller observations are covered below.

Preserve the established desktop interface, product vocabulary in `CONTEXT.md`, and the existing agent, project, and Sync behavior. This is an Operate surface: use explicit language, reliable keyboard behavior, and clearer information hierarchy. App screens use code-first; the saved comp-first setting applies to landing pages and showcases.

The current init-only PR #99 remains limited to the three committed context/configuration files. Keep the remediation separately reviewable; do not push application changes into that PR as a side effect. This plan does not authorize a rebrand, new sync topology, new credential storage, a migration, or a hashing scheme.

## Interaction decisions proposed for approval

1. **Name the action and scope.** Library shows “Viewing preset: [name]” separately from “Shared active preset: [name]”. Membership filters and switches say “In preset” / “Not in preset”; actual deployment says “On agents”. A workspace preset control names either “Switch shared preset” or “Add preset skills to this project”. Keep the current project add/remove semantics and shared-switch semantics.
2. **Review every consequential preset switch.** Remove the one-time bypass. Before a switch that removes resources, show the current and next preset, owning host, verified agent additions/removals, preserved local edits, and direct additions affected. Explain that hosts participating in Sync follow the shared choice on their next successful sync. List known participating hosts and identify unavailable hosts; do not invent exact downstream file counts. A switch with no removals still names its shared scope, without an unnecessary second confirmation.
3. **Compare before choosing a conflict outcome.** “Compare changes” opens a read-only sheet with named versions, host provenance, resource-relative paths, additions/deletions, and a bounded text comparison. The selected keep/keep-both outcome gets a concise confirmation before resolution. The same pattern supports edited agent copies and same-name preset reviews where choices alter content or membership. Preserve existing resolution and safety-history behavior.
4. **Make inspection work from the keyboard.** Skill titles use native buttons or links; selection uses checkbox semantics. Nested switches, agent actions, and drag handles remain separate controls. Confirmation dialogs have an accessible name, initial focus, contained Tab navigation, Escape handling, and focus restoration. Keep existing pending-operation behavior while preventing duplicate submissions.
5. **Reduce ambiguity without removing useful information.** Separate browsing/view preferences from operational actions, name agent-state groups, remove redundant creator text only when the creator group already supplies it, and keep description/provenance discoverable. Make long preset navigation searchable or locally scrollable so workspaces remain reachable. Keep desktop density and the current light/dark identity.

## Sequence, ownership, and dependencies

**Owner:** Codex implements and verifies each phase. **Reviewer:** Yi approves this proposal and the final application changes. No schedule or additional owners are assumed.

### 1. Shared accessible controls

Harden the existing shared confirmation path and reuse proven focus behavior from the project Git dialog instead of adding a modal framework. Correct card/list activation, view-button labels, focus-visible treatments, keyboard access to tag actions, expansion semantics, and non-color status names.

**Dependency:** none after plan approval. **Done when:** inspection, selection, confirmation, cancellation, and focus return work in a keyboard-only fixture without unintended mutations or broken dragging. Existing confirmation consumers retain their pending and error behavior.

### 2. Scope, preset review, Library hierarchy, and help

Update preset controls and the switch hook together. Extend the preview contract only as needed for accurate additions/removals, target identity, direct additions, and preserved edits. Revalidate the preview on confirmation; changed state requires a refreshed review. Keep selected/viewed preset distinct from the shared active preset.

Update in-app help and directly affected product-guide passages to explain the current Library → preset → actual agent-copy model and Sync terminology. Make relevant help reachable from the shell or command palette. Keep installation into the library distinct from adding to an agent.

**Dependency:** phase 1 for reliable review dialogs. **Done when:** project and global actions visibly name their different scopes; every removal-producing switch is reviewed even after a previous switch; cancel performs no mutation; Library labels and help agree with actual behavior. A long preset list does not displace all workspace navigation.

### 3. Conflict comparison and reviewed resolution

Add a small read-only comparison contract through the existing local/SSH Sync dispatch. Reuse stored skill conflict refs, retained resource versions, existing preset membership data, and agent-copy paths. Reuse the existing diff components for bounded text; show explicit summaries for binary, oversized, deleted, or unavailable content. Cap both file size and text-line complexity because the current line diff uses a quadratic matrix.

Bind requests to captured host/resource identity. Ignore late responses after the selection or host changes. Do not resolve credentials or read beyond the selected resource root; portable MCP comparisons retain credential references. Use existing Git revision IDs and direct comparison where needed rather than introducing a hashing system. Preview must not commit, resolve, fetch, rewrite metadata, or reapply resources.

Before mutation, validate that the reviewed versions still match under the existing repository lock. If not, refresh the comparison and ask the user to review again. Keep existing safety snapshots and resolution choices. An unavailable host or older CLI gets a clear compatibility/error state; no automatic installation or update.

**Dependency:** phase 1; typed backend data before the complete frontend comparison flow. **Done when:** skill, instruction, MCP, preset, and edited-copy decisions expose the relevant versions or an explicit unavailable state; outcome confirmation names what is retained/replaced; stale reviews cannot silently apply a different version. Existing recovery remains discoverable.

### 4. Error states and layout edges

Distinguish loading, empty, failed, stale, and successful agent-log states. Retain the selected host, offer Retry, and prevent a late response from appearing under another host. Add text/accessible names for host-state dots and expansion state for sidebar sections.

Check the minimum desktop window and largest text setting, long resource/host names, many presets, empty results, offline hosts, and large comparisons. Reflow toolbars and Sync navigation where needed; contain wide diffs within their own region. Keep actions, modal footers, and workspace navigation reachable.

**Dependency:** phases 1–3 for final integrated verification. **Done when:** failure cannot masquerade as empty success; keyboard and pointer paths reach the same actions; the supported desktop sizes and text settings do not clip primary controls.

### 5. Verification, polish, and delivery evidence

Run focused behavior tests first, then the existing frontend checks and affected Rust checks. Add a changeset for application changes. Batch light/dark visual checks at normal and minimum supported desktop sizes and at the largest text setting, fix what the pass reveals, then run the mechanical detector once on changed markup. Finish with an Impeccable polish pass and re-critique the addressed surfaces.

Report source, automated fixture, native-runtime, remote-host, and CI evidence separately. No score increase is an acceptance substitute. Do not claim a remote host worked from mocked IPC alone. Publishing remediation requires an explicit next instruction.

## Finding-to-acceptance map

| Finding | Repair and acceptance | Proof and dependency |
| --- | --- | --- |
| F1 · P1 · Hidden preset scope | Explicit shared/project action labels; repeat removal review; accurate targets, direct additions, and preserved edits; cancel writes nothing; stale review refreshes. | Browser scenarios cover first and later switches, no-removal switches, project-local actions, unavailable hosts, and changed state. Rust fixtures assert preview/apply agreement and local-edit protection. Phase 2 depends on phase 1. |
| F2 · P1 · Conflict choices without evidence | Compare both versions, file lists, provenance, and proposed outcome for skills, instructions/MCPs, edited copies, and same-name presets. Fail visibly when a comparison is unavailable. | Temporary repositories cover modified/deleted/renamed/binary/large content, missing refs, stale review, no preview writes, and existing recovery. Browser fixtures cover inspect → choose → confirm/cancel and late responses. Phase 3 depends on phase 1 and backend contract. |
| F3 · P1 · Keyboard and semantic gaps | Native detail activation and selection; named view/close buttons; focus-visible and focus-accessible actions; accessible confirmation semantics and focus restoration. | Keyboard-only browser flow opens details, toggles selection separately, keeps drag handles usable, contains modal focus, blocks duplicate apply, and restores focus after close. Native VoiceOver spot-check is separate runtime evidence. Phase 1. |
| F4 · P1 · Stale help | Guide and contextual help teach viewed vs shared active preset, actual agent copies, project scope, and current Sync labels. Help is reachable without About navigation. | Browser walkthrough checks discoverability and follows the documented flow against current behavior; review affected prose instead of writing brittle copy-snapshot tests. Phase 2. |
| F5 · P2 · Library scope/hierarchy | Name viewed and shared active presets; distinguish membership from actual presence; group view preferences and operational actions; suppress only redundant grouped metadata. | Fixture has different viewed/active presets and different membership/agent states; changing filters/views does not mutate resources. Check long names and both card/list views. Phase 2. |
| M1 · Failed log looks empty | Distinct error and Retry; show empty only after a successful empty response; keep stale data clearly identified if retained. | Inject failure, then retry success; delay host A and switch to B; verify A cannot overwrite B. Phase 4. |
| M2 · Color-only host state | State name is available without color or selecting the host; loading/offline states retain identity. | Accessible-name assertions and visual check for needs-review, offline, paused, and up-to-date. Phases 1 and 4. |
| M3 · Sidebar expansion semantics | Collapsers announce expanded/collapsed state and expose a stable controlled region; focus remains usable. | Keyboard toggle and accessible-state assertions. Phase 1. |
| M4 · Many presets and narrow/large-text layouts | Keep workspace navigation reachable; make overflow localized; keep long names inspectable; preserve useful descriptions without excessive repeated metadata. | Seed 25+ presets, long names and descriptions; verify 1440×860 and minimum 1100×640, default/largest text settings, light/dark, Sync review and dialogs. Phase 4. |

## Evidence and source locations

The critique scored 23/40. The deterministic source scan returned zero findings; it does not test scope comprehension or accessibility behavior. Historical screenshots are useful identity evidence; the installed app was observed without mutation, but its build is not matched to this checkout.

- `src/hooks/usePresetSwitch.tsx:23` skips the removal preview after a stored first-switch flag. `src/components/PresetBar.tsx:185` branches between shared switching and project activation/removal.
- `src-tauri/src/core/scenario_service.rs:591` currently previews removal pairs only. The apply path already protects edited copies; the preview must describe that protection accurately.
- `src/views/sync/Review.tsx:60` renders choices from host/name metadata. `src/lib/tauri.ts:1079` defines that metadata-only conflict contract.
- `src-tauri/src/core/sync_service.rs:196` describes skill conflicts; `core/merge/resolve.rs:40` uses retained refs and existing history protection. `core/resource_sync.rs:329` can describe retained resource content internally. `core/host_sync.rs:22` controls allowed Sync screen commands.
- `src/components/DocumentDiffViewer.tsx:39` uses an O(n²) line-diff matrix; `SkillSourceDiffViewer.tsx` already supplies text/binary/large-file presentation. Reuse with explicit limits.
- `src/components/ConfirmDialog.tsx:62`, `LibrarySkillCard.tsx:97`, `LibrarySkillRow.tsx:70`, and `src/views/MySkills.tsx:1321` contain the inspected accessibility gaps. `ProjectGitDialog.tsx` and `e2e/specs/project-git.spec.ts` provide existing focus/viewport patterns.
- `src/views/sync/HostPane.tsx:137` converts a rejected log load into an empty list. `src/views/sync/parts.tsx`, `src/views/Sync.tsx`, and `src/components/Sidebar.tsx` own status and expansion presentation.
- `src/i18n/en.json:25` contains obsolete help. `CONTEXT.md` is the vocabulary authority; update relevant passages in `README.md` and `AGENTS-MANAGER.md` only where this work changes the described flow.
- Existing tests include `e2e/specs/sync.spec.ts`, `preset-skill-order.spec.ts`, `sidebar-order.spec.ts`, `resource-workspace.spec.ts`, and `src/lib/librarySkillQuery.test.ts`. Extend behavior coverage rather than replacing stable tests with markup checks.

## Commands and verification limits

Planned implementation checks: `pnpm test`, `pnpm build`, `pnpm lint`, formatting checks for changed files, focused `pnpm e2e` specifications, and focused Rust tests in `src-tauri` for touched Sync/preview behavior. Use temporary repositories and fake IPC for destructive outcomes. Keep real user libraries and connected SSH hosts untouched.

Dependencies are not installed in this checkout and ports 1420/1421 had no development server during critique. Install from the frozen lockfile only when implementation begins, use the fixture harness for browser evidence, and report any toolchain/platform blockers. Run native checks against a build from this worktree when available; do not reuse the installed app as acceptance evidence.

Approval gate: `plannotator annotate plan.html --gate --json --require-approval --result-file review.json`. Only an explicit approved decision authorizes implementation; annotations require revision and another review, and dismissal does not approve the work.
