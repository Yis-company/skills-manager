# Agents Manager remediation — validation

The approved plan was implemented locally. `review.json` records `decision: approved`. Application changes and the changeset are uncommitted; no application changes were pushed to the init-only PR.

## Result by finding

| Finding | Delivered behavior | Evidence |
| --- | --- | --- |
| F1 · Preset scope | Global/agent workspace actions explicitly switch the shared preset; project actions name project-local add/remove. Every removal-producing workspace switch is reviewed, including after the old first-switch flag. The selected host’s additions, removals, paths, and preserved edits are shown; other hosts’ exact changes are explicitly uninspected. The backend revalidates the review under the repository lock. | Browser repeat-switch/cancel and direct no-removal cases; Rust stale-state, preserved-copy and successful owned-link removal cases. |
| F2 · Conflict evidence | Read-only comparisons cover skill conflicts, instruction/MCP conflicts, same-name presets, and edited agent copies. Named versions and paths precede the chosen outcome’s confirmation. Refresh/error states do not offer blind resolution. Reviewed mutations reject changed evidence. Existing recovery remains, including a pre-resolution history point for reviewed preset decisions. | Rust retained refs, binary instruction payloads, MCP references/nonportable-value rejection, preset memberships and edited-copy tests; Sync and new browser scenarios. |
| F3 · Keyboard/semantics | Skill titles and selection have separate native controls; drag handles retain keyboard activation without dead outer tab stops. Shared confirmation, comparison and Help dialogs have names, focus entry/containment/restoration, labeled close controls, and isolated Escape behavior. Focus-visible styling and text contrast tokens were improved. | Keyboard inspection in grid/list, nested Tab/Escape/focus restoration, Help palette access, existing drag-order and confirmation regressions. |
| F4 · Guidance | Help and affected README/workflow passages distinguish viewed preset, shared active preset, actual agent copies, project scope and Sync. Help is reachable from the command palette and uses the shared accessible modal. | Browser Help path; source/prose review; linked Sync compatibility anchor updated. |
| F5 · Library hierarchy | Viewed and shared active presets are named beside the content. Filters say In preset / Not in preset. Actual presence is visibly labeled On agents. Display controls and operational actions are grouped separately; creator metadata is suppressed when its group supplies it. | Library scenarios and current-worktree screenshots in light/dark, both views, long names and enlarged text. |
| M1 · Log failures | Loading, error/retry and successful empty states are distinct. Host-keyed queries prevent a result from being displayed under another host identity. | Injected agent-log failure followed by Retry; host/query scope source and existing delayed-host regression tests. |
| M2 · Host state | State dots have accessible names and visible state text accompanies hosts in Sync navigation. | Browser assertions/visual inspection and source review. |
| M3 · Expansion | Sidebar collapsers expose expanded state and identify controlled regions. | Browser expansion assertion and existing sidebar navigation/reordering checks. |
| M4 · Layout edges | Presets scroll locally while workspace navigation remains reachable. Cards size from their available width, descriptions have two lines, full names remain inspectable, toolbars wrap by purpose, and comparison/dialog overflow is contained. | Eight viewport/theme/text-size combinations with 25 presets and long content; bounds and horizontal-overflow assertions; six screenshots inspected. |

## Checks

- `pnpm test`: **202 tests passed**, 22 files.
- `pnpm build`: **passed** (TypeScript and production Vite build). Existing bundle-size/Browserslist advisories remain; no build error.
- `pnpm lint`: **passed**.
- `pnpm exec oxfmt --check <changed frontend/docs files>`: **passed**, 28 files.
- `git diff --check`: **passed**.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib core::change_review --offline`: **9 passed**.
- Same Cargo command with `core::scenario_service`: **10 passed**; `core::resource_sync`: **3 passed**; `core::merge::integration_tests`: **32 passed**. **54 Rust tests total**, zero final failures.
- `pnpm exec playwright test -c e2e/playwright.config.ts e2e/specs/critique-remediation.spec.ts e2e/specs/sync.spec.ts e2e/specs/library.spec.ts e2e/specs/library-detail.spec.ts e2e/specs/tags.spec.ts e2e/specs/sidebar-order.spec.ts e2e/specs/preset-skill-order.spec.ts e2e/specs/resource-workspace.spec.ts e2e/specs/workspace-bulk.spec.ts e2e/specs/project-git.spec.ts --workers=4`: **65 passed**, Chromium fixtures, 20.3 seconds.
- Final independent manual detector: `impeccable detect --json src` returned **0 findings**, complete `[]`, empty stderr. It ran once for the final remediation pass; no overlay or native pixel scan was claimed.

## Fresh design review

Independent Assessment A (`remediation_design_review`) scored the revised surface **28/40**, before the final Help-modal and tooltip corrections. The source detector was isolated in Assessment B (`remediation_detector`). No later score is claimed.

The design review confirmed clearer scope, readable comparisons, coherent operational styling and recoverable states. Its Help accessibility and truncated-name tooltip findings were fixed and verified. Its suggestion to confirm additive-only shared switches was not adopted: the approved plan expressly keeps a clearly named, reviewed-by-backend switch without an extra confirmation when no removals occur. A browser scenario verifies that behavior.

Questions skipped: no unresolved priority issue within the approved scope requires another product decision.

## Evidence boundaries

Browser scenarios use the current worktree with fake Tauri IPC; Rust tests use temporary stores and repositories. This verifies frontend behavior and the changed backend paths separately, without modifying real user libraries or connected hosts.

The screenshots in `.impeccable/review/2026-11-01-remediation/` cover 1440×860/default text and 1100×640/largest text in light/dark. They were inspected after the final layout changes. No development server remains on the fixture port (1421).

Native desktop/VoiceOver behavior and real SSH-host execution were not exercised. No remote CI or release acceptance is claimed. An older host CLI may not support the new comparison command; comparison errors remain actionable and do not trigger an automatic host update.

Comparisons cap file count/depth and text byte/line size. Binary and oversized files receive summaries. Existing Git revision IDs and direct text/file metadata checks are reused; no application hashing scheme or credential resolution was introduced. MCP comparisons preserve portable credential references and reject nonportable current values.

## Local evidence locations

- `.impeccable/review/2026-11-01-remediation/`: six verified Library/comparison screenshots.
- `/tmp/skills-manager-browser-final-all.log`: final 65 browser results.
- `/tmp/skills-manager-unit-final.log`: final frontend unit results.
- `/tmp/skills-manager-rust-0.log` through `rust-3.log`: the four final focused Rust groups.
- `/tmp/skills-manager-build-final.log`, `lint-final.log`, and `format-check.log`: final frontend checks.
