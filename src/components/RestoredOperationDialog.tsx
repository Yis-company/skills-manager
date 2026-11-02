import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { useOperations } from "../context/OperationsContext";
import { getErrorMessage } from "../lib/error";
import { operationHostMatches, type OperationSnapshot } from "../lib/operations";
import { Modal } from "./Modal";

/** The original context remains inspectable even after its host/project is removed. */
export function RestoredOperationDialog({
  operation,
  onClose,
}: {
  operation: OperationSnapshot;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const { remoteHosts } = useApp();
  const { reconnect } = useOperations();

  return (
    <Modal
      open
      title={operation.origin?.project_name ?? t("sidebar.installSkills")}
      onClose={onClose}
    >
      <div className="space-y-3 text-[13px] text-secondary">
        <p>{t("operations.restored")}</p>
        <p className="break-words">
          {operation.origin?.host_name ?? operation.host_id ?? t("operations.hostLocal")} ·{" "}
          {operation.origin?.ssh_target}
        </p>
        <p className="break-words">{operation.destination?.path ?? operation.project_path}</p>
        {operation.items.map((item) => (
          <p className="break-words" key={item.id}>
            {[item.relative_path, item.agent, t(`operations.phase.${item.phase}`)]
              .filter(Boolean)
              .join(" · ")}
          </p>
        ))}
        {(operation.recovery || operation.items.some((i) => i.phase === "outcome_unknown")) && (
          <p>{t("operations.unknownHelp")}</p>
        )}
        <button
          className="app-button-secondary"
          disabled={!operationHostMatches(operation, remoteHosts)}
          onClick={() => {
            void reconnect(operation).catch((e) =>
              toast.error(getErrorMessage(e, t("common.error"))),
            );
          }}
        >
          {t("operations.reconnect")}
        </button>
        {!operationHostMatches(operation, remoteHosts) && (
          <p>{t("operations.originalHostMissing")}</p>
        )}
      </div>
    </Modal>
  );
}
