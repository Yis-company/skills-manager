import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { getErrorMessage } from "../../lib/error";
import * as api from "../../lib/tauri";

/**
 * Skills changed on two hosts (SID-108). Choices are named by host, recorded
 * in Sync, and applied by each host on its next pull.
 */
export function Review({
  conflicts,
  onResolved,
}: {
  conflicts: api.SyncConflict[];
  onResolved: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<null | string>(null);

  if (conflicts.length === 0) {
    return <div className="py-16 text-center text-[13px] text-muted">{t("sync.review.empty")}</div>;
  }

  const resolve = async (conflict: api.SyncConflict, keep: null | string) => {
    setBusy(conflict.skill_id);

    try {
      await api.syncCall(null, "sync_resolve", {
        skill: conflict.skill_id,
        keep,
        both: keep === null,
      });
      toast.success(t("backup.conflicts.resolved"));
      await onResolved();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-3">
      <p className="text-[12px] text-muted">{t("sync.review.desc")}</p>
      {conflicts.map((conflict) => (
        <div key={conflict.skill_id} className="rounded-lg border border-border-subtle p-4">
          <div className="text-[11px] font-semibold uppercase tracking-wide text-amber-600 dark:text-amber-400">
            {t("sync.review.skill")}
          </div>
          <div className="mt-0.5 text-[14px] font-medium text-primary">{conflict.name}</div>
          <div className="text-[12px] text-muted">
            {t("sync.review.changedOnBoth", {
              one: conflict.this_host,
              other: conflict.other_host || t("sync.review.anotherHost"),
            })}
          </div>
          <div className="mt-3 flex flex-wrap gap-2">
            {[conflict.this_host, conflict.other_host].flatMap((host) =>
              host
                ? [
                    <button
                      key={host}
                      type="button"
                      disabled={busy !== null}
                      className="app-button-secondary !py-1.5"
                      onClick={() => resolve(conflict, host)}
                    >
                      {t("sync.review.keep", { host })}
                    </button>,
                  ]
                : [],
            )}
            <button
              type="button"
              disabled={busy !== null}
              className="app-button-secondary !py-1.5"
              onClick={() => resolve(conflict, null)}
            >
              {t("sync.review.keepBoth")}
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
