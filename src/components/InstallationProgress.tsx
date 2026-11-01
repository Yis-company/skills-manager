import { useRef } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { useOperations } from "../context/OperationsContext";
import { getErrorMessage } from "../lib/error";
import {
  installationPhaseKey,
  operationCounts,
  operationIdentity,
  type OperationRequest,
  type OperationSnapshot,
} from "../lib/operations";

export function InstallationProgress({
  operation,
  onReview,
}: {
  operation: OperationSnapshot;
  onReview?: () => void;
}) {
  const { t } = useTranslation();
  const { remoteHosts } = useApp();
  const { act, install, refresh, refreshErrors } = useOperations();
  const region = useRef<HTMLElement>(null);
  const counts = operationCounts(operation);

  const host = operation.host_id
    ? (remoteHosts.find((h) => h.id === operation.host_id)?.name ?? operation.host_id)
    : t("operations.hostLocal");

  const perform = (request: OperationRequest) => {
    void act(operation.host_id, request)
      .then(() => region.current?.focus())
      .catch((e) => toast.error(getErrorMessage(e, t("common.error"))));
  };

  return (
    <section
      ref={region}
      tabIndex={-1}
      aria-label={t("operations.installProgress")}
      className="app-panel space-y-2 px-4 py-3"
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0 text-[13px]" role="status">
          <p className="font-medium text-primary">{t("operations.installDestination", { host })}</p>
          <p className="text-secondary">
            {t(
              operation.preview?.length
                ? "operations.installPreviewReady"
                : operation.settled && counts.review > 0
                  ? "operations.installNeedsReview"
                  : operation.settled
                    ? "operations.installFinished"
                    : "operations.installRunning",
            )}
            {!operation.preview?.length && (
              <>
                {" "}
                ·{" "}
                {counts.total
                  ? `${counts.completed}/${counts.total}`
                  : t("operations.installNothing")}
              </>
            )}
          </p>
        </div>
        {!operation.settled && (
          <button
            className="app-button-secondary"
            disabled={operation.cancel_requested}
            onClick={() => perform({ action: "cancel", operation_id: operation.operation_id })}
          >
            {t(operation.cancel_requested ? "operations.stopping" : "operations.installCancel")}
          </button>
        )}
        {operation.settled && counts.failed > 0 && (
          <button
            className="app-button-secondary"
            onClick={() =>
              perform({
                action: "retry_failed",
                operation_id: operation.operation_id,
                attempt_id: crypto.randomUUID(),
              })
            }
          >
            {t("operations.retry", { count: counts.failed })}
          </button>
        )}
        {operation.preview?.length ? (
          <button
            className="app-button-secondary"
            onClick={() => {
              region.current?.focus();
              onReview?.();
            }}
          >
            {t("operations.reviewInstall")}
          </button>
        ) : (
          operation.settled &&
          counts.review > 0 && (
            <button
              className="app-button-secondary"
              onClick={() => {
                const sources = operation.items
                  .filter((i) => i.phase === "needs_review")
                  .flatMap((i) => (i.source ? [i.source] : []));

                void install(sources, operation.host_id).catch((e) =>
                  toast.error(getErrorMessage(e, t("common.error"))),
                );
              }}
            >
              {t("operations.prepareAgain")}
            </button>
          )
        )}
      </div>
      <div className="space-y-1 text-[13px] text-secondary" role="status">
        {operation.items.map((item) => (
          <p key={item.id} className="break-words">
            <span className="font-medium">
              {item.relative_path}: {t(installationPhaseKey(item.phase))}
            </span>
            {item.message && <span className="block">{item.message}</span>}
          </p>
        ))}
      </div>
      {refreshErrors[operationIdentity(operation)] && (
        <div className="flex flex-wrap items-center gap-2 text-[13px] text-secondary">
          <span>{t("operations.libraryRefreshFailed")}</span>
          <button
            className="app-button-secondary"
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
