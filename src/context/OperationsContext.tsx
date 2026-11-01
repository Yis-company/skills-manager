/* eslint-disable react-refresh/only-export-components */
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";

import { projectSkillsQueryOptions, queryKeys, refreshQuery } from "../lib/appQueries";
import { invokeHost } from "../lib/hostCall";
import { listenOnActiveHost } from "../lib/hostEvents";
import type { OperationRequest, OperationSnapshot } from "../lib/operations";
import { groupProjectSkills } from "../lib/projectSkillGroups";
import type { Project } from "../lib/tauri";

interface OperationsState {
  operations: OperationSnapshot[];
  refreshErrors: Record<string, string>;
  submit: (projectId: string, paths?: string[]) => Promise<void>;
  act: (request: OperationRequest) => Promise<void>;
  refresh: (operation: OperationSnapshot) => Promise<void>;
}

const Context = createContext<null | OperationsState>(null);

const EMPTY_OPERATIONS: OperationSnapshot[] = [];

const key = (hostId: null | string) => ["host", hostId, "operations"] as const;

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
  const submitting = useRef(new Map<string, Promise<void>>());

  const query = useQuery({
    queryKey: key(hostId),
    queryFn: async () => {
      const snapshots = await invokeHost<OperationSnapshot[]>(hostId, "operation_request", {
        request: { action: "list" },
      });

      const current = client.getQueryData<OperationSnapshot[]>(key(hostId)) ?? [];

      // Operations live for the session. An older in-flight list must not
      // discard a newly submitted operation or replace its newer revision.
      return [
        ...current.filter(
          (old) => !snapshots.some((next) => next.operation_id === old.operation_id),
        ),
        ...snapshots.map((next) => {
          const old = current.find((item) => item.operation_id === next.operation_id);

          return old && old.revision > next.revision ? old : next;
        }),
      ];
    },
    refetchInterval: 1000,
    refetchIntervalInBackground: true,
  });

  const operations = query.data ?? EMPTY_OPERATIONS;

  const refresh = useCallback(
    async (operation: OperationSnapshot) => {
      const origin = operation.host_id;

      try {
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
        setRefreshErrors((current) => {
          const next = { ...current };
          delete next[operation.operation_id];

          return next;
        });
      } catch {
        setRefreshErrors((current) => ({ ...current, [operation.operation_id]: "refresh" }));
      }
    },
    [client],
  );

  useEffect(() => {
    for (const operation of operations) {
      const identity = `${operation.operation_id}/${operation.attempt_id}`;

      if (operation.settled && !refreshed.current.has(identity)) {
        refreshed.current.add(identity);
        void refresh(operation);
      }
    }
  }, [operations, refresh]);

  useEffect(() => {
    const unlisten = listenOnActiveHost("operation-changed", () => {
      void client.invalidateQueries({ queryKey: key(hostId) });
    });

    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [client, hostId]);

  async function act(request: OperationRequest) {
    const snapshot = await invokeHost<OperationSnapshot>(hostId, "operation_request", { request });
    client.setQueryData<OperationSnapshot[]>(key(hostId), (current) => {
      const records = current ?? [];
      const existing = records.find((o) => o.operation_id === snapshot.operation_id);

      if (existing && existing.revision > snapshot.revision) return records;

      return existing
        ? records.map((record) =>
            record.operation_id === snapshot.operation_id ? snapshot : record,
          )
        : [...records, snapshot];
    });
    void client.invalidateQueries({ queryKey: key(hostId) });
  }

  async function submit(projectId: string, paths?: string[]) {
    const identity = JSON.stringify([hostId, projectId, paths ?? null]);
    const existing = submitting.current.get(identity);

    if (existing) return existing;

    const promise = act({
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

  return (
    <Context.Provider value={{ operations, refreshErrors, submit, act, refresh }}>
      {children}
    </Context.Provider>
  );
}

export function useOperations() {
  const context = useContext(Context);

  if (!context) throw new Error("useOperations requires OperationsProvider");

  return context;
}
