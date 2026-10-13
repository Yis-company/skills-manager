---
"skills-manager": minor
---

Deployed instructions and MCP servers now follow library changes after a sync
and at startup. An update that applies cleanly happens on its own; one that
meets edits made on this device waits for review and is recorded in the
activity log. Which items are deployed still stays per device. Fixes updating an
MCP deployment that has no per-target override, which failed with "overrideConfig
may contain only server fields".
