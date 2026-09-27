# Update CLI from Settings

## Outcome

Expose an Update CLI button beside Check for every saved host in Settings → Remotes. Opening it checks the host and explains the proposed update; installation starts only after the user confirms in the existing dialog.

## Verified starting point

RemoteHostsSection.tsx already opens the shared update dialog, but its action appears only after Check reports incompatible versions. RemoteCliUpdateDialog.tsx already probes on opening, shows the current and required versions and Yis-company/skills-manager source, handles installation stages and errors, and reconnects through the existing host flow.

The existing remote_host_install_cli command installs the exact version required by the running desktop app. This plan extends discoverability and dialog states; it preserves that implementation and the earlier remote-update plan.

## Proposed behavior

For each saved host, keep Check and add a consistently visible Update CLI action. Disable conflicting actions during checks, connections, or an installation using the existing pending state.

Opening Update CLI performs a fresh probe. Show Checking host while pending, keep installation unavailable until the result arrives, and allow cancellation before installation.

If versions differ, show both versions and retain Install matching version and reconnect. The target is the local app version, including when that requires a downgrade; it is never the latest release independently of the app.

If versions already match, show CLI is up to date with the matching version and a close action. Do not offer or invoke an unnecessary reinstall or reconnect.

If the probe fails, show its actual error and retain Retry. A failed probe does not authorize installation. Preserve existing install progress, duplicate-action protection, retry behavior, reconnect errors, and protection against switching back from a newer host selection.

## Implementation sequence

After approval, Luna updates the Settings entry point and shared dialog in a bounded frontend change. Preserve the connection-failure entry point and reuse existing context, API calls, and host selection safeguards.

Update the supported locale strings for the button, neutral checking description, and matching-version result. Use the existing Settings button styling, keyboard behavior, focus handling, and status/error semantics.

Extend the existing remote CLI browser tests and add a focused changeset describing the visible Settings action. No backend refactor or new installer path is needed.

## Acceptance and verification

A saved host exposes Update CLI before Check has ever run. Opening and cancelling it makes no install or connect request.

A matching-version probe displays the up-to-date state and makes no install or connect request. A failed probe supports retry while keeping installation unavailable.

A mismatched host still installs exactly once after explicit confirmation, reports progress and failures, and reconnects using the existing flow. Existing stale-selection and reconnect regression coverage continues to pass.

Run the focused e2e/specs/remote-cli-update.spec.ts browser suite, relevant frontend tests, pnpm lint, and pnpm build. Report local results separately from remote CI or live SSH proof. Inspect the Settings controls and dialog at wide and narrow widths.

## Scope and approval

No automatic updates, latest-version policy, missing-CLI provisioning, bulk updates, remote desktop replacement, or changes to skills and library data. No live remote installation, publication, or deployment is part of this implementation.

Review plan.html through the required Plannotator approval gate and record its structured decision in review.json. Implementation begins only after approval.
