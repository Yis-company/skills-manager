import { useTranslation } from "react-i18next";

import type { SyncHost } from "../../hooks/useSyncScreen";
import { formatWhen } from "../../lib/backupFormat";
import type { SyncHistoryEntry } from "../../lib/tauri";
import { StatePill } from "./parts";

/** A card per host, then the latest changes from the shared history. */
export function Overview({
  hosts,
  history,
  onOpen,
}: {
  hosts: SyncHost[];
  history: SyncHistoryEntry[];
  onOpen: (host: SyncHost) => void;
}) {
  const { t } = useTranslation();

  return (
    <div className="space-y-5">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {hosts.map((host) => (
          <button
            key={host.id ?? "local"}
            type="button"
            onClick={() => onOpen(host)}
            className="rounded-lg border border-border-subtle p-3 text-left hover:bg-surface-hover"
          >
            <div className="truncate text-[13px] font-medium text-primary">
              {host.name || t("sync.thisHost")}
            </div>
            <StatePill state={host.state} className="mt-2" />
            <div className="mt-1 text-[11px] text-faint">
              {formatWhen(host.status?.last_sync ?? null) ?? t("sync.neverSynced")}
            </div>
          </button>
        ))}
      </div>
      <div>
        <div className="app-section-title mb-2">{t("sync.recentChanges")}</div>
        {history.length === 0 ? (
          <p className="text-[13px] text-muted">{t("backup.history.empty")}</p>
        ) : (
          <ul className="divide-y divide-border-subtle text-[13px]">
            {history.slice(0, 8).map((entry) => (
              <li key={entry.commit} className="flex gap-3 py-2">
                <span className="w-36 shrink-0 text-[12px] text-faint">
                  {formatWhen(entry.committed_at)}
                </span>
                <span className="w-28 shrink-0 truncate font-medium text-tertiary">
                  {entry.host}
                </span>
                <span className="min-w-0 truncate text-secondary">{entry.message}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
