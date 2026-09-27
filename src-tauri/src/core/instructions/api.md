# Instructions request API

The desktop command and headless host dispatch both accept
`instructions_request({ request })`. Requests are tagged by `action`.

```ts
type Target = {
  agent_key: "claude_code" | "codex" | "antigravity" | "hermes" | "cursor";
  project_id?: string;       // absent means the user's home directory
  relative_dir?: string;     // path under the agent's instruction root
};

type Request =
  | { action: "list" }
  | { action: "get"; id: string }
  | { action: "save"; id?: string; expected_revision?: string; name: string;
      description?: string; files: Record<string, string> }
  | { action: "remove"; id: string; detach?: boolean }
  | { action: "scan"; target: Target; include_dirs?: string[] }
  | { action: "read"; target: Target; path: string }
  | { action: "write"; target: Target; path: string; content: string;
      expected_revision: string }
  | { action: "preview"; target: Target; instruction_id: string; dry_run?: boolean }
  | { action: "apply"; preview_id: string;
      resolutions?: Record<string, "keep_local" | "take_library">; dry_run?: boolean }
  | { action: "deployments" }
  | { action: "undeploy"; deployment_id: string }
  | { action: "recover" };
```

Library bundles are complete `.md`/`.mdc` files. `list` returns summaries,
`get` returns file contents, and `save` stages a complete bundle before
swapping it into place. Updates require the `revision` returned by `get`;
stale updates are rejected. Removing a bundle with linked deployments requires
`detach: true`; this clears the links and preserves all target files. `undeploy`
is separate and removes only files that still match deployed content.

`scan` finds native instruction entrypoints and user-selected include
directories, then follows local Markdown and native `@file.md` references as a
separate graph. It does not fetch links. Unreferenced project Markdown is not
treated as an instruction node. Missing, ignored, excluded and out-of-scope
paths appear in the response; scans are bounded by depth, visited-entry count,
file count, and reference count. Scan is read-only; use `read` before editing.
File writes require the opaque `revision` from `read` and recheck both the
resolved target and bytes under the write lock. Preview reports create,
replace, merge, delete, unchanged, and three-way conflicts. Conflicts need an
explicit `keep_local` or `take_library` resolution. Apply preflights every
target path and byte value, journals the transaction, and restores only files
that still contain transaction output. `recover` returns transaction id, kind,
status, affected relative paths, failures and remaining recovery work.

Global Cursor instructions are unsupported. Global Claude Code, Codex,
Antigravity and Hermes use their native config roots; Codex honors `CODEX_HOME`
and Hermes honors `HERMES_HOME`. Global Hermes manages `SOUL.md` only.

Responses retain the stable shapes:

- `list` / `deployments` / `recover`: `{ items: [...] }`
- `get` / `save`: `{ item: ... }`
- `remove`: `{ removed: true, detached: string[] }`
- `scan`: `{ target, files, excluded, references, warnings }`
- `read` / `write`: `{ path, content?, revision }`
- `preview`: `{ preview_id, instruction_id, definition_revision, target, changes, warnings }`
- `apply`: `{ applied, failed, partial, transaction_id, status }`
- `undeploy`: `{ detached: true, removed }`

Recovery items use `{ transaction_id, kind, status, files, failed?, remaining?,
created_at }`.
