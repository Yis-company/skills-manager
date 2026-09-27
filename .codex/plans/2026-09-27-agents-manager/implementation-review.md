# Implementation review

The user approved the canonical plan directly in the conversation. Implementation incorporates `origin/main` at `081eed8` and preserves the pre-existing `.serena/` directory.

Independent read-only review covered the frontend/API integration, CLI/backup integration, and instruction recovery paths. It identified:

- Canonical Claude agent-key mismatch and an MCP import response that did not match the editor. The adapter now uses `claude_code`; import prepares a draft for explicit Save.
- Project tabs pinned to the first configured agent. They now use the existing project agent allowlist with a selector.
- Missing catalog provenance/variant handling. Supported variants retain a concrete registry version and show required inputs; unsupported setups stay manual.
- Library removal leaving deployment records unusable. Explicit detach removes links while preserving native content.
- Read-only dry runs against a pre-feature database. Missing resource state is treated as empty without migrating or creating a database on disk.
- A merge/SQLite cleanup crash window. Reconciliation proves the exact reviewed merge exists before cleaning the stale conflict record.
- Instruction recovery state/path consistency and missing native instruction/reference cases. These were escalated to Sol Ultra for focused corrections and regression tests.

Additional parent review found MCP stale-file overwrite, incomplete database finalization, unsafe renamed-entry summaries, and previews that omitted the actual proposed settings. These were escalated to Sol Ultra. Recovery journals contain only validated managed entries; full native configs remain transient.

CodeRabbit CLI is installed but not authenticated. No CodeRabbit review was run; the independent local reviews above are the fallback, not a claim of remote review.

Final check results and remaining verification limits are recorded separately in `validation.md` when the checks finish.
