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

export interface OperationItem {
  id: string;
  attempt_id: string;
  relative_path: string;
  agent: string;
  phase: OperationPhase;
  message: null | string;
}

export interface OperationSnapshot {
  operation_id: string;
  attempt_id: string;
  host_id: null | string;
  project_id: string;
  project_path: string;
  revision: number;
  settled: boolean;
  cancel_requested: boolean;
  items: OperationItem[];
}

export type OperationRequest =
  | { action: "cancel" | "read" | "reconcile"; operation_id: string }
  | { action: "list" }
  | { action: "retry_failed"; operation_id: string; attempt_id: string }
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
