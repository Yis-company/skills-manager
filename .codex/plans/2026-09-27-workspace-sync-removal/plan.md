# Workspace updates and persistent agent removal

Implementation review · 27 September 2026

## Intended result

Add Update All from Library to update every eligible, already deployed project skill across the whole workspace, regardless of filters or selection. This does not install the entire library.

Fix global agent skill removal so disabled skills stay disabled after restart. The reported returning skills are improve-react, next-best-practices, next-upgrade, and react-doctor. The screenshot shows these four In sync across enabled agents. Their actual host has not been reproduced or modified.

## Evidence and decisions

src/views/ProjectDetail.tsx:733 updates selected rows only. src/lib/projectSkillGroups.ts:102 allows project_newer as an update candidate, but src-tauri/src/commands/projects/center_sync.rs:331 rejects locally newer copies. Align individual and batch actions with backend protection; do not weaken it or introduce an implicit destructive reset.

src-tauri/src/commands/sync.rs:140 updates SQL during removal without persisting sync metadata. The preset toggle at :363 already uses a repository lock and metadata writer. Reuse that pattern for enable and disable.

src-tauri/src/core/app_state.rs:116 reloads metadata at startup; core/skill_store.rs:1126 restores scenario tool toggles, and core/scenario_service.rs:717 applies the startup scenario. Stale enabled metadata can restore a removed skill. This is a confirmed code mechanism; reproduce it with isolated tests before fixing.

Removal currently logs filesystem errors but then drops the record and returns success. Propagate actionable failures and preserve protections for shared paths and independently replaced content.

## Update All policy

Update center_newer variants from the library. Skip project_newer and diverged variants and report that their local changes require review. Exclude in-sync and unlinked copies.

Evaluate each effective variant: a grouped row can contain both a safe library update and another agent’s locally changed copy. Preserve vendored paths and grouped identities. Update All uses the entire project; the existing selection action keeps its selection scope. Ordinary individual and selected-batch updates follow the same safe policy.

Capture the originating host and project. Prevent duplicate submissions while pending and keep subsequent operations and refreshes on that captured host. Report updated, skipped-for-review, and failed counts separately. Refresh authoritative project state after success or partial failure. Use existing localized UI conventions.

## Implementation sequence after approval

1. Reproduce the status mismatch and full-project candidate selection with focused frontend tests. Reproduce disable → metadata reload → startup scenario application with temporary stores and deployment paths.

2. Persist enable/disable using the existing lock and metadata write helper. Propagate persistence and removal failures accurately, preserve shared and independent content, and verify durable explicit re-enable.

3. Add Update All using existing project data and update commands. Apply eligibility per effective variant, pending state, outcome feedback, originating host/project capture, and authoritative refresh.

4. Run focused frontend and Rust tests, then applicable lint, type/build, formatting, and repository-required checks. Keep local results distinct from remote CI and real-host evidence. Use bounded Luna implementation work with parent integration.

## Acceptance checks

Eligible copies hidden by filters or absent from selection are updated; unrelated projects and hosts are untouched. The action updates deployed copies only.

Mixed-status grouped and vendored copies update only eligible variants. Local-newer and diverged copies stay unchanged and are reported for review. Individual, selected-batch, and all-project actions agree with backend protection.

Repeated clicks, partial failures, and host switching during an operation preserve correct targeting and feedback.

Disabling for one agent and multiple agents survives actual metadata roundtrip and startup application. Explicit re-enable survives restart.

Another agent’s shared deployment path and independently replaced content remain intact. Filesystem and metadata failures reach the caller truthfully. Tests use temporary paths and synthetic skills.

## Uncertainty and scope limits

Source-reference backfill may also recreate missing deployments. Change it only if the restart reproduction demonstrates that it restores an explicitly disabled skill. Do not expand into general scanning or reconciliation changes.

The four reported skills may share source or deployment characteristics not visible here. Isolated tests prove repaired code paths, not the user’s installed build or host. Any later live verification must name the host.

No schema changes, synchronization framework, new hashing, real-user skill deletion, live installation, release, or publication are included.

## Approval boundary

This is a proposal. Implementation starts only after Plannotator records approval in review.json. Approval authorizes these two scoped changes and isolated verification. If changes are requested, revise plan.md, regenerate plan.html, and repeat the gate.
