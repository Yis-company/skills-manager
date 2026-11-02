/** Shared with core/operations.rs; see docs/operations.md. */
export type OperationPhase =
  | "applying"
  | "canceled"
  | "checking_result"
  | "failed"
  | "needs_review"
  | "outcome_unknown"
  | "preparing"
  | "queued"
  | "stopping"
  | "succeeded"
  | "unchanged"
  | "waiting";

export type InstallSource =
  | { kind: "git"; url: string }
  | { kind: "local_batch"; path: string }
  | { kind: "local"; path: string; name?: null | string; origin?: "discovered" | null }
  | { kind: "marketplace"; source: string; skill_id: string };

/** Match host-captured intent, including nullable fields and sanitized Git URLs. */
export function installSourceIdentity(source: InstallSource): string {
  switch (source.kind) {
    case "local":
      return JSON.stringify([source.kind, source.path, source.name ?? null, source.origin ?? null]);
    case "local_batch":
      return JSON.stringify([source.kind, source.path]);
    case "marketplace":
      return JSON.stringify([source.kind, source.source, source.skill_id]);
    case "git": {
      let url = source.url;

      try {
        const parsed = new URL(url);

        if (["http:", "https:"].includes(parsed.protocol)) parsed.username = "";
        parsed.password = "";
        parsed.search = "";
        parsed.hash = "";
        url = parsed.toString();
      } catch {
        /* SCP-style Git sources retain their original spelling. */
      }

      return JSON.stringify([source.kind, url]);
    }
  }
}

export type OperationDestination =
  | { kind: "library"; path: string }
  | { kind: "project"; id: string; path: string };

export interface OperationItem {
  id: string;
  attempt_id: string;
  relative_path: string;
  agent: null | string;
  source?: InstallSource | null;
  phase: OperationPhase;
  message: null | string;
}

export interface OperationSnapshot {
  operation_id: string;
  /** Cached display identity; never a connection, destination or approval authority. */
  origin?: {
    host_name: string;
    ssh_target?: null | string;
    host_created_at?: null | number;
    project_name?: null | string;
  };
  restored?: boolean;
  admission?: { id: string; action: string; expected_attempt?: string };
  /** App-session transport observation; never a host receipt or replay authority. */
  recovery?: "checking" | "unknown";
  recovery_attempt?: string;
  attempt_id: string;
  host_id: null | string;
  project_id: null | string;
  project_path: null | string;
  work?: "installation" | "project_update";
  destination?: null | OperationDestination;
  sources?: InstallSource[];
  preview?: { rel_path: string; name: string; description: null | string }[] | null;
  revision: number;
  settled: boolean;
  cancel_requested: boolean;
  items: OperationItem[];
}

export type OperationRequest =
  | { action: "cancel" | "read" | "reconcile"; operation_id: string }
  | {
      action: "confirm_install";
      operation_id: string;
      attempt_id: string;
      items: { rel_path: string; name: string }[];
    }
  | { action: "list" }
  | { action: "prepare_review"; operation_id: string; attempt_id: string; item_ids?: string[] }
  | { action: "retry_failed"; operation_id: string; attempt_id: string }
  | {
      action: "submit_install";
      operation_id: string;
      host_id: null | string;
      sources: InstallSource[];
    }
  | {
      action: "submit_project_update";
      operation_id: string;
      host_id: null | string;
      project_id: string;
      skill_paths?: string[];
    };

export const isItemActive = (item: OperationItem) =>
  ["queued", "preparing", "waiting", "applying", "stopping", "checking_result"].includes(
    item.phase,
  );

export const operationCounts = (operation: OperationSnapshot) => ({
  total: operation.items.length,
  completed: operation.items.filter((item) => !isItemActive(item)).length,
  updated: operation.items.filter((item) => item.phase === "succeeded").length,
  failed: operation.items.filter((item) => item.phase === "failed").length,
  review: operation.items.filter((item) => item.phase === "needs_review").length,
  canceled: operation.items.filter((item) => item.phase === "canceled").length,
  unknown: operation.items.filter((item) => item.phase === "outcome_unknown").length,
  unchanged: operation.items.filter((item) => item.phase === "unchanged").length,
});

export const isLibraryOperation = (operation: OperationSnapshot) =>
  operation.destination?.kind === "library";

/** Older project receipts may omit the path; captured paths must still match. */
export const operationProjectMatches = (
  operation: OperationSnapshot,
  hostId: null | string,
  projectId: string,
  projectPath?: string,
) =>
  operation.host_id === hostId &&
  operation.project_id === projectId &&
  (!operation.project_path || operation.project_path === projectPath);

export const operationQueryKey = (hostId: null | string) => ["host", hostId, "operations"] as const;

export const installationPhaseKey = (phase: OperationPhase) =>
  phase === "succeeded"
    ? "install.installed"
    : phase === "needs_review"
      ? "operations.installNeedsReview"
      : `operations.phase.${phase}`;

/** Tuple encoding avoids collisions between host, operation and attempt identifiers. */
export const operationIdentity = (operation: OperationSnapshot) =>
  JSON.stringify([
    operation.host_id,
    operation.operation_id,
    operation.attempt_id,
    operation.origin?.ssh_target ?? null,
    operation.origin?.host_created_at ?? null,
  ]);

export const operationNeedsAttention = (operation: OperationSnapshot) =>
  operation.recovery === "unknown" ||
  operation.items.some((item) =>
    ["failed", "needs_review", "outcome_unknown"].includes(item.phase),
  );

export const operationNeedsSession = (operation: OperationSnapshot) =>
  !!operation.admission || !operation.settled || operationNeedsAttention(operation);

/** A saved host ID/name alone cannot authorize a connection to a replacement. */
export function operationHostMatches(
  operation: OperationSnapshot,
  hosts: { id: string; ssh_target: string; created_at: number }[],
) {
  if (operation.host_id === null || (!operation.restored && !operation.origin?.ssh_target))
    return true;
  const host = hosts.find((h) => h.id === operation.host_id);

  return (
    !!host &&
    operation.origin?.ssh_target === host.ssh_target &&
    operation.origin.host_created_at === host.created_at
  );
}

export function sameOperationContext(a: OperationSnapshot, b: OperationSnapshot) {
  return (
    a.host_id === b.host_id &&
    a.operation_id === b.operation_id &&
    (!a.origin ||
      !b.origin ||
      (a.origin.ssh_target === b.origin.ssh_target &&
        a.origin.host_created_at === b.origin.host_created_at))
  );
}

export function mergeOperations(
  current: OperationSnapshot[],
  snapshots: OperationSnapshot[],
  hostId: null | string,
  dismissed = new Set<string>(),
) {
  const records = [...current];

  for (let snapshot of snapshots) {
    if (snapshot.host_id !== hostId || dismissed.has(operationIdentity(snapshot))) continue;
    const index = records.findIndex((old) => sameOperationContext(old, snapshot));

    if (index < 0) records.push(snapshot);
    else {
      const old = records[index];
      snapshot = { ...snapshot, origin: old.origin ?? snapshot.origin };

      if (snapshot.revision < old.revision) continue;

      if (old.admission?.expected_attempt && old.admission.expected_attempt !== snapshot.attempt_id)
        continue;

      if (old.recovery) {
        if (old.recovery_attempt && snapshot.attempt_id !== old.recovery_attempt) continue;

        // Silence, an empty list, or an unfinished unowned receipt cannot clear
        // uncertainty. Equal-revision verified reads can repair a lost reply.
        if (snapshot.settled && !snapshot.items.some((item) => item.phase === "outcome_unknown"))
          records[index] = snapshot;
        else {
          const merged = {
            ...snapshot,
            recovery: old.recovery,
            recovery_attempt: old.recovery_attempt,
            settled: old.recovery === "unknown",
            preview: null,
            items: snapshot.items.map((item) => {
              const prior = old.items.find((previous) => previous.id === item.id);

              if (
                prior &&
                ["succeeded", "unchanged"].includes(prior.phase) &&
                (isItemActive(item) || item.phase === "outcome_unknown")
              )
                return prior;

              return isItemActive(item) || item.phase === "outcome_unknown"
                ? {
                    ...item,
                    phase:
                      old.recovery === "checking"
                        ? ("checking_result" as const)
                        : ("outcome_unknown" as const),
                  }
                : item;
            }),
          };

          records[index] = merged;
        }
      } else if (
        snapshot.revision > old.revision ||
        ((old.restored || old.admission) && snapshot.revision === old.revision)
      )
        records[index] = snapshot;
    }
  }

  return records;
}

/** Keep successful siblings; checking a response is observation, not execution. */
export function checkingOperation(
  operation: OperationSnapshot,
  request?: OperationRequest,
): OperationSnapshot {
  const attempt =
    request && "attempt_id" in request
      ? request.attempt_id
      : (operation.recovery_attempt ?? (operation.attempt_id || undefined));

  return {
    ...operation,
    recovery: "checking",
    recovery_attempt: attempt,
    settled: false,
    preview: null,
    items: operation.items.map((item) =>
      isItemActive(item) ||
      item.phase === "outcome_unknown" ||
      (request?.action === "retry_failed" && item.phase === "failed") ||
      request?.action === "confirm_install"
        ? { ...item, phase: "checking_result", message: null }
        : item,
    ),
  };
}
