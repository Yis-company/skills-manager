import {
  type FetchQueryOptions,
  type QueryClient,
  type QueryKey,
  queryOptions,
} from "@tanstack/react-query";

import { invokeHost } from "./hostCall";
import type { ManagedSkill, Preset, Project, ProjectSkill, RemoteHost, ToolInfo } from "./tauri";

export interface PresetsData {
  presets: Preset[];
  activePreset: null | Preset;
}

export const queryKeys = {
  presets: (hostId: null | string) => ["host", hostId, "presets"] as const,
  tools: (hostId: null | string) => ["host", hostId, "tools"] as const,
  managedSkills: (hostId: null | string) => ["host", hostId, "managedSkills"] as const,
  projects: (hostId: null | string) => ["host", hostId, "projects"] as const,
  remoteHosts: () => ["remoteHosts"] as const,
  projectSkills: (hostId: null | string, projectId: string) =>
    ["host", hostId, "projectSkills", projectId] as const,
};

const desktopQueryDefaults = {
  retry: false,
  networkMode: "always" as const,
  staleTime: 0,
};

export const presetsQueryOptions = (hostId: null | string) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.presets(hostId),
    queryFn: async (): Promise<PresetsData> => {
      const [presets, activePreset] = await Promise.all([
        invokeHost<Preset[]>(hostId, "get_presets"),
        invokeHost<null | Preset>(hostId, "get_active_preset"),
      ]);

      return { presets, activePreset };
    },
  });

export const toolsQueryOptions = (hostId: null | string) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.tools(hostId),
    queryFn: () => invokeHost<ToolInfo[]>(hostId, "get_tool_status"),
  });

export const managedSkillsQueryOptions = (hostId: null | string) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.managedSkills(hostId),
    queryFn: () => invokeHost<ManagedSkill[]>(hostId, "get_managed_skills"),
  });

export const projectsQueryOptions = (hostId: null | string) =>
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

export const projectSkillsQueryOptions = (hostId: null | string, projectId: string) =>
  queryOptions({
    ...desktopQueryDefaults,
    queryKey: queryKeys.projectSkills(hostId, projectId),
    queryFn: () => invokeHost<ProjectSkill[]>(hostId, "get_project_skills", { projectId }),
  });

interface RefreshState {
  /** Settles once the refresh, including any trailing read, is done. */
  done: Promise<void>;
  followUp: boolean;
}

// Query identity avoids creating a second query-key hashing scheme. A refresh
// that arrives while a read is running asks the same runner for one trailing
// authoritative read after that request settles.
const refreshes = new WeakMap<object, RefreshState>();

export async function refreshQuery<TData, TQueryKey extends QueryKey>(
  client: QueryClient,
  options: FetchQueryOptions<TData, Error, TData, TQueryKey>,
): Promise<TData> {
  const query = client.getQueryCache().build(client, options);
  const current = refreshes.get(query);

  if (current) {
    if (client.isFetching({ queryKey: options.queryKey, exact: true }) > 0) {
      current.followUp = true;
    }

    await current.done;
  } else {
    const state: RefreshState = { done: Promise.resolve(), followUp: false };
    refreshes.set(query, state);
    state.done = (async () => {
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
      } finally {
        refreshes.delete(query);
      }
    })();
    await state.done;
  }

  // The refresh left its authoritative result in the cache; read it back.
  return client.ensureQueryData(options);
}
