import type { InvokeArgs } from "@tauri-apps/api/core";

import type {
  AppUpdateInfo,
  BatchImportResult,
  GitBackupSizeReport,
  GithubDevicePollResult,
  Preset,
  ProjectSkill,
} from "../../src/lib/tauri";
import { projectGitHandler } from "./projectGit";
import {
  createState,
  type InstructionChange,
  type McpDefinition,
  type McpOperation,
  type McpServerDefinition,
  type McpTarget,
  type RemoteSeed,
  type State,
} from "./state";

/** A command's response: any value IPC can carry back to the app. */
type CommandResult = boolean | null | number | object | string;

/**
 * A fake command: reads and changes `state`, returns what the real command
 * would. Throwing rejects the invoke, like a backend error.
 */
type Handler<A> = (args: A, state: State) => CommandResult;

type InstructionTarget = {
  agent_key: string;
  project_id?: string;
  worktree?: string;
  relative_dir?: string;
};

/** What the app's `instructions_request` sends, for the actions the specs reach. */
type InstructionsRequest =
  | { action: "apply"; preview_id: string }
  | { action: "deployments" | "list" | "recover" | "worktrees" }
  | { action: "get" | "remove"; id: string }
  | { action: "preview"; target: InstructionTarget; instruction_id: string }
  | { action: "read"; target: InstructionTarget; path: string }
  | {
      action: "save";
      id?: string;
      name: string;
      description?: string;
      files: Record<string, string>;
      expected_revision?: string;
    }
  | { action: "scan"; target: InstructionTarget; include_dirs?: string[] }
  | {
      action: "write";
      target: InstructionTarget;
      path: string;
      content: string;
      expected_revision: string;
    };

/** What the app's `mcps_request` sends, for the actions the specs reach. */
type McpsRequest =
  | { action: "apply"; previewId: string }
  | { action: "capabilities" | "deployments" | "list" }
  | { action: "catalog"; serverId?: string; query?: string; cursor?: string }
  | { action: "get" | "remove"; id: string }
  | { action: "import"; target: McpTarget; name: string; conflict: string; newName?: string }
  | { action: "inspect"; target: McpTarget }
  | { action: "preview"; target: McpTarget; operations: McpOperation[] }
  | {
      action: "save";
      definition: { id?: string } & Omit<McpDefinition, "id" | "revision" | "updatedAt">;
      expectedRevision?: string;
    }
  | { action: "undeploy"; target: McpTarget; definitionId: string };

const nothing = () => null;

function presetsWithCounts(state: State): Preset[] {
  return [...state.presets]
    .sort((a, b) => a.sort_order - b.sort_order)
    .map((p) => ({
      ...p,
      skill_count: state.skills.filter((s) => s.preset_ids.includes(p.id)).length,
    }));
}

function findProjectSkill(
  state: State,
  projectId: string,
  relativePath: string,
  agent: string,
): ProjectSkill {
  const found = state.projectSkills[projectId]?.find(
    (s) => s.relative_path === relativePath && s.agent === agent,
  );

  if (!found) throw new Error(`fake backend: no project skill ${relativePath} for ${agent}`);

  return found;
}

function markInSync(state: State, { projectId, skillRelativePath, agent }: ProjectSkillArgs) {
  const found = findProjectSkill(state, projectId, skillRelativePath, agent);
  found.sync_status = "in_sync";
  found.in_center = true;
}

type ProjectSkillArgs = {
  projectId: string;
  skillRelativePath: string;
  agent: string;
};

// Only the commands the specs reach. Add one when a spec needs it; an unknown
// command fails the test with "fake backend: no handler for <cmd>".
export const handlers = {
  project_git_request: projectGitHandler,
  // ── App shell ──
  log_startup_event: nothing,
  remote_host_disconnect: nothing,
  remote_hosts_list: (_args, state) => state.remoteHosts,
  remote_host_connect: ({ hostId }: { hostId: string }, state) => {
    if (state.remoteCliNeedsUpdate) {
      throw {
        kind: "remote_version_mismatch",
        message: "Remote CLI version does not match",
      };
    }

    return {
      host_id: hostId,
      version: "1.40.0",
      os: "linux",
      arch: "x86_64",
      home: "/home/e2e",
      base_dir: "/home/e2e/.skills-manager",
    };
  },
  remote_host_probe: (_args, state) => ({
    version: state.remoteCliNeedsUpdate ? "1.39.0" : "1.40.0",
    compatible: !state.remoteCliNeedsUpdate,
    app_version: "1.40.0",
  }),
  remote_host_install_cli: ({ hostId }: { hostId: string }, state) => {
    state.remoteCliNeedsUpdate = false;

    return state.remoteHosts.find((host) => host.id === hostId) ?? null;
  },
  remote_invoke: (
    { hostId, command, args }: { hostId: string; command: string; args?: InvokeArgs },
    state,
  ) => {
    const remoteState = createState(state.remoteStates[hostId] ?? {});

    const handler = findHandler(command);

    if (!handler) throw new Error(`fake backend: no remote handler for ${command}`);
    const result = handler(args ?? {}, remoteState);

    const remoteSeed: RemoteSeed = {
      skills: remoteState.skills,
      globalLocalSkills: remoteState.globalLocalSkills,
      presets: remoteState.presets,
      activePresetId: remoteState.activePresetId,
      presetSkillOrder: remoteState.presetSkillOrder,
      projects: remoteState.projects,
      projectSkills: remoteState.projectSkills,
      projectGit: remoteState.projectGit,
      projectAgentTargets: remoteState.projectAgentTargets,
      tools: remoteState.tools,
      settings: remoteState.settings,
      instructionBundles: remoteState.instructionBundles,
      instructionFiles: remoteState.instructionFiles,
      instructionWorktrees: remoteState.instructionWorktrees,
      mcpDefinitions: remoteState.mcpDefinitions,
      mcpTargets: remoteState.mcpTargets,
      resourcePreviews: remoteState.resourcePreviews,
      failedInstallSkillIds: remoteState.failedInstallSkillIds,
    };

    state.remoteStates[hostId] = remoteSeed;

    return result;
  },
  instructions_request: ({ request }: { request: InstructionsRequest }, state) => {
    const action = request.action;

    if (request.action === "list")
      return {
        items: state.instructionBundles.map(({ files, ...item }) => ({
          ...item,
          files: Object.entries(files).map(([path, content]) => ({
            path,
            size: content.length,
          })),
        })),
      };

    if (request.action === "get") {
      const item = state.instructionBundles.find((entry) => entry.id === request.id);

      if (!item) throw new Error("instruction bundle not found");

      return { item };
    }

    if (request.action === "save") {
      const files = request.files;

      if (Object.keys(files).some((path) => path.startsWith("/") || path.split("/").includes("..")))
        throw new Error("unsafe file path");

      const id = String(request.id ?? `instruction-${state.instructionBundles.length + 1}`);

      const existing = state.instructionBundles.find((entry) => entry.id === id);

      if (existing && request.expected_revision !== existing.revision)
        throw new Error("instruction bundle changed; reload before saving");

      const item = {
        id,
        name: String(request.name),
        description: request.description,
        revision: String(Number(existing?.revision ?? "0") + 1),
        updated_at: "2026-09-27T00:00:00Z",
        files,
      };

      if (existing) Object.assign(existing, item);
      else state.instructionBundles.push(item);

      return { item };
    }

    if (request.action === "remove") {
      state.instructionBundles = state.instructionBundles.filter((item) => item.id !== request.id);

      return { removed: true };
    }

    if (request.action === "worktrees") return { items: state.instructionWorktrees };

    if (request.action === "scan")
      return {
        target: request.target,
        files: Object.entries(state.instructionFiles).map(([path, file]) => ({
          path,
          exists: true,
          content: file.content,
          revision: file.revision,
          managed: file.managed,
          kind: file.kind,
        })),
        references: [],
        excluded: [],
        warnings: [],
      };

    if (request.action === "read") {
      const entry = state.instructionFiles[String(request.path)];

      if (!entry) throw new Error("file not found");

      return {
        path: request.path,
        content: entry.content,
        revision: entry.revision,
      };
    }

    if (request.action === "write") {
      const entry = state.instructionFiles[String(request.path)];

      if (!entry || entry.revision !== request.expected_revision)
        throw new Error("file changed; refresh before saving");
      entry.content = String(request.content);
      entry.revision = String(Number(entry.revision) + 1);

      return { path: request.path, revision: entry.revision };
    }

    if (request.action === "preview") {
      const item = state.instructionBundles.find((entry) => entry.id === request.instruction_id);

      if (!item) throw new Error("instruction bundle not found");

      const changes = Object.entries(item.files).map(([path, content]) => {
        const local = state.instructionFiles[path];

        const change: InstructionChange = {
          path,
          status: local ? (local.content === content ? "unchanged" : "conflict") : "create",
          content,
        };

        if (local) {
          change.previous = local.content;
          change.conflict = "local file differs from deployed baseline and incoming version";
        }

        return change;
      });

      const preview_id = `instruction-preview-${Object.keys(state.resourcePreviews).length + 1}`;
      state.resourcePreviews[preview_id] = {
        kind: "instructions",
        payload: changes,
      };

      return {
        preview_id,
        instruction_id: item.id,
        target: request.target,
        changes,
        warnings: [],
      };
    }

    if (request.action === "apply") {
      const preview = state.resourcePreviews[String(request.preview_id)];

      if (!preview || preview.kind !== "instructions") throw new Error("preview expired");

      const changes = preview.payload;

      const applied: string[] = [];

      for (const change of changes) {
        if (change.status === "unchanged") continue;
        state.instructionFiles[change.path] = {
          content: change.content,
          revision: "1",
          managed: true,
          kind: change.path.includes("/") ? "nested" : "root",
        };
        applied.push(change.path);
      }

      delete state.resourcePreviews[String(request.preview_id)];

      return {
        applied,
        failed: [],
        partial: false,
        transaction_id: "fake-transaction",
      };
    }

    if (request.action === "deployments" || request.action === "recover") return { items: [] };
    throw new Error(`unknown instructions action: ${String(action)}`);
  },
  mcps_request: ({ request }: { request: McpsRequest }, state) => {
    const action = request.action;

    if (request.action === "list") return { definitions: state.mcpDefinitions };

    if (request.action === "capabilities")
      return {
        targets: state.tools.map((tool) => ({
          agentKey: tool.key,
          scopes: [
            { kind: "global", supported: true },
            { kind: "project", supported: tool.key !== "hermes" },
          ],
          features: ["stdio", "http", "sse"],
          limitations: [],
        })),
      };

    if (request.action === "inspect") {
      const target = request.target;
      const key = `${target.projectId ?? "global"}:${target.agentKey}`;

      return { entries: state.mcpTargets[key] ?? [] };
    }

    if (request.action === "import") {
      const target = request.target;
      const key = `${target.projectId ?? "global"}:${target.agentKey}`;

      const entry = (state.mcpTargets[key] ?? []).find((item) => item.name === request.name);

      if (!entry?.definition) throw new Error("Native definition unavailable");

      const existing = state.mcpDefinitions.find((item) => item.name === request.name);

      if (existing && request.conflict === "keep") return { definition: existing };

      const draft: { id?: string; name?: string } & Omit<McpServerDefinition, "name"> = {
        ...entry.definition,
      };

      if (existing && request.conflict === "replace") draft.id = existing.id;

      if (request.conflict === "rename") draft.name = request.newName;

      return { draft, expectedRevision: existing?.revision };
    }

    if (request.action === "catalog")
      return request.serverId
        ? { server: { name: String(request.serverId) } }
        : { servers: [], nextCursor: null };

    if (request.action === "get") {
      const definition = state.mcpDefinitions.find((item) => item.id === request.id);

      if (!definition) throw new Error("MCP definition not found");

      return { definition };
    }

    if (request.action === "save") {
      const input = request.definition;
      const id = String(input.id ?? `mcp-${state.mcpDefinitions.length + 1}`);
      const existing = state.mcpDefinitions.find((item) => item.id === id);

      if (existing && request.expectedRevision !== existing.revision)
        throw new Error("MCP definition changed; reload before saving");

      const definition: McpDefinition = {
        ...input,
        id,
        revision: String(Number(existing?.revision ?? "0") + 1),
        updatedAt: "2026-09-27T00:00:00Z",
      };

      if (existing) Object.assign(existing, definition);
      else state.mcpDefinitions.push(definition);

      return { definition };
    }

    if (request.action === "remove") {
      state.mcpDefinitions = state.mcpDefinitions.filter((item) => item.id !== request.id);

      return { removed: true };
    }

    if (request.action === "preview") {
      const operations = request.operations;

      const changes = operations.map((operation) => ({
        name:
          state.mcpDefinitions.find((item) => item.id === operation.definitionId)?.name ??
          operation.definitionId,
        kind: operation.kind,
        warnings: [],
      }));

      const previewId = `mcp-preview-${Object.keys(state.resourcePreviews).length + 1}`;
      state.resourcePreviews[previewId] = {
        kind: "mcps",
        payload: { target: request.target, operations },
      };

      return { previewId, changes };
    }

    if (request.action === "apply") {
      const preview = state.resourcePreviews[String(request.previewId)];

      if (!preview || preview.kind !== "mcps") throw new Error("preview expired");

      const payload = preview.payload;

      const key = `${payload.target.projectId ?? "global"}:${payload.target.agentKey}`;
      const entries = state.mcpTargets[key] ?? [];

      for (const operation of payload.operations) {
        const definition = state.mcpDefinitions.find((item) => item.id === operation.definitionId);

        if (operation.kind === "undeploy") {
          state.mcpTargets[key] = entries.filter(
            (item) => item.managedId !== operation.definitionId,
          );
          continue;
        }

        if (
          definition &&
          !entries.some((item) => item.name === (operation.newName ?? definition.name))
        )
          entries.push({
            name: operation.newName ?? definition.name,
            managedId: definition.id,
            status: "managed",
          });
      }

      state.mcpTargets[key] = entries;
      delete state.resourcePreviews[String(request.previewId)];

      return { applied: true, changes: entries };
    }

    if (request.action === "deployments") return { deployments: [] };

    if (request.action === "undeploy") {
      const target = request.target;
      const key = `${target.projectId ?? "global"}:${target.agentKey}`;
      const before = state.mcpTargets[key] ?? [];
      state.mcpTargets[key] = before.filter((item) => item.managedId !== request.definitionId);

      return {
        removed: before.length !== state.mcpTargets[key].length,
        preservedEdited: false,
      };
    }

    throw new Error(`unknown MCP action: ${String(action)}`);
  },
  resource_sync_request: () => ({ conflicts: [] }),
  check_app_update: (): AppUpdateInfo => ({
    has_update: false,
    current_version: "1.40.0",
    latest_version: "1.40.0",
    release_url: "",
  }),
  "plugin:opener|open_url": nothing,
  "plugin:dialog|open": (_args, state) => state.dialogPaths.shift() ?? null,

  // ── Settings ──
  get_settings: ({ key }: { key: string }, state) => state.settings[key] ?? null,
  set_settings: ({ key, value }: { key: string; value: string }, state) => {
    state.settings[key] = value;

    return null;
  },
  get_central_repo_path: (_args, state) => state.centralRepoPath,
  get_central_repo_path_override: (_args, state) => state.centralRepoPathOverride,
  get_central_repo_pending_path: (_args, state) => state.centralRepoPendingPath,
  get_central_repo_warnings: () => [],
  check_last_panic: () => null,
  set_central_repo_path: ({ path }: { path: null | string }, state) => {
    const nextPath = path ?? "/home/e2e/.skills-manager";
    state.centralRepoPathOverride = path;
    state.centralRepoPendingPath = nextPath === state.centralRepoPath ? null : nextPath;

    return nextPath;
  },

  // ── Tools ──
  get_tool_status: (_args, state) => state.tools,
  get_global_local_skills: ({ agent }: { agent: string }, state) =>
    state.globalLocalSkills[agent] ?? [],
  unsync_skill_from_tool: ({ skillId, tool }: { skillId: string; tool: string }, state) => {
    const skill = state.skills.find((item) => item.id === skillId);

    if (skill) skill.targets = skill.targets.filter((target) => target.tool !== tool);
    state.globalLocalSkills[tool] = (state.globalLocalSkills[tool] ?? []).filter(
      (localSkill) => localSkill.center_skill_id !== skillId,
    );

    return null;
  },
  delete_global_local_skill: (
    { agent, skillRelativePath }: { agent: string; skillRelativePath: string },
    state,
  ) => {
    state.globalLocalSkills[agent] = (state.globalLocalSkills[agent] ?? []).filter(
      (skill) => skill.relative_path !== skillRelativePath,
    );

    return null;
  },

  // ── Skills and tags ──
  get_managed_skills: (_args, state) => state.skills,
  install_from_skillssh: ({ source, skillId }: { source: string; skillId: string }, state) => {
    if (state.failedInstallSkillIds.includes(skillId)) {
      throw new Error(`fake install failure for ${skillId}`);
    }

    const sourceSkill = state.market.find(
      (skill) => skill.source === source && skill.skill_id === skillId,
    );

    const sourceRef = `${source}/${skillId}`;

    if (!state.skills.some((skill) => skill.source_ref === sourceRef)) {
      const now = Date.now();
      const name = sourceSkill?.name || skillId;
      state.skills.push({
        id: `market:${sourceRef}`,
        name,
        description: null,
        author: null,
        source_type: "skillssh",
        source_ref: sourceRef,
        source_ref_resolved: null,
        source_subpath: null,
        source_branch: null,
        source_revision: null,
        remote_revision: null,
        update_status: "up_to_date",
        last_checked_at: now,
        last_check_error: null,
        central_path: `/home/e2e/.skills-manager/skills/${name}`,
        enabled: true,
        created_at: now,
        updated_at: now,
        status: "available",
        targets: [],
        preset_ids: [],
        tags: [],
      });
    }

    return null;
  },
  cancel_install: () => true,
  delete_managed_skill: ({ skillId }: { skillId: string }, state) => {
    state.skills = state.skills.filter((skill) => skill.id !== skillId);

    return null;
  },
  delete_managed_skills: ({ skillIds }: { skillIds: string[] }, state) => {
    const failed = state.deleteFailedIds.filter((id) => skillIds.includes(id));
    const deletedIds = skillIds.filter((id) => !failed.includes(id));
    state.skills = state.skills.filter((skill) => !deletedIds.includes(skill.id));

    if (state.rejectBatchDeleteAfterPartialWrite)
      throw new Error("fake batch delete failed after partial write");

    return { deleted: deletedIds.length, failed };
  },
  check_all_skill_updates: nothing,
  get_all_tags: (_args, state) => [...new Set(state.skills.flatMap((s) => s.tags))].sort(),
  rename_tag: ({ oldName, newName }: { oldName: string; newName: string }, state) => {
    for (const s of state.skills) {
      s.tags = [...new Set(s.tags.map((tag) => (tag === oldName ? newName : tag)))];
    }

    return null;
  },
  delete_tag: ({ name }: { name: string }, state) => {
    for (const s of state.skills) s.tags = s.tags.filter((tag) => tag !== name);

    return null;
  },
  get_skill_document: ({ skillId }: { skillId: string }, state) => {
    const found = state.skills.find((s) => s.id === skillId);

    return {
      skill_id: skillId,
      filename: "SKILL.md",
      content: `# ${found?.name}\n\nLibrary copy.`,
      central_path: found?.central_path ?? "",
    };
  },

  // ── Presets ──
  get_presets: (_args, state) => presetsWithCounts(state),
  get_active_preset: (_args, state) =>
    presetsWithCounts(state).find((p) => p.id === state.activePresetId) ?? null,
  reorder_presets: ({ ids }: { ids: string[] }, state) => {
    for (const p of state.presets) p.sort_order = ids.indexOf(p.id);

    return null;
  },
  get_preset_skill_order: ({ presetId }: { presetId: string }, state) =>
    state.presetSkillOrder[presetId] ?? [],
  reorder_preset_skills: (
    { presetId, skillIds }: { presetId: string; skillIds: string[] },
    state,
  ) => {
    state.presetSkillOrder[presetId] = skillIds;

    return null;
  },
  add_skill_to_preset: ({ skillId, presetId }: { skillId: string; presetId: string }, state) => {
    const found = state.skills.find((s) => s.id === skillId);

    if (found && !found.preset_ids.includes(presetId)) found.preset_ids.push(presetId);

    return null;
  },
  remove_skill_from_preset: (
    { skillId, presetId }: { skillId: string; presetId: string },
    state,
  ) => {
    const found = state.skills.find((s) => s.id === skillId);

    if (found) found.preset_ids = found.preset_ids.filter((id) => id !== presetId);

    return null;
  },
  get_skill_tool_toggles: (_args, state) =>
    state.tools.map((t) => ({
      tool: t.key,
      display_name: t.display_name,
      installed: t.installed,
      globally_enabled: t.enabled,
      enabled: true,
    })),

  // ── Projects ──
  get_projects: (_args, state) => [...state.projects].sort((a, b) => a.sort_order - b.sort_order),
  reorder_projects: ({ ids }: { ids: string[] }, state) => {
    for (const p of state.projects) p.sort_order = ids.indexOf(p.id);

    return null;
  },
  get_project_skills: ({ projectId }: { projectId: string }, state) =>
    state.projectSkills[projectId] ?? [],
  delete_project_skill: (
    {
      projectId,
      skillRelativePath,
      agent,
      wholeSkill,
    }: {
      projectId: string;
      skillRelativePath: string;
      agent: string;
      wholeSkill?: boolean;
    },
    state,
  ) => {
    const skills = state.projectSkills[projectId] ?? [];

    const target = skills.find(
      (skill) => skill.relative_path === skillRelativePath && skill.agent === agent,
    );

    if (!target)
      throw new Error(`fake backend: no project skill ${skillRelativePath} for ${agent}`);

    const remaining = skills.filter((skill) => {
      if (skill.relative_path === skillRelativePath && skill.agent === agent) return false;

      return !(wholeSkill && target.vendored && skill.alias_of === skillRelativePath);
    });

    state.projectSkills[projectId] = remaining;

    return null;
  },
  preview_project_retired_links: () => [],
  apply_project_retired_links: () => [],
  get_project_agent_targets: ({ projectId }: { projectId: string }, state) =>
    state.projectAgentTargets[projectId] ?? [],
  get_project_skill_document: ({ skillRelativePath }: ProjectSkillArgs) => ({
    skill_name: skillRelativePath,
    filename: "SKILL.md",
    content: `# ${skillRelativePath}\n\nProject copy.`,
  }),
  slugify_skill_names: ({ names }: { names: string[] }) =>
    names.map((name) => name.toLowerCase().replace(/[^a-z0-9]+/g, "-")),
  update_project_skill_to_center: (args: ProjectSkillArgs, state) => {
    markInSync(state, args);

    return null;
  },
  update_project_skill_from_center: (args: ProjectSkillArgs, state) => {
    markInSync(state, args);

    return null;
  },

  // ── Install ──
  fetch_leaderboard: (_args, state) => state.market,
  search_skillssh: ({ query, limit }: { query: string; limit: null | number }, state) =>
    state.market
      .filter((s) => s.name.toLowerCase().includes(query.toLowerCase()))
      .slice(0, limit ?? undefined),
  scan_local_skills: (_args, state) => state.scan,
  batch_import_folder: (_args: { folderPath: string }, state): BatchImportResult => {
    const imported = state.batchImport.splice(0);
    state.skills.push(...imported);

    return { imported: imported.length, skipped: 0, errors: [] };
  },

  // ── Backup ──
  git_backup_status: (_args, state) => state.gitStatus,
  git_backup_fetch: nothing,
  git_backup_migrate_credentials: nothing,
  git_backup_pending_conflicts: () => [],
  git_backup_list_versions: () => [],
  git_backup_size_report: (): GitBackupSizeReport => ({
    total_bytes: 1024,
    oversized: [],
    skill_limit_bytes: 10_000_000,
    repo_warn_bytes: 100_000_000,
  }),
  git_backup_set_remote: ({ url }: { url: string }, state) => {
    state.gitStatus.remote_url = url;

    return url;
  },
  backup_device_name: () => "e2e-machine",
  github_device_flow_start: (_args, state) => state.deviceFlow.start,
  github_device_flow_poll: (_args, state): GithubDevicePollResult => {
    const status = state.deviceFlow.polls.shift() ?? "pending";

    return {
      status,
      result: status === "connected" ? state.deviceFlow.result : null,
    };
  },
} satisfies Record<string, Handler<never>>;

export type Command = keyof typeof handlers;

/** The args a command's handler declares, which the app sends with every invoke. */
export type CommandArgs<C extends Command> = Parameters<(typeof handlers)[C]>[0];

function isCommand(cmd: string): cmd is Command {
  return Object.hasOwn(handlers, cmd);
}

/** The handler of `cmd`, taking the raw invoke args; undefined when the fake has none. */
export function findHandler(cmd: string): Handler<InvokeArgs> | undefined {
  if (!isCommand(cmd)) return undefined;

  // SAFETY: the app invokes each command through its typed wrapper, with the
  // args the real command (and so this handler) declares.
  return handlers[cmd] as Handler<InvokeArgs>;
}
