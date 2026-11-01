/* eslint-disable react-refresh/only-export-components */
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";

import {
  managedSkillsQueryOptions,
  projectSkillsQueryOptions,
  queryKeys,
  refreshQuery,
} from "../lib/appQueries";
import { invokeHost } from "../lib/hostCall";
import {
  type InstallSource,
  isLibraryOperation,
  mergeOperations,
  operationIdentity,
  operationNeedsSession,
  operationQueryKey,
  type OperationRequest,
  type OperationSnapshot,
} from "../lib/operations";
import { groupProjectSkills } from "../lib/projectSkillGroups";
import { type Project, remoteHostRelease, remoteHostRetained } from "../lib/tauri";

interface OperationsState {
  operations: OperationSnapshot[];
  allOperations: OperationSnapshot[];
  unavailableHosts: (null | string)[];
  refreshErrors: Record<string, string>;
  install: (sources: InstallSource[], origin?: null | string) => Promise<void>;
  submit: (projectId: string, paths?: string[]) => Promise<void>;
  act: (origin: null | string, request: OperationRequest) => Promise<void>;
  refresh: (operation: OperationSnapshot) => Promise<void>;
}

const Context = createContext<null | OperationsState>(null);

const EMPTY_OPERATIONS: OperationSnapshot[] = [];

const key = operationQueryKey;

/** Owns observation for the app session; views never run item execution loops. */
export function OperationsProvider({
  hostId,
  children,
}: {
  hostId: null | string;
  children: ReactNode;
}) {
  const client = useQueryClient();
  const [refreshErrors, setRefreshErrors] = useState<Record<string, string>>({});
  const refreshed = useRef(new Set<string>());
  const refreshing = useRef(new Map<null | string, number>());
  const refreshFailures = useRef(new Map<string, OperationSnapshot>());
  const submitting = useRef(new Map<string, Promise<void>>());

  const [observedHosts, setObservedHosts] = useState<(null | string)[]>([null]);
  const pending = useRef(new Map<null | string, number>());

  const observe = useCallback((origin: null | string) => {
    setObservedHosts((hosts) => (hosts.includes(origin) ? hosts : [...hosts, origin]));
  }, []);

  if (!observedHosts.includes(hostId)) setObservedHosts([...observedHosts, hostId]);
  useEffect(() => {
    void remoteHostRetained()
      .then((hosts) => hosts.forEach(observe))
      .catch(console.error);
  }, [observe]);

  const hosts = observedHosts;

  const queries = useQueries({
    queries: hosts.map((origin) => ({
      queryKey: key(origin),
      queryFn: async () => {
        const snapshots = await invokeHost<OperationSnapshot[]>(origin, "operation_request", {
          request: { action: "list" },
        });

        return mergeOperations(
          client.getQueryData<OperationSnapshot[]>(key(origin)) ?? [],
          snapshots,
          origin,
        );
      },
      enabled: () =>
        client.getQueryData(key(origin)) === undefined ||
        origin === hostId ||
        (pending.current.get(origin) ?? 0) > 0 ||
        (client.getQueryData<OperationSnapshot[]>(key(origin)) ?? []).some(operationNeedsSession),
      refetchInterval: 1000,
      refetchIntervalInBackground: true,
      retry: false,
    })),
  });

  const unavailableHosts = hosts.filter((_, index) => queries[index].isError);
  const allOperations = queries.flatMap((query) => query.data ?? EMPTY_OPERATIONS);
  const operations = allOperations.filter((operation) => operation.host_id === hostId);

  const releaseIfIdle = useCallback(
    (origin: null | string) => {
      const records = client.getQueryData<OperationSnapshot[]>(key(origin)) ?? [];

      if (
        origin !== null &&
        (refreshing.current.get(origin) ?? 0) === 0 &&
        (pending.current.get(origin) ?? 0) === 0 &&
        ![...refreshFailures.current.values()].some((operation) => operation.host_id === origin) &&
        records.length > 0 &&
        records.every(
          (operation) =>
            !operationNeedsSession(operation) &&
            refreshed.current.has(operationIdentity(operation)),
        )
      ) {
        void remoteHostRelease(origin).catch(console.error);
      }
    },
    [client],
  );

  const refresh = useCallback(
    async (operation: OperationSnapshot) => {
      const origin = operation.host_id;
      const identity = operationIdentity(operation);
      refreshing.current.set(origin, (refreshing.current.get(origin) ?? 0) + 1);

      try {
        if (isLibraryOperation(operation)) {
          await refreshQuery(client, managedSkillsQueryOptions(origin));
        } else if (operation.project_id) {
          const skills = await refreshQuery(
            client,
            projectSkillsQueryOptions(origin, operation.project_id),
          );

          // Refresh only the changed project's health, without rescanning every project.
          const groups = groupProjectSkills(skills);
          client.setQueryData<Project[]>(queryKeys.projects(origin), (current) =>
            current?.map((project) =>
              project.id !== operation.project_id
                ? project
                : {
                    ...project,
                    skill_count: groups.length,
                    sync_health: {
                      in_sync: groups.filter((g) => g.status === "in_sync").length,
                      center_newer: groups.filter((g) => g.status === "center_newer").length,
                      project_newer: groups.filter((g) => g.status === "project_newer").length,
                      diverged: groups.filter((g) => g.status === "diverged").length,
                      project_only: groups.filter((g) => g.status === "project_only").length,
                    },
                  },
            ),
          );
        }

        if (
          (client.getQueryData<OperationSnapshot[]>(key(origin)) ?? []).find(
            (o) => o.operation_id === operation.operation_id,
          )?.attempt_id !== operation.attempt_id
        )
          return;

        for (const [id, failed] of refreshFailures.current) {
          if (failed.host_id === origin && failed.operation_id === operation.operation_id)
            refreshFailures.current.delete(id);
        }

        setRefreshErrors((current) => {
          const next = { ...current };
          delete next[operationIdentity(operation)];

          return next;
        });
      } catch {
        if (
          (client.getQueryData<OperationSnapshot[]>(key(origin)) ?? []).find(
            (o) => o.operation_id === operation.operation_id,
          )?.attempt_id !== operation.attempt_id
        )
          return;
        refreshFailures.current.set(identity, operation);
        setRefreshErrors((current) => ({ ...current, [operationIdentity(operation)]: "refresh" }));
      } finally {
        refreshing.current.set(origin, (refreshing.current.get(origin) ?? 1) - 1);
        releaseIfIdle(origin);
      }
    },
    [client, releaseIfIdle],
  );

  useEffect(() => {
    for (const operation of allOperations) {
      const identity = operationIdentity(operation);

      if (operation.settled && !refreshed.current.has(identity)) {
        refreshed.current.add(identity);
        void refresh(operation);
      }
    }
  }, [allOperations, refresh]);

  useEffect(() => {
    const unlisten = listen<{ host_id?: null | string }>("operation-changed", ({ payload }) => {
      const origin = payload.host_id ?? null;

      if (!hosts.includes(origin)) return;
      void client.invalidateQueries({ queryKey: key(origin) });
    });

    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [client, hosts]);

  async function act(origin: null | string, request: OperationRequest) {
    pending.current.set(origin, (pending.current.get(origin) ?? 0) + 1);
    observe(origin);

    try {
      const snapshot = await invokeHost<OperationSnapshot>(origin, "operation_request", {
        request,
      });

      client.setQueryData<OperationSnapshot[]>(key(origin), (current) =>
        mergeOperations(current ?? [], [snapshot], origin),
      );
    } finally {
      pending.current.set(origin, (pending.current.get(origin) ?? 1) - 1);
      releaseIfIdle(origin);
      void client.invalidateQueries({ queryKey: key(origin) });
    }
  }

  async function submit(projectId: string, paths?: string[]) {
    const identity = JSON.stringify([hostId, projectId, paths ?? null]);
    const existing = submitting.current.get(identity);

    if (existing) return existing;

    const promise = act(hostId, {
      action: "submit_project_update",
      operation_id: crypto.randomUUID(),
      host_id: hostId,
      project_id: projectId,
      skill_paths: paths,
    });

    submitting.current.set(identity, promise);

    try {
      await promise;
    } finally {
      submitting.current.delete(identity);
    }
  }

  async function install(sources: InstallSource[], origin: null | string = hostId) {
    const identity = JSON.stringify([origin, "library", sources]);
    const existing = submitting.current.get(identity);

    if (existing) return existing;

    const promise = act(origin, {
      action: "submit_install",
      operation_id: crypto.randomUUID(),
      host_id: origin,
      sources,
    });

    submitting.current.set(identity, promise);

    try {
      await promise;
    } finally {
      submitting.current.delete(identity);
    }
  }

  return (
    <Context.Provider
      value={{
        operations,
        allOperations,
        unavailableHosts,
        refreshErrors,
        submit,
        install,
        act,
        refresh,
      }}
    >
      {children}
    </Context.Provider>
  );
}

export function useOperations() {
  const context = useContext(Context);

  if (!context) throw new Error("useOperations requires OperationsProvider");

  return context;
}
