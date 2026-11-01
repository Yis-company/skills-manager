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
import { getErrorKind } from "../lib/error";
import { invokeHost } from "../lib/hostCall";
import {
  checkingOperation,
  type InstallSource,
  installSourceIdentity,
  isLibraryOperation,
  mergeOperations,
  operationIdentity,
  operationNeedsSession,
  operationQueryKey,
  type OperationRequest,
  type OperationSnapshot,
} from "../lib/operations";
import { groupProjectSkills } from "../lib/projectSkillGroups";
import {
  type Project,
  remoteHostConnect,
  remoteHostRelease,
  remoteHostRetained,
} from "../lib/tauri";

interface OperationsState {
  operations: OperationSnapshot[];
  allOperations: OperationSnapshot[];
  unavailableHosts: (null | string)[];
  refreshErrors: Record<string, string>;
  install: (sources: InstallSource[], origin?: null | string) => Promise<void>;
  submit: (projectId: string, paths?: string[]) => Promise<void>;
  act: (origin: null | string, request: OperationRequest) => Promise<void>;
  refresh: (operation: OperationSnapshot) => Promise<void>;
  reconnect: (operation: OperationSnapshot) => Promise<void>;
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
  const recovering = useRef(new Map<string, Promise<void>>());

  const admissions = useRef(
    new Map<string, { origin: null | string; request: OperationRequest }>(),
  );

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

  const reconnect = useCallback(
    (operation: OperationSnapshot, request?: OperationRequest) => {
      const origin = operation.host_id;

      if (origin === null) return Promise.resolve();
      const id = JSON.stringify([origin, operation.operation_id]);
      const existing = recovering.current.get(id);

      if (existing) return existing;
      const checking = checkingOperation(operation, request);

      const update = (change: (current: OperationSnapshot) => OperationSnapshot) =>
        client.setQueryData<OperationSnapshot[]>(key(origin), (current = []) => {
          const found = current.some((o) => o.operation_id === operation.operation_id);

          return found
            ? current.map((o) => (o.operation_id === operation.operation_id ? change(o) : o))
            : [...current, change(checking)];
        });

      update(() => checking);
      let expired = false;
      let timeout: ReturnType<typeof setTimeout>;

      const work = async () => {
        for (const delay of [0, 500, 1500]) {
          if (delay) await new Promise((resolve) => setTimeout(resolve, delay));

          if (expired) return;

          try {
            await remoteHostConnect(origin, false);

            if (expired) return;

            const snapshot = await invokeHost<OperationSnapshot>(origin, "operation_request", {
              request: { action: "reconcile", operation_id: operation.operation_id },
            });

            if (expired) return;
            client.setQueryData<OperationSnapshot[]>(key(origin), (current = []) =>
              mergeOperations(current, [snapshot], origin),
            );

            const result = client
              .getQueryData<OperationSnapshot[]>(key(origin))
              ?.find((o) => o.operation_id === operation.operation_id);

            if (result && !result.recovery) return;
          } catch {
            /* Reconnect/read only: a missing receipt never permits a write. */
          }
        }
      };

      const promise = Promise.race([
        work(),
        new Promise<void>((resolve) => {
          timeout = setTimeout(resolve, 12000);
        }),
      ]).finally(() => {
        expired = true;
        clearTimeout(timeout);
        update((current) =>
          current.recovery
            ? {
                ...current,
                recovery: "unknown",
                settled: true,
                preview: null,
                items: current.items.map((item) =>
                  [
                    "queued",
                    "preparing",
                    "waiting",
                    "applying",
                    "stopping",
                    "checking_result",
                  ].includes(item.phase)
                    ? { ...item, phase: "outcome_unknown", message: null }
                    : item,
                ),
              }
            : current,
        );
        recovering.current.delete(id);
        void remoteHostRelease(origin).catch(console.error);
      });

      recovering.current.set(id, promise);

      return promise;
    },
    [client],
  );

  useEffect(() => {
    hosts.forEach((origin, index) => {
      if (origin === null || getErrorKind(queries[index].error) !== "network") return;

      for (const operation of queries[index].data ?? [])
        if (!operation.settled && !operation.recovery) void reconnect(operation);
    });
  }, [hosts, queries, reconnect]);

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

      if (operation.settled && !operation.recovery && !refreshed.current.has(identity)) {
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

  const recoverRequest = useCallback(
    (origin: null | string, request: OperationRequest) => {
      if (!("operation_id" in request)) return Promise.resolve();

      const existing = client
        .getQueryData<OperationSnapshot[]>(key(origin))
        ?.find((o) => o.operation_id === request.operation_id);

      const projectId = request.action === "submit_project_update" ? request.project_id : null;

      const project = client
        .getQueryData<Project[]>(queryKeys.projects(origin))
        ?.find((p) => p.id === projectId);

      return reconnect(
        existing ?? {
          operation_id: request.operation_id,
          attempt_id: "",
          host_id: origin,
          project_id: projectId,
          project_path: project?.path ?? null,
          destination: projectId
            ? { kind: "project", id: projectId, path: project?.path ?? "" }
            : { kind: "library", path: "" },
          sources: request.action === "submit_install" ? request.sources : [],
          revision: 0,
          settled: false,
          cancel_requested: false,
          items: [],
        },
        request,
      );
    },
    [client, reconnect],
  );

  useEffect(() => {
    const unlisten = listen<{ host_id: string; state: string }>(
      "remote-session-state",
      ({ payload }) => {
        if (payload.state !== "disconnected") return;
        const records = client.getQueryData<OperationSnapshot[]>(key(payload.host_id)) ?? [];

        for (const { origin, request } of admissions.current.values())
          if (origin === payload.host_id) void recoverRequest(origin, request);

        for (const operation of records)
          if (!operation.settled && !operation.recovery) void reconnect(operation);
      },
    );

    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [client, reconnect, recoverRequest]);

  async function act(origin: null | string, request: OperationRequest) {
    const records = client.getQueryData<OperationSnapshot[]>(key(origin)) ?? [];

    const uncertain = records.filter(
      (o) => o.recovery || o.items.some((i) => i.phase === "outcome_unknown"),
    );

    if (
      request.action === "submit_project_update" &&
      uncertain.some((o) => o.project_id === request.project_id)
    )
      return;

    if (
      request.action === "submit_install" &&
      uncertain.some((o) =>
        [...(o.sources ?? []), ...o.items.flatMap((i) => (i.source ? [i.source] : []))].some(
          (source) =>
            request.sources.some(
              (selected) => installSourceIdentity(source) === installSourceIdentity(selected),
            ),
        ),
      )
    )
      return;

    if (
      ["confirm_install", "retry_failed", "cancel"].includes(request.action) &&
      "operation_id" in request &&
      records.some((o) => o.operation_id === request.operation_id && o.recovery)
    )
      return;
    pending.current.set(origin, (pending.current.get(origin) ?? 0) + 1);
    observe(origin);
    const admission = crypto.randomUUID();
    admissions.current.set(admission, { origin, request });

    try {
      const snapshot = await invokeHost<OperationSnapshot>(origin, "operation_request", {
        request,
      });

      client.setQueryData<OperationSnapshot[]>(key(origin), (current) =>
        mergeOperations(current ?? [], [snapshot], origin),
      );
    } catch (error) {
      if (origin !== null && getErrorKind(error) === "network")
        await recoverRequest(origin, request);
      else throw error;
    } finally {
      admissions.current.delete(admission);
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
        reconnect,
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
