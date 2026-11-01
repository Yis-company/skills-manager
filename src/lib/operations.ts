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

export const operationQueryKey = (hostId: null | string) => ["host", hostId, "operations"] as const;

export const installationPhaseKey = (phase: OperationPhase) =>
  phase === "succeeded"
    ? "install.installed"
    : phase === "needs_review"
      ? "operations.installNeedsReview"
      : `operations.phase.${phase}`;

/** Tuple encoding avoids collisions between host, operation and attempt identifiers. */
export const operationIdentity = (operation: OperationSnapshot) =>
  JSON.stringify([operation.host_id, operation.operation_id, operation.attempt_id]);

export const operationNeedsAttention = (operation: OperationSnapshot) =>
  operation.items.some((item) =>
    ["failed", "needs_review", "outcome_unknown"].includes(item.phase),
  );

export const operationNeedsSession = (operation: OperationSnapshot) =>
  !operation.settled || operationNeedsAttention(operation);

export function mergeOperations(
  current: OperationSnapshot[],
  snapshots: OperationSnapshot[],
  hostId: null | string,
) {
  const records = [...current];

  for (const snapshot of snapshots) {
    if (snapshot.host_id !== hostId) continue;
    const index = records.findIndex((old) => old.operation_id === snapshot.operation_id);

    if (index < 0) records.push(snapshot);
    else if (snapshot.revision > records[index].revision) records[index] = snapshot;
  }

  return records;
}
