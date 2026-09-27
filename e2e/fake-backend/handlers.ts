import type {
  AppUpdateInfo,
  BatchImportResult,
  GitBackupSizeReport,
  GithubDevicePollResult,
  Preset,
  ProjectSkill,
} from "../../src/lib/tauri";
import { createState, type RemoteSeed, type State } from "./state";

/**
 * A fake command: reads and changes `state`, returns what the real command
 * would. Throwing rejects the invoke, like a backend error.
 */
type Handler<A> = (args: A, state: State) => unknown;

const nothing = () => null;

function presetsWithCounts(state: State): Preset[] {
  return [...state.presets]
    .sort((a, b) => a.sort_order - b.sort_order)
    .map((p) => ({ ...p, skill_count: state.skills.filter((s) => s.preset_ids.includes(p.id)).length }));
}

function findProjectSkill(state: State, projectId: string, relativePath: string, agent: string): ProjectSkill {
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

type ProjectSkillArgs = { projectId: string; skillRelativePath: string; agent: string };

// Only the commands the specs reach. Add one when a spec needs it; an unknown
// command fails the test with "fake backend: no handler for <cmd>".
export const handlers: Record<string, Handler<never>> = {
  // ── App shell ──
  log_startup_event: nothing,
  remote_host_disconnect: nothing,
  remote_hosts_list: (_args: unknown, state) => state.remoteHosts,
  remote_host_connect: ({ hostId }: { hostId: string }, state) => {
    if (state.remoteCliNeedsUpdate) {
      throw { kind: "remote_version_mismatch", message: "Remote CLI version does not match" };
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
  remote_host_probe: (_args: unknown, state) => ({
    version: state.remoteCliNeedsUpdate ? "1.39.0" : "1.40.0",
    compatible: !state.remoteCliNeedsUpdate,
    app_version: "1.40.0",
  }),
  remote_host_install_cli: ({ hostId }: { hostId: string }, state) => {
    state.remoteCliNeedsUpdate = false;
    return state.remoteHosts.find((host) => host.id === hostId) ?? null;
  },
  remote_invoke: ({ hostId, command, args }: { hostId: string; command: string; args: unknown }, state) => {
    const remoteState = createState(state.remoteStates[hostId] ?? {});
    const handler = handlers[command] as ((args: unknown, state: State) => unknown) | undefined;
    if (!handler) throw new Error(`fake backend: no remote handler for ${command}`);
    const result = handler(args ?? {}, remoteState);
    const remoteSeed: RemoteSeed = {
      skills: remoteState.skills,
      presets: remoteState.presets,
      activePresetId: remoteState.activePresetId,
      presetSkillOrder: remoteState.presetSkillOrder,
      projects: remoteState.projects,
      projectSkills: remoteState.projectSkills,
      projectAgentTargets: remoteState.projectAgentTargets,
      tools: remoteState.tools,
      settings: remoteState.settings,
    };
    state.remoteStates[hostId] = remoteSeed;
    return result;
  },
  check_app_update: (): AppUpdateInfo => ({
    has_update: false,
    current_version: "1.40.0",
    latest_version: "1.40.0",
    release_url: "",
  }),
  "plugin:opener|open_url": nothing,
  "plugin:dialog|open": (_args: unknown, state) => state.dialogPaths.shift() ?? null,

  // ── Settings ──
  get_settings: ({ key }: { key: string }, state) => state.settings[key] ?? null,
  set_settings: ({ key, value }: { key: string; value: string }, state) => {
    state.settings[key] = value;
    return null;
  },
  get_central_repo_path: (_args: unknown, state) => state.centralRepoPath,
  get_central_repo_path_override: (_args: unknown, state) => state.centralRepoPathOverride,
  get_central_repo_pending_path: (_args: unknown, state) => state.centralRepoPendingPath,
  get_central_repo_warnings: () => [],
  check_last_panic: () => null,
  set_central_repo_path: ({ path }: { path: string | null }, state) => {
    const nextPath = path ?? "/home/e2e/.skills-manager";
    state.centralRepoPathOverride = path;
    state.centralRepoPendingPath = nextPath === state.centralRepoPath ? null : nextPath;
    return nextPath;
  },

  // ── Tools ──
  get_tool_status: (_args: unknown, state) => state.tools,

  // ── Skills and tags ──
  get_managed_skills: (_args: unknown, state) => state.skills,
  delete_managed_skill: ({ skillId }: { skillId: string }, state) => {
    state.skills = state.skills.filter((skill) => skill.id !== skillId);
    return null;
  },
  delete_managed_skills: ({ skillIds }: { skillIds: string[] }, state) => {
    const failed = state.deleteFailedIds.filter((id) => skillIds.includes(id));
    const deletedIds = skillIds.filter((id) => !failed.includes(id));
    state.skills = state.skills.filter((skill) => !deletedIds.includes(skill.id));
    if (state.rejectBatchDeleteAfterPartialWrite) throw new Error("fake batch delete failed after partial write");
    return { deleted: deletedIds.length, failed };
  },
  check_all_skill_updates: nothing,
  get_all_tags: (_args: unknown, state) => [...new Set(state.skills.flatMap((s) => s.tags))].sort(),
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
    return { skill_id: skillId, filename: "SKILL.md", content: `# ${found?.name}\n\nLibrary copy.`, central_path: found?.central_path ?? "" };
  },

  // ── Presets ──
  get_presets: (_args: unknown, state) => presetsWithCounts(state),
  get_active_preset: (_args: unknown, state) =>
    presetsWithCounts(state).find((p) => p.id === state.activePresetId) ?? null,
  reorder_presets: ({ ids }: { ids: string[] }, state) => {
    for (const p of state.presets) p.sort_order = ids.indexOf(p.id);
    return null;
  },
  get_preset_skill_order: ({ presetId }: { presetId: string }, state) => state.presetSkillOrder[presetId] ?? [],
  reorder_preset_skills: ({ presetId, skillIds }: { presetId: string; skillIds: string[] }, state) => {
    state.presetSkillOrder[presetId] = skillIds;
    return null;
  },

  // ── Projects ──
  get_projects: (_args: unknown, state) => [...state.projects].sort((a, b) => a.sort_order - b.sort_order),
  reorder_projects: ({ ids }: { ids: string[] }, state) => {
    for (const p of state.projects) p.sort_order = ids.indexOf(p.id);
    return null;
  },
  get_project_skills: ({ projectId }: { projectId: string }, state) => state.projectSkills[projectId] ?? [],
  delete_project_skill: ({ projectId, skillRelativePath, agent, wholeSkill }: { projectId: string; skillRelativePath: string; agent: string; wholeSkill?: boolean }, state) => {
    const skills = state.projectSkills[projectId] ?? [];
    const target = skills.find((skill) => skill.relative_path === skillRelativePath && skill.agent === agent);
    if (!target) throw new Error(`fake backend: no project skill ${skillRelativePath} for ${agent}`);
    const remaining = skills.filter((skill) => {
      if (skill.relative_path === skillRelativePath && skill.agent === agent) return false;
      return !(wholeSkill && target.vendored && skill.alias_of === skillRelativePath);
    });
    state.projectSkills[projectId] = remaining;
    return null;
  },
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
  fetch_leaderboard: (_args: unknown, state) => state.market,
  search_skillssh: ({ query, limit }: { query: string; limit: number | null }, state) =>
    state.market
      .filter((s) => s.name.toLowerCase().includes(query.toLowerCase()))
      .slice(0, limit ?? undefined),
  scan_local_skills: (_args: unknown, state) => state.scan,
  batch_import_folder: (_args: { folderPath: string }, state): BatchImportResult => {
    const imported = state.batchImport.splice(0);
    state.skills.push(...imported);
    return { imported: imported.length, skipped: 0, errors: [] };
  },

  // ── Backup ──
  git_backup_status: (_args: unknown, state) => state.gitStatus,
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
  github_device_flow_start: (_args: unknown, state) => state.deviceFlow.start,
  github_device_flow_poll: (_args: unknown, state): GithubDevicePollResult => {
    const status = state.deviceFlow.polls.shift() ?? "pending";
    return { status, result: status === "connected" ? state.deviceFlow.result : null };
  },
};
