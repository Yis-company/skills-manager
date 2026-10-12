import { X } from "lucide-react";
import { useTranslation } from "react-i18next";

import type { SyncStateKey } from "../../lib/tauri";
import { cn } from "../../utils";

/** "syncing" is the screen's own state while Sync now runs. */
export type ShownState = "syncing" | SyncStateKey;

const DOT: Record<ShownState, string> = {
  up_to_date: "bg-emerald-500",
  syncing: "bg-accent animate-pulse",
  paused: "bg-zinc-400",
  offline: "bg-zinc-400",
  not_set_up: "bg-zinc-300 dark:bg-zinc-600",
  needs_review: "bg-amber-500",
  update_required: "bg-red-500",
};

export function StateDot({ state }: { state: ShownState }) {
  return <span className={cn("h-2 w-2 shrink-0 rounded-full", DOT[state])} />;
}

export function StatePill({ state, className }: { state: ShownState; className?: string }) {
  const { t } = useTranslation();

  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full border border-border-subtle px-2 py-0.5 text-[11px] text-secondary",
        className,
      )}
    >
      <StateDot state={state} />
      {t(`remoteHosts.syncStates.${state}`)}
    </span>
  );
}

/** The one-time "Backup is now Sync" notice (SID-109). */
export function SyncNotice({ onDismiss }: { onDismiss: () => void }) {
  const { t } = useTranslation();

  return (
    <section
      role="note"
      className="app-panel bg-accent/5 flex items-start gap-3 border-accent-border p-4"
    >
      <div className="min-w-0 flex-1">
        <h2 className="text-[14px] font-semibold text-primary">{t("sync.notice.title")}</h2>
        <ul className="mt-1 list-disc space-y-0.5 pl-5 text-[13px] text-secondary">
          <li>{t("sync.notice.hosts")}</li>
          <li>{t("sync.notice.presets")}</li>
          <li>{t("sync.notice.agents")}</li>
        </ul>
      </div>
      <button
        type="button"
        onClick={onDismiss}
        className="rounded p-1 text-muted hover:text-secondary"
        title={t("sync.notice.dismiss")}
      >
        <X className="h-4 w-4" />
      </button>
    </section>
  );
}
