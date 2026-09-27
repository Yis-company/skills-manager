# Agents Manager

The Library, Global Workspace and Project Workspace contain Skills, Instructions and MCPs. The host selector applies to all three. A remote host keeps its own library and project paths; selecting it does not copy the local library to that machine.

The app keeps its existing executable name, application identifier, `~/.skills-manager` data paths, and `skills-manager-cli` command. The rebrand changes the visible product name.

## Instructions

Edit a supported instruction file where it lives, or save whole files and their referenced documents as a reusable bundle. Directory scopes and document links are separate: a Markdown link does not imply an agent automatically loads that document. Scans show excluded directories and partial results.

Deployment copies a reviewed snapshot and remembers its source. Editing the library never changes a deployed file automatically. Review an update to compare the previous deployment, current project and new library content. Resolve overlapping edits explicitly. Deleting a library item and removing a deployment are separate actions; locally edited files are preserved.

The editor preserves native instruction filenames and frontmatter. Shared `AGENTS.md` and agent-specific layouts depend on each agent's loading rules. For example, Claude Code's default AGENTS fallback can be suppressed by a project `CLAUDE.md` or `CLAUDE.local.md`.

## MCP connections

Add a definition manually, import a representable existing configuration, or browse the official MCP Registry. Review the command or endpoint and required inputs. Adding a catalog entry does not run it. Agents Manager writes configuration; the target agent handles startup and authentication.

Import opens a draft; Save adds it to the library. Catalog entries offer supported package/endpoint choices and keep the selected registry version with the definition. Fill required argument placeholders before saving. Package mappings follow the [official Registry schema](https://raw.githubusercontent.com/modelcontextprotocol/registry/main/docs/reference/server-json/draft/server.schema.json) and [uv's versioned tool syntax](https://docs.astral.sh/uv/guides/tools/#requesting-specific-versions); custom runtimes and package formats remain manual setup.

Definitions carry shared defaults. Deployments can override supported settings without changing the library definition. Same-name collisions offer keep, replace or rename. No deployment target is selected by default when adding an item.

Credentials remain outside the library. Supply an environment-variable reference and make that variable available to the target agent on the target machine. The application does not read its value or install secrets. Existing configurations with literal credentials are not copied into portable definitions. Unrelated configuration and authentication entries remain untouched.

Current capability boundaries:

| Agent | MCP scopes | Instruction constraint |
|---|---|---|
| Claude Code | Global and project | Native AGENTS fallback depends on version and loading settings |
| Codex | Global and trusted project | Honors AGENTS override/fallback precedence |
| Antigravity | Global and project | Uses native AGENTS/GEMINI/rule paths; secret-dependent MCPs need a verified supported reference mechanism |
| Hermes | Global | SOUL is identity; project context files have their own precedence |
| Cursor | Global and project | Global User Rules are edited in Cursor's UI; project/nested files are supported |

No agent is given an invented nested MCP configuration or an unsupported credential-reference syntax.

## CLI and SSH

Use `instructions` or `mcps` with an action and optional request fields in a JSON file (`--input -` reads stdin). The CLI uses the same core as the desktop app:

```sh
skills-manager-cli --json instructions list
skills-manager-cli --json instructions get --id <bundle-id>
skills-manager-cli --json instructions scan --agent codex --project <project-id>
skills-manager-cli --json mcps capabilities
skills-manager-cli --json mcps inspect --agent cursor --project <project-id>
skills-manager-cli --json mcps catalog --input catalog-search.json
```

For example, `catalog-search.json` contains `{"query":"filesystem"}`. Request action fields are supplied by the command. Create a bundle using `instructions save --input bundle.json`:

```json
{
  "name": "Team conventions",
  "files": {
    "AGENTS.md": "Use pnpm. For testing, see [testing](docs/agents/testing.md).\n",
    "docs/agents/testing.md": "Run the relevant behavior tests before finishing.\n"
  }
}
```

Save returns its identifier and revision. Edits to an existing bundle supply `id` and `expected_revision`. MCP edits use `definition.id` and `expectedRevision`. An outdated revision is refused.

Prepare an instruction deployment request with `instruction_id` and `target: {"agent_key":"codex","project_id":"..."}`. MCP preview requests use `target: {"agentKey":"codex","projectId":"..."}` and `operations: [{"kind":"deploy","definitionId":"..."}]`.

```sh
# Validation-only preview: no target, library or preview-state writes.
skills-manager-cli --json instructions preview --input deployment.json --dry-run

# Store a reviewable preview, then apply its returned identifier.
skills-manager-cli --json instructions preview --input deployment.json
skills-manager-cli --json instructions apply --id <preview-id>
```

`--dry-run` is supported on `preview`; unsupported actions reject the flag instead of executing a mutation. A stale preview must be recreated. To manage a registered SSH host, add `--host <host-id>` to a resource command. The existing matching remote CLI and SSH access are required; its macOS/Linux support is unchanged. Running the CLI directly on that host also works.

Removing a library item with active deployments requires `{"id":"...","detach":true}`. It removes the saved links and definition while leaving native files in place. `undeploy` is a separate action that removes only unchanged managed content.

Interrupted instruction operations can be recovered from Instructions or `instructions recover`. For MCPs, inspect/deployments reports pending recovery IDs; use the displayed recovery control or `mcps recover --id <recovery-id>`. Recovery checks the original destination and preserves later edits. An interrupted library directory replacement blocks Git backup until recovered.

## Backup compatibility

Reusable instruction bundles and MCP definitions live under the existing Git library in `.agents-manager/`. Deployment state, overrides, journals, catalog caches and secrets are excluded. Restoring a library does not deploy its contents into projects.

Upgrade all devices sharing an expanded library before enabling synchronization. Older clients can mishandle the new resource types; a schema marker cannot change an old binary's behavior. New clients validate portable resources and refuse detected incompatible writes.

A conflicting instruction bundle or MCP definition pauses the merge without applying merged files. In the Library, review both versions and select the version to keep, then run sync again. The CLI exposes the same choices:

```sh
skills-manager-cli --json instructions conflicts
skills-manager-cli --json instructions resolve --input conflict-choice.json
```

The choice file contains `{"id":"<conflict-id>","choice":"local"}` or `"remote"`. Here local means the selected host's library; remote means the Git backup version. Choices are tied to the compared versions and expire when either changes.

Desktop backup controls still operate on the local machine. Selecting an SSH host does not retarget them. Run backup on the remote machine through its own app or existing CLI Git commands.
