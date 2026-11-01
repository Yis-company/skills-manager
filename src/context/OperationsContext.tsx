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
  operationHostMatches,
  operationIdentity,
  operationNeedsAttention,
  operationNeedsSession,
  operationQueryKey,
  type OperationRequest,
  type OperationSnapshot,
  sameOperationContext,
} from "../lib/operations";
import { groupProjectSkills } from "../lib/projectSkillGroups";
import {
  type Project,
  type RemoteHost,
  remoteHostConnect,
  remoteHostRelease,
  remoteHostRetained,
} from "../lib/tauri";
import { useOperationPersistence } from "./useOperationPersistence";

interface OperationsState {
  persistenceStatus: "error" | "loading" | "saved" | "saving";
  flush: () => Promise<void>;
  dismiss: (operation: OperationSnapshot) => void;
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
  remoteHosts,
  children,
}: {
  hostId: null | string;
  remoteHosts: RemoteHost[];
  children: ReactNode;
}) {
  const client = useQueryClient();
  const [refreshErrors, setRefreshErrors] = useState<Record<string, string>>({});
  const dismissed = useRef(new Set<string>());
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

  const { persistenceStatus, observationReady, flush } = useOperationPersistence(client, observe);

  const captureOrigin = (snapshot: OperationSnapshot) => {
    const host = remoteHosts.find((h) => h.id === snapshot.host_id);

    const project = client
      .getQueryData<Project[]>(queryKeys.projects(snapshot.host_id))
      ?.find((p) => p.id === snapshot.project_id);

    return {
      ...snapshot,
      origin: snapshot.origin ?? {
        host_name: host?.name ?? (snapshot.host_id === null ? "Local" : snapshot.host_id),
        ssh_target: host?.ssh_target ?? null,
        host_created_at: host?.created_at ?? null,
        project_name: project?.name ?? null,
      },
    };
  };

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
          snapshots.map(captureOrigin),
          origin,
          dismissed.current,
        );
      },
      enabled: () =>
        observationReady &&
        (client.getQueryData(key(origin)) === undefined ||
          origin === hostId ||
          (pending.current.get(origin) ?? 0) > 0 ||
          (client.getQueryData<OperationSnapshot[]>(key(origin)) ?? []).some(
            (o) => !o.restored && operationNeedsSession(o),
          )),
      refetchInterval: 1000,
      refetchIntervalInBackground: true,
      retry: false,
    })),
  });

  const unavailableHosts = hosts.filter((_, index) => queries[index].isError);
  const allOperations = queries.flatMap((query) => query.data ?? EMPTY_OPERATIONS);

  const operations = allOperations.filter(
    (operation) => operation.host_id === hostId && operationHostMatches(operation, remoteHosts),
  );

  const reconnect = useCallback(
    (operation: OperationSnapshot, request?: OperationRequest) => {
      const origin = operation.host_id;

      if (!operationHostMatches(operation, remoteHosts))
        return Promise.reject(
          new Error("The original host is unavailable; its saved identity does not match"),
        );
      const id = JSON.stringify([origin, operation.operation_id]);
      const existing = recovering.current.get(id);

      if (existing) return existing;
      const checking = checkingOperation(operation, request);

      const update = (change: (current: OperationSnapshot) => OperationSnapshot) =>
        client.setQueryData<OperationSnapshot[]>(key(origin), (current = []) => {
          const found = current.some((o) => sameOperationContext(o, operation));

          return found
            ? current.map((o) => (sameOperationContext(o, operation) ? change(o) : o))
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
            if (origin !== null) await remoteHostConnect(origin, false);

            if (expired) return;

            const snapshot = await invokeHost<OperationSnapshot>(origin, "operation_request", {
              request: { action: "reconcile", operation_id: operation.operation_id },
            });

            if (expired) return;
            client.setQueryData<OperationSnapshot[]>(key(origin), (current = []) =>
              mergeOperations(current, [snapshot], origin, dismissed.current),
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

        if (origin !== null) void remoteHostRelease(origin).catch(console.error);
      });

      recovering.current.set(id, promise);

      return promise;
    },
    [client, remoteHosts],
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
          const project = client
            .getQueryData<Project[]>(queryKeys.projects(origin))
            ?.find((p) => p.id === operation.project_id);

          if (project && operation.project_path && project.path !== operation.project_path)
            throw new Error("The original project is no longer available");

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

      if (
        operation.settled &&
        !operation.restored &&
        !operation.recovery &&
        !refreshed.current.has(identity)
      ) {
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
    const records = (client.getQueryData<OperationSnapshot[]>(key(origin)) ?? []).filter((o) =>
      operationHostMatches(o, remoteHosts),
    );

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
      ["confirm_install", "retry_failed", "cancel", "prepare_review"].includes(request.action) &&
      "operation_id" in request &&
      records.some((o) => o.operation_id === request.operation_id && o.recovery)
    )
      return;

    const prior =
      "operation_id" in request
        ? records.find((o) => o.operation_id === request.operation_id)
        : undefined;

    if (prior?.admission) return;

    if (prior?.restored && origin !== null) await remoteHostConnect(origin, false);

    if (
      "operation_id" in request &&
      !prior &&
      (client.getQueryData<OperationSnapshot[]>(key(origin)) ?? []).some(
        (o) => o.operation_id === request.operation_id,
      )
    )
      throw new Error("The original host identity no longer matches");
    pending.current.set(origin, (pending.current.get(origin) ?? 0) + 1);
    observe(origin);
    const admission = crypto.randomUUID();
    admissions.current.set(admission, { origin, request });

    const projectId = request.action === "submit_project_update" ? request.project_id : null;

    const project = client
      .getQueryData<Project[]>(queryKeys.projects(origin))
      ?.find((p) => p.id === projectId);

    const provisional: OperationSnapshot | undefined =
      prior ??
      ("operation_id" in request
        ? captureOrigin({
            operation_id: request.operation_id,
            attempt_id: "",
            host_id: origin,
            project_id: projectId,
            project_path: project?.path ?? null,
            destination: projectId
              ? { kind: "project", id: projectId, path: project?.path ?? "" }
              : { kind: "library", path: "" },
            work: projectId ? "project_update" : "installation",
            sources: request.action === "submit_install" ? request.sources : [],
            revision: 0,
            settled: false,
            cancel_requested: false,
            items: [],
          })
        : undefined);

    if (provisional)
      client.setQueryData<OperationSnapshot[]>(key(origin), (current = []) => [
        ...current.filter((o) => !sameOperationContext(o, provisional)),
        {
          ...provisional,
          admission: {
            id: admission,
            action: request.action,
            expected_attempt: "attempt_id" in request ? request.attempt_id : undefined,
          },
        },
      ]);
    // Capture uncertainty before calling the host; storage failure stays visible
    // and never discards the result held in memory.
    await flush().catch(() => {});

    try {
      const snapshot = await invokeHost<OperationSnapshot>(origin, "operation_request", {
        request,
      });

      if (
        snapshot.host_id !== origin ||
        ("operation_id" in request &&
          !["submit_install", "submit_project_update"].includes(request.action) &&
          snapshot.operation_id !== request.operation_id) ||
        ("attempt_id" in request && snapshot.attempt_id !== request.attempt_id)
      ) {
        await recoverRequest(origin, request);

        return;
      }

      client.setQueryData<OperationSnapshot[]>(key(origin), (current = []) =>
        mergeOperations(
          snapshot.host_id === origin &&
            provisional &&
            snapshot.operation_id !== provisional.operation_id
            ? current.filter((o) => o.admission?.id !== admission)
            : current,
          [captureOrigin(snapshot)],
          origin,
          dismissed.current,
        ),
      );
    } catch (error) {
      if (origin !== null && getErrorKind(error) === "network")
        await recoverRequest(origin, request);
      else {
        client.setQueryData<OperationSnapshot[]>(key(origin), (current = []) =>
          current.flatMap((o) => (o.admission?.id === admission ? (prior ? [prior] : []) : [o])),
        );
        throw error;
      }
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

  function dismiss(operation: OperationSnapshot) {
    if (!operation.settled || operation.admission || operationNeedsAttention(operation)) return;
    dismissed.current.add(operationIdentity(operation));
    client.setQueryData<OperationSnapshot[]>(key(operation.host_id), (current = []) =>
      current.filter((o) => operationIdentity(o) !== operationIdentity(operation)),
    );
    releaseIfIdle(operation.host_id);
  }

  return (
    <Context.Provider
      value={{
        persistenceStatus,
        flush,
        dismiss,
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
