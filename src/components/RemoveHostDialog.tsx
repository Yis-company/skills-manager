import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { getErrorMessage } from "../lib/error";
import * as api from "../lib/tauri";
import { ConfirmDialog } from "./ConfirmDialog";

/**
 * Forget a host, asking "Also remove from Sync" (checked by default, SID-111):
 * checked, its deploy key goes at once and its timer when it next answers;
 * unchecked, the host keeps syncing on its own.
 */
export function RemoveHostDialog({
  host,
  onClose,
  onRemoved,
}: {
  host: { id: string; name: string } | null;
  onClose: () => void;
  onRemoved?: () => void;
}) {
  const { t } = useTranslation();
  const { activeHost, switchHost, refreshRemoteHosts } = useApp();
  const [alsoLeave, setAlsoLeave] = useState(true);

  const remove = async () => {
    if (!host) return;

    try {
      // Switch away first: a switch that fails or is declined leaves everything as it was.
      if (activeHost?.id === host.id && !(await switchHost(null))) return;

      if (alsoLeave) {
        const left = await api.hostSyncLeave(host.id);

        if (left.delete_key_by_hand) {
          toast.warning(t("remoteHosts.deleteKeyByHand", { key: left.delete_key_by_hand }), {
            duration: 15000,
          });
        }
      }

      await api.remoteHostRemove(host.id);
      await refreshRemoteHosts();
      toast.success(t("remoteHosts.removed"));
      onRemoved?.();
    } catch (e) {
      toast.error(getErrorMessage(e, t("common.error")));
    }
  };

  return (
    <ConfirmDialog
      open={host !== null}
      message={t("remoteHosts.removeConfirm", { name: host?.name ?? "" })}
      confirmLabel={t("remoteHosts.remove")}
      onClose={() => {
        setAlsoLeave(true);
        onClose();
      }}
      onConfirm={remove}
    >
      <label className="flex items-start gap-2 text-[13px] text-secondary">
        <input
          type="checkbox"
          checked={alsoLeave}
          onChange={(e) => setAlsoLeave(e.target.checked)}
          className="mt-0.5"
        />
        <span>
          {t("remoteHosts.alsoLeaveSync")}
          <span className="block text-[12px] text-muted">{t("remoteHosts.alsoLeaveSyncHint")}</span>
        </span>
      </label>
    </ConfirmDialog>
  );
}
