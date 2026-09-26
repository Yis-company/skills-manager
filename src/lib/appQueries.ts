import {
  queryOptions,
  type FetchQueryOptions,
  type QueryClient,
  type QueryKey,
} from "@tanstack/react-query";
import { invokeHost } from "./hostCall";
import type {
  ManagedSkill,
  Preset,
  Project,
  ProjectSkill,
  RemoteHost,
  ToolInfo,
} from "./tauri";

export interface PresetsData {
  presets: Preset[];
  activePreset: Preset | null;
}

export const queryKeys = {
  presets: (hostId: string | null) => ["host", hostId, "presets"] as const,
  tools: (hostId: string | null) => ["host", hostId, "tools"] as const,
  managedSkills: (hostId: string | null) => ["host", hostId, "managedSkills"] as const,
  projects: (hostId: string | null) => ["host", hostId, "projects"] as const,
  remoteHosts: () => ["remoteHosts"] as const,
  projectSkills: (hostId: string | null, projectId: string) =>
    ["host", hostId, "projectSkills", projectId] as const,
};

const desktopQueryDefaults = {
  retry: false,
  networkMode: "always" as const,
  staleTime: 0,
};

export const presetsQueryOptions = (hostId: string | null) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.presets(hostId),
    queryFn: async (): Promise<PresetsData> => {
      const [presets, activePreset] = await Promise.all([
        invokeHost<Preset[]>(hostId, "get_presets"),
        invokeHost<Preset | null>(hostId, "get_active_preset"),
      ]);
      return { presets, activePreset };
    },
  });

export const toolsQueryOptions = (hostId: string | null) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.tools(hostId),
    queryFn: () => invokeHost<ToolInfo[]>(hostId, "get_tool_status"),
  });

export const managedSkillsQueryOptions = (hostId: string | null) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.managedSkills(hostId),
    queryFn: () => invokeHost<ManagedSkill[]>(hostId, "get_managed_skills"),
  });

export const projectsQueryOptions = (hostId: string | null) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.projects(hostId),
    queryFn: () => invokeHost<Project[]>(hostId, "get_projects"),
  });

export const remoteHostsQueryOptions = () =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.remoteHosts(),
    queryFn: () => invokeHost<RemoteHost[]>(null, "remote_hosts_list"),
  });

export const projectSkillsQueryOptions = (hostId: string | null, projectId: string) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.projectSkills(hostId, projectId),
    queryFn: () => invokeHost<ProjectSkill[]>(hostId, "get_project_skills", { projectId }),
  });

interface RefreshState {
  promise: Promise<unknown>;
  followUp: boolean;
}

// Query identity avoids creating a second query-key hashing scheme. A refresh
// that arrives while a read is running asks the same runner for one trailing
// authoritative read after that request settles.
const refreshes = new WeakMap<object, RefreshState>();

export function refreshQuery<TData, TQueryKey extends QueryKey>(
  client: QueryClient,
  options: FetchQueryOptions<TData, Error, TData, TQueryKey>
): Promise<TData> {
  const query = client.getQueryCache().build(client, options);
  const current = refreshes.get(query);
  if (current) {
    if (client.isFetching({ queryKey: options.queryKey, exact: true }) > 0) {
      current.followUp = true;
    }
    return current.promise as Promise<TData>;
  }

  const state: RefreshState = { promise: Promise.resolve(undefined), followUp: false };
  refreshes.set(query, state);
  state.promise = (async () => {
    try {
      do {
        state.followUp = false;
        if (client.isFetching({ queryKey: options.queryKey, exact: true }) > 0) {
          await client.cancelQueries({ queryKey: options.queryKey, exact: true });
        }
        try {
          await client.fetchQuery(options);
        } catch (error) {
          if (!state.followUp) throw error;
        }
      } while (state.followUp);
      return query.state.data as TData;
    } finally {
      refreshes.delete(query);
    }
  })();
  return state.promise as Promise<TData>;
}
