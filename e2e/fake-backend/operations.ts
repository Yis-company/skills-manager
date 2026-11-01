/** Contract responses only. Scheduling and filesystem safety are tested in Rust. */
import type { OperationRequest, OperationSnapshot } from "../../src/lib/operations";
import type { State } from "./state";

export function operationHandler(
  { request }: { request: OperationRequest },
  state: State,
): OperationSnapshot | OperationSnapshot[] {
  if (request.action === "list") return state.operations;

  if (request.action === "submit_project_update") {
    const existing = state.operations.find(
      (o) =>
        o.operation_id === request.operation_id ||
        (!o.settled && o.project_id === request.project_id),
    );

    if (existing) return existing;

    const items = (state.projectSkills[request.project_id] ?? []).filter(
      (s) =>
        !s.alias_of &&
        s.in_center &&
        ["center_newer", "project_newer", "diverged"].includes(s.sync_status) &&
        (!request.skill_paths || request.skill_paths.includes(s.relative_path)),
    );

    const operation: OperationSnapshot = {
      operation_id: request.operation_id,
      attempt_id: "attempt-1",
      host_id: request.host_id,
      project_id: request.project_id,
      project_path: state.projects.find((p) => p.id === request.project_id)?.path ?? "",
      revision: 1,
      cancel_requested: false,
      settled: !state.holdOperations,
      items: items.map((s, i) => ({
        id: `${request.operation_id}-${i}`,
        attempt_id: "attempt-1",
        relative_path: s.relative_path,
        agent: s.agent,
        phase:
          s.sync_status !== "center_newer"
            ? "needs_review"
            : state.holdOperations
              ? "queued"
              : "succeeded",
        message: null,
      })),
    };

    state.operations.push(operation);

    if (!state.holdOperations)
      for (const item of operation.items) {
        if (item.phase === "succeeded") {
          const skill = state.projectSkills[operation.project_id].find(
            (s) => s.relative_path === item.relative_path && s.agent === item.agent,
          );

          if (skill) skill.sync_status = "in_sync";
        }
      }

    return operation;
  }

  const operation = state.operations.find((o) => o.operation_id === request.operation_id);

  if (!operation) throw new Error("Operation not found");

  if (request.action === "cancel") {
    operation.cancel_requested = true;
    operation.settled = true;
    operation.revision++;
    operation.items = operation.items.map((item) =>
      ["queued", "waiting", "preparing"].includes(item.phase)
        ? { ...item, phase: "canceled" }
        : item,
    );
  }

  if (request.action === "retry_failed") {
    operation.attempt_id = request.attempt_id;
    operation.revision++;
    operation.items = operation.items.map((item) =>
      item.phase === "failed"
        ? {
            ...item,
            phase: state.holdOperations ? "queued" : "succeeded",
            attempt_id: request.attempt_id,
            message: null,
          }
        : item,
    );
    operation.settled = !operation.items.some((item) => item.phase === "queued");
  }

  return operation;
}
