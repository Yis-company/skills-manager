# Agents Manager: skills, instructions, and MCP configuration

**Status:** Proposed; implementation requires approval of this plan.  
**Planning baseline:** `origin/main` at `081eed8`, including the existing SSH remote CLI updater.  
**Working branch:** `t3code/agents-manager-expansion`; no merge or implementation has been performed.  
**Authorship:** Astra planning agent; integrated against repository and official-documentation findings by the parent agent.

## 1. Product outcome

Expand Skills Manager into **Agents Manager**, retaining the existing Library, Global Workspace, Project Workspace, and host selector. Each workspace gains **Skills / Instructions / MCPs** tabs.

Users maintain reusable instruction bundles and MCP definitions, edit supported agent files, and explicitly deploy reviewed changes to global or project targets. Skills and skills-only presets retain their current behavior.

Visible branding changes; application identifiers, executable and CLI names, existing data locations, and compatibility paths remain unchanged. CLI parity and SSH support for all three resource types are launch requirements.

Launch adapters cover Claude Code, Codex, Antigravity, Hermes, and Cursor according to their real capabilities. Unsupported scopes are labeled and unavailable rather than represented by invented files.

### Scope boundaries

- Instructions are whole files or bundles with referenced Markdown documents. No composable sections, AI calls, automatic rewrites, or content generation.
- MCP management configures local stdio and remote connections. It does not install dependencies, launch servers, manage processes, implement OAuth, or store credentials.
- Linked deployments update only after explicit review. Saving a library item never updates projects automatically.
- Each SSH host owns its library. No cross-host transfer is added.
- Backup and multi-device sync include reusable definitions, not deployed project files, machine-specific deployment records, or secrets.

## 2. Navigation and concrete screens

Preserve current project registration and navigation. Add resource tabs within `/my-skills`, `/global-workspace/{-$agentKey}`, and `/project/$id`. Persist the active resource in route search state so reload and back navigation work, defaulting existing links to Skills. The selected host remains prominent. Existing linked skill workspaces remain skill-only in this release.

### A. Project Instructions

The existing sidebar and host selector frame a project page with Skills / Instructions / MCPs tabs. The Instructions tab has a directory tree on the left and editor on the right. The selected file shows its actual path, applicable agents, loading caveats, symlink destination if any, and unsaved edits. A separate reference panel lists local document links and missing targets.

Example: `AGENTS.md`, `backend/AGENTS.md`, and `frontend/CLAUDE.md` are directory-scoped files; `docs/agents/testing.md` is a referenced document. These relationships stay separate. Linking a document does not imply automatic loading.

A linked file shows its library source, revision, local edits, and available updates. “Shared” and “Agent-specific” are project choices. A layout change previews every file operation and preserves existing files until explicitly resolved. Scan exclusions and incomplete results are visible.

### B. Add MCP to Library

Manual, Import existing, and Online catalog lead to one typed definition editor: name, connection/transport, command or URL, settings, and external credential references. Optional global/project agent targets are all unchecked initially.

Review separates the library save from selected deployments, shows actual destination paths and scoped diffs, and requires explicit apply. A library save can succeed while a deployment fails; report each result accurately. Unsupported choices, such as Hermes project MCP, explain the limitation.

### C. Review linked updates

Show previous deployment, current target, new library content, and proposed result. Distinguish library-only changes, preserved project edits, and conflicts. Let the user keep the target version or edit a conflict result; apply remains unavailable until required conflicts are resolved.

MCP updates use structured fields and show deployment overrides retaining precedence. Same-name collisions offer **Keep existing / Replace / Rename**. Illustrative mockups in the HTML represent these three screens, not implemented functionality.

## 3. Agent capability contract

Use an explicit capability table and focused agent adapters. Do not infer one agent's scopes or syntax from another.

| Agent | Instructions | MCP configuration | Important behavior |
|---|---|---|---|
| Claude Code | Global `~/.claude/CLAUDE.md`; project/nested `CLAUDE.md`, supported `CLAUDE.local.md`, native `AGENTS.md` fallback | Global `~/.claude.json`; project `.mcp.json` | Native AGENTS support begins at v2.1.277; ancestor CLAUDE files can suppress fallback. Preserve private project MCP entries in `.claude.json`; do not silently change global loading settings. |
| Codex | Global `$CODEX_HOME/AGENTS.override.md` then `AGENTS.md`; project per-directory override then AGENTS/configured fallback | Global `$CODEX_HOME/config.toml`; trusted project `.codex/config.toml` | At most one instruction file per directory. Respect existing trust. Use `env_vars`, `bearer_token_env_var`, and `env_http_headers` for supported references. |
| Antigravity | Global `~/.gemini/AGENTS.md`/`GEMINI.md`; project/nested counterparts and supported rule files | Global `~/.gemini/config/mcp_config.json`; project `.agents/mcp_config.json` | Remote key is `serverUrl`. Secret interpolation was not verified: public connections work; reference-dependent deployment stays unavailable until supported syntax is proven. |
| Hermes | Global `~/.hermes/SOUL.md` identity; project `.hermes.md`, recognized alternate context files and recursive `AGENTS.md` | Global `~/.hermes/config.yaml` only | SOUL is identity, not generic global AGENTS. Respect `HERMES_HOME` and native context priority. Project MCP is unsupported. `${VAR}`/`${env:VAR}` references are documented. |
| Cursor | Global User Rules through native UI; project `.cursor/rules/*.mdc` and root/nested `AGENTS.md` | Global `~/.cursor/mcp.json`; project `.cursor/mcp.json` | No documented global rules file: show native-UI guidance. MCP supports `${env:NAME}`; `envFile` is stdio-only. |

Nested instruction files are supported where documented; nested MCP deployment is not offered. Preserve rule-file frontmatter and agent metadata. Include Antigravity native rule paths `~/.gemini/config/rules/*.md` and project `.agents/rules/*.md` in adapter fixtures rather than treating them as ordinary AGENTS files.

Capability fixtures cover scope, path, format, transport, reference handling, loading precedence, and unsupported cases. Installed-version gaps produce explanations and compatible alternatives. File presence and expected loading are distinct from proof that a running agent loaded a file; no AI session is launched to test this.

## 4. Architecture and storage

Reuse the Rust core, database, command layer, CLI, and host dispatch. Add focused Instructions and MCP modules rather than replacing skills with a general resource framework.

The existing Git library is `<base>/skills`; SQLite is its sibling `<base>/skills-manager.db`. Keep skills in their current layout. Proposed portable storage:

```text
<base>/skills/.agents-manager/
  schema.json
  instructions/<stable-id>/
    definition.json
    files/AGENTS.md
    files/docs/testing.md
  mcps/<stable-id>.json
```

Use random IDs and explicit revision identifiers, never new content hashes. Revision ancestry supports reconciliation; a display sequence is not a cross-device ordering guarantee. Instruction metadata records entry points, relative files, applicability, and revision. MCP metadata is a typed connection definition with non-secret defaults, external references, and catalog provenance, never a complete raw agent configuration.

Explicitly exclude the reserved namespace from **every** skill scan, including backup reconciliation. Existing reconciliation can detect a `SKILL.md` under hidden directories: merely hiding new files is insufficient. Extend backup, typed merge snapshots, validation, restore, and reindex to recognize the new resources.

Add host-local database records for deployment links, destinations, last-applied revision, safe scoped baselines, and overrides. They remain outside Git. Reuse the existing migration framework (baseline schema v11) for additive tables; retain skills and presets unchanged.

```text
Workspace + host
  → existing hostCall / hostScope
  → local core or remote CLI dispatch
  → that host's library, database and agent files
  → preview → explicit apply with fresh preconditions
```

Queries, previews, and mutations carry host/workspace identity. Host switching invalidates previews and prevents late responses or writes from targeting the newly selected host.

| Area | Existing integration seams |
|---|---|
| Views/navigation | `src/router.tsx`, `src/components/Sidebar.tsx`, `src/views/MySkills.tsx`, project detail and workspace views |
| Host calls | `src/lib/hostCall.ts`, `src/lib/hostScope.ts`, host-qualified query keys |
| Persistence | `core/central_repo.rs`, `core/skill_store.rs`, `core/migrations.rs` |
| Projects/agents | `commands/projects/`, `core/project_scanner.rs`, `core/tool_adapters.rs` |
| SSH | `core/host_dispatch.rs`, `remote_host.rs`, `remote_session.rs`, `remote_install.rs` |
| CLI | `src-tauri/src/bin/skills-manager-cli/main.rs`, `args.rs` and command handlers |
| Backup | `core/git_backup.rs`, `git_backup_store.rs`, `sync_metadata.rs`, `merge/` |

## 5. Instructions: discovery, editing, and reuse

Discover supported global, project, and nested files. Use bounded traversal with default exclusions for `.git`, dependencies, builds, and caches. Show exclusions, allow specific inclusions, and report depth/count/size limits as partial results rather than silently claiming completeness.

Maintain separate directory-scope and local-reference views. Resolve relative links against their source file; handle Markdown links and supported native references. Detect missing files and cycles, stop at visited files, and identify unsupported syntax. Do not fetch external links automatically. Imports that eagerly load content and links followed on demand have different labels.

Each reusable item is a whole file or relative-path bundle. References outside its root require explicit inclusion/remapping; reject traversal and unsafe destinations. Missing included bundle files block deployment; unrelated informational links do not force arbitrary file adoption.

Files can be edited in place or imported into the library. Editing does not silently adopt/link them. Show existing symlink targets; outside-boundary targets need a separately selected supported location. Do not create instruction deployment symlinks because library updates must remain reviewed snapshots.

Shared layouts use a common file only where loading rules support it. Agent-specific layouts preserve distinct native files. For Claude fallback precedence, preview a supported native import/file arrangement or explain the limitation; never silently change global settings to make a project layout work.

## 6. Deployment and update behavior

Instruction and MCP links mean **reviewed snapshots with a remembered base**. Compare last-applied content, current target, and new library revision. Reconcile instruction files/text with three-way comparison. Compare MCP typed fields and then apply explicit deployment overrides.

- Library-only changes become proposed updates; target-only changes remain intact.
- Non-overlapping changes can produce a proposed merged result; conflicts require an explicit resolution.
- Review deletions and their reference consequences.
- Deduplicate agent selections resolving to the same physical file; incompatible content for one target blocks apply.

Re-read targets and compare bytes against preview preconditions before writing; recheck path and symlink boundaries. Use manager locks and same-directory temporary replacements with preserved, restrictive permissions. External editors do not share locks, so do not claim transaction isolation from all other software.

Bundles are not atomic across files. Preflight, journal scoped reversible changes, apply per file, and advance deployment state only when complete. Recovery reports completed/pending operations; rollback touches only output that still matches this operation, preserving subsequent edits.

Do not persist entire MCP configuration preimages: unrelated secrets may be present. Full bytes exist only transiently for safe patching/precondition checks. Durable recovery contains safe managed-entry changes only. Replacing an existing secret-bearing entry is blocked until externalized through a supported mechanism; keep or rename remains available.

Library deletion never deletes deployed files. Show affected links and explicitly detach when deleting. Undeploy is separate and removes only unchanged managed content; edited targets remain for review.

## 7. MCP creation, import, and catalog

The portable model supports stdio commands/arguments and remote transports supported by each agent, defaults, and deployment overrides. Emit cwd, headers, environment fields and transports only where the adapter supports them; no implicit transport proxies or shell wrappers.

Import parses on the selected host and sanitizes before IPC, persistence, logs, previews, exports, or backups. Redact literal credential fields and require external references; preserve the original file. Never resolve a secret for serialization. Arbitrary shell fragments and ambiguous token-bearing URLs/arguments require manual reconstruction rather than blind copying into the library. Secret-detection heuristics are not a guarantee: use typed allowlisted import fields and reject unclassified sensitive values.

Use format-aware edits for JSON/JSONC, TOML, and YAML, preserving unrelated fields and comments. Block constructs that cannot safely round-trip rather than rewriting a complete configuration. Keep external agent authentication/trust settings unchanged.

Manual/import/catalog flows share validation and review. Collision options are Keep / Replace / Rename. Shared-default updates retain explicit deployment overrides and report unsupported overrides instead of dropping them.

The proposed first catalog is the official MCP Registry. Use unauthenticated `/v0.1/servers`, substring search, `version=latest`, cursor pagination and version details. Cache metadata, throttle requests, and keep manual/import usable during outages. Persist the chosen concrete version/provenance; do not make runtime commands silently follow catalog updates.

Registry metadata is not runnable configuration. Ask the user to select a transport/package and fill required inputs. Initially translate verified npm/stdio, PyPI/stdio, and remote endpoint metadata through tested mappings; unsupported package formats remain visible with documentation/manual setup. Preview exact commands and never execute them during catalog browsing or import. The agent may later run the configured launcher under its own policies; Agents Manager does not install the runtime.

Catalog requests execute on the active host through the existing host-call model; offline remote hosts still support manual/import. This preserves the agreed host-owned-library boundary without adding implicit desktop-to-remote transfers.

## 8. CLI, SSH, and backup

Keep `skills-manager-cli` and existing commands. Add `instructions` and `mcps` families for list/show, create/import/edit, validate, deploy/update preview and apply, detach/remove, and undeploy. Target flags identify an existing project or global agent scope. Add only the project lookup/selection support needed for parity; reuse current project records.

`--json` returns structured outcomes; `--dry-run` never writes. Noninteractive callers must supply conflict choices. Apply accepts a host-local preview identifier with stored safe proposed operations; sensitive full-config byte preconditions stay transient or must be freshly recomputed with scoped comparisons. Revalidate scope, target entry, unrelated-field preservation and library revision at apply; expired/stale previews require review again. Do not persist secret-bearing preview files for convenience.

Extend current remote allowlists and protocol handlers, retaining the exact-version handshake. Reuse system SSH authentication, installed remote CLI, and existing updater. Test disconnects without reporting uncertain writes as success. Remote macOS/Linux x64/arm64 remain supported; remote Windows remains unsupported.

Extend backup/export/import and two-device reconciliation to definitions and instruction payloads. Exclude machine deployments, overrides, project files, caches, journals and credentials. Restore library content only; never redeploy as a side effect. Keep skills-only presets unchanged.

Existing desktop backup orchestration is local-only: selecting an SSH host must not retarget local backup settings. Remote reusable definitions participate when backup runs on that machine through the existing CLI/installed app; the desktop should explain this boundary.

Version the resource schema and make new clients refuse unsupported future writes. **A new marker does not force old clients to comply.** Current schema validation parses markers but does not reliably enforce future schema/app versions. Test a pinned previous writer against a disposable expanded repo before shared sync ships. If old writers cannot safely preserve the new namespace, require coordinated upgrades of devices sharing the backup before enabling expanded sync; do not imply the new client can remotely enforce that upgrade. Document mixed-version sync as unsupported and stop new-client sync on detected incompatible writes. A compatibility release may be needed before feature release if tests establish that requirement.

## 9. Ordered delivery slices

All slices are release requirements, not reductions to the agreed scope.

| Slice | Deliverable and completion condition |
|---|---|
| 1. Contracts/compatibility | Five capability fixtures, schemas, additive deployment records, reserved namespace exclusions, and tested backup compatibility strategy. |
| 2. Instructions end to end | Bounded discovery, separate trees, editing, bundles, reviewed updates/recovery, CLI and SSH dispatch. |
| 3. MCPs end to end | Typed definitions, sanitized import, five adapters, overrides, official catalog, conflict flow, CLI and SSH. |
| 4. Product integration | Visible rebrand, tabs in existing workspaces, host-safe state and failure handling; skills/presets preserved. |
| 5. Backup/release verification | Portable round trips, two-device conflicts, compatibility safeguards, recovery, docs, changeset and required checks. |

Before implementation, incorporate latest main in this branch without replacing unrelated work. If conflicts occur, stop and raise them. Preserve `.serena/`. Use Luna for bounded implementation work, with explicit owned files and behavior checks; parent integrates. Escalate repeated unresolved issues to Sol Ultra, and use Astra for difficult planning decisions as requested.

## 10. Acceptance and validation

- Existing skills, projects, presets, host switching, and CLI workflows remain usable.
- All three workspace contexts expose resource tabs with correct host/scope identity.
- Instruction scopes/references, nested exclusions, missing links, cycles and symlinks are accurately represented.
- Saving library content leaves deployments untouched. Reviewed updates preserve local edits, overrides, unrelated config and comments.
- Concurrent edits invalidate previews; shared targets are written once; interrupted bundles expose recovery.
- Credential fixtures never appear in IPC, portable files, database baselines, logs, previews, exports or backups.
- Every supported adapter scope/format has fixtures; unsupported scopes and secret syntax are visibly unavailable.
- Catalog outages preserve manual/import; discovery executes no server commands.
- Dry runs are read-only; GUI/CLI/SSH resource results agree.
- Backup/restore and two-device conflicts retain resources without syncing deployments or dropping unknown supported data.

Use focused Rust temporary-directory fixtures, fault injection and CLI integration tests. Frontend tests exercise host-qualified state and decisions. E2E covers library → review → apply → reload, conflicts, adapter limitations and host switching. Use disposable SSH fixtures for live verification; report mocks and live results separately. Do not change real user projects or credentials for tests.

Required checks verified from the current manifest and CI:

```sh
pnpm lint
pnpm build
pnpm test
pnpm e2e
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Use the pinned package manager and frozen install when dependencies are absent. Report baseline failures, untested environments and remote CI separately from local passes. Native Windows and Linux checks require their corresponding CI/platform environments; local macOS alone does not establish all-platform success.

## 11. Risks and rollback

Primary risks: destructive config rewrites, credentials entering portable definitions, stale-host writes, agent loading differences, and old backup writers mishandling new types. Tests and explicit capability limits above target these risks.

Revert code without renaming existing data or moving skills. Preserve additive files/records; do not let older binaries write incompatible expanded backups. A reviewed deployment inverse only restores output still matching that operation. Keep later edits and unresolved recovery state. Never roll back by broad Git reset or project directory replacement.

Approval authorizes the design and ordered implementation. It does not authorize publishing, modifying real credentials, enabling incompatible backup writers, or inventing agent support. Record the structured Plannotator decision in `review.json`; do not begin implementation before approval.

## Sources and evidence

Repository baseline: `origin/main` `081eed8`, inspected read-only on 2026-09-27. Code evidence includes the integration paths above, `core/merge/protocol.rs`, `core/merge/validate.rs`, and `.github/workflows/{frontend,test}.yml`. No implementation tests were run for this planning task.

- [Progressive instruction organization](https://www.aihero.dev/a-complete-guide-to-agents-md)
- [Claude instructions](https://code.claude.com/docs/en/memory#agentsmd) and [MCP](https://code.claude.com/docs/en/mcp)
- [Codex instructions](https://learn.chatgpt.com/docs/agent-configuration/agents-md) and [MCP](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)
- [Antigravity rules](https://antigravity.google/docs/rules/) and [MCP](https://antigravity.google/docs/mcp)
- [Hermes context](https://hermes-agent.nousresearch.com/docs/user-guide/features/context-files), [MCP reference](https://hermes-agent.nousresearch.com/docs/reference/mcp-config-reference), and [configuration](https://hermes-agent.nousresearch.com/docs/user-guide/configuration)
- [Cursor rules](https://cursor.com/docs/rules) and [MCP](https://cursor.com/docs/mcp)
- [Official Registry API](https://github.com/modelcontextprotocol/registry/blob/main/docs/reference/api/official-registry-api.md) and [consumer guidance](https://modelcontextprotocol.io/registry/registry-aggregators)
