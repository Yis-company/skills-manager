import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useOperations } from "../context/OperationsContext";
import { getErrorMessage } from "../lib/error";
import {
  operationCounts,
  operationIdentity,
  type OperationItem,
  type OperationSnapshot,
} from "../lib/operations";

export function ProjectOperationProgress({ operation }: { operation: OperationSnapshot }) {
  const { t } = useTranslation();
  const progressRef = useRef<HTMLElement>(null);
  const { act, refresh, refreshErrors } = useOperations();
  const counts = operationCounts(operation);

  const perform = (action: "cancel" | "retry_failed") => {
    void act(
      operation.host_id,
      action === "cancel"
        ? { action, operation_id: operation.operation_id }
        : { action, operation_id: operation.operation_id, attempt_id: crypto.randomUUID() },
    )
      .then(() => progressRef.current?.focus())
      .catch((error) => toast.error(getErrorMessage(error, t("common.error"))));
  };

  return (
    <section
      ref={progressRef}
      tabIndex={-1}
      aria-label={t("operations.projectProgress")}
      className="app-panel space-y-2 px-4 py-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div role="status" className="min-w-0 text-[13px]">
          <p className="font-medium text-primary">
            {t(
              operation.settled
                ? "operations.finished"
                : operation.cancel_requested
                  ? "operations.stopping"
                  : "operations.updating",
            )}
          </p>
          <p className="tabular-nums text-secondary">
            {operation.settled ? t("operations.result", counts) : t("operations.progress", counts)}
          </p>
        </div>
        {!operation.settled && (
          <button
            className="app-toolbar-button app-toolbar-button-secondary"
            disabled={operation.cancel_requested}
            onClick={() => perform("cancel")}
          >
            {t(operation.cancel_requested ? "operations.stopping" : "operations.cancel")}
          </button>
        )}
        {operation.settled && counts.failed > 0 && (
          <button
            className="app-toolbar-button app-toolbar-button-secondary"
            onClick={() => perform("retry_failed")}
          >
            {t("operations.retry", { count: counts.failed })}
          </button>
        )}
      </div>
      {(counts.unknown > 0 || counts.unchanged > 0) && (
        <p className="text-[13px] text-secondary">{t("operations.otherResults", counts)}</p>
      )}
      {refreshErrors[operationIdentity(operation)] && (
        <div className="flex flex-wrap items-center gap-2 text-[13px] text-secondary">
          <span>{t("operations.refreshFailed")}</span>
          <button
            className="app-toolbar-button app-toolbar-button-secondary"
            onClick={() => {
              void refresh(operation);
            }}
          >
            {t("common.refresh")}
          </button>
        </div>
      )}
    </section>
  );
}

export function ProjectItemProgress({
  items,
  targets,
}: {
  items: OperationItem[];
  targets: { key: string; display_name: string }[];
}) {
  const { t } = useTranslation();

  if (items.length === 0) return null;

  return (
    <div className="space-y-1 text-[12px] text-secondary" role="status">
      {items.map((item) => (
        <p key={item.id}>
          <span className="font-medium">
            {targets.find((target) => target.key === item.agent)?.display_name ?? item.agent}:{" "}
            {t(`operations.phase.${item.phase}`)}
          </span>
          {item.message && <span className="block break-words">{item.message}</span>}
        </p>
      ))}
    </div>
  );
}
