import { AlertTriangle, LayoutGrid, Plus, RefreshCw } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useAddHost } from "../hooks/useAddHost";
import { type SyncHost, useSyncScreen } from "../hooks/useSyncScreen";
import * as api from "../lib/tauri";
import { cn } from "../utils";
import { AddHost } from "./sync/AddHost";
import { HostPane } from "./sync/HostPane";
import { Overview } from "./sync/Overview";
import { StateDot, SyncNotice } from "./sync/parts";
import { Review } from "./sync/Review";

/** A host's key in the list: its id, or "local" for this computer. */
type Selection = "add" | "overview" | "review" | string;

const NOTICE_SETTING = "backup_sync_notice_dismissed";

const hostKey = (host: SyncHost) => host.id ?? "local";

/**
 * Sync (SID-112, variant C): a master-detail split. The left column lists
 * Overview, Needs review, every host and Add host; the right pane shows the
 * selection.
 */
export function Sync() {
  const { t } = useTranslation();
  const screen = useSyncScreen();
  const [selection, setSelection] = useState<null | Selection>(null);
  const [noticeOpen, setNoticeOpen] = useState(false);

  const add = useAddHost((hostId) => {
    setSelection(hostId);
    void screen.refresh();
  });

  useEffect(() => {
    api
      .getSettings(NOTICE_SETTING)
      .then((v) => setNoticeOpen(v !== "1"))
      .catch(() => {});
  }, []);

  const dismissNotice = () => {
    setNoticeOpen(false);
    void api.setSettings(NOTICE_SETTING, "1");
  };

  const thisHost = screen.hosts[0];
  // This computer starts on its own setup until it has a Sync remote.
  const current = selection ?? (thisHost.inSync ? "overview" : "local");
  const host = screen.hosts.find((h) => hostKey(h) === current);
  const conflicts = screen.local?.conflicts ?? [];
  const settingUp = add.run && !add.run.done ? add.run : null;

  const navItem = (key: Selection, label: ReactNode, extra?: ReactNode) => (
    <button
      key={key}
      type="button"
      onClick={() => setSelection(key)}
      className={cn(
        "flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[13px]",
        current === key
          ? "bg-surface-active text-primary"
          : "text-secondary hover:bg-surface-hover",
      )}
    >
      {label}
      <span className="ml-auto">{extra}</span>
    </button>
  );

  return (
    <div className="app-page !gap-4">
      <div className="app-page-header">
        <h1 className="app-page-title">{t("backup.title")}</h1>
        <p className="app-page-subtitle">
          {screen.local?.remote ? (
            <span className="font-mono text-[12px]">{screen.local.remote}</span>
          ) : (
            t("backup.subtitle")
          )}
        </p>
      </div>
      {noticeOpen && thisHost.inSync && <SyncNotice onDismiss={dismissNotice} />}

      <div className="grid min-h-[560px] grid-cols-[240px_minmax(0,1fr)] overflow-hidden rounded-xl border border-border-subtle">
        <aside className="space-y-0.5 border-r border-border-subtle bg-bg-secondary p-2">
          {navItem(
            "overview",
            <>
              <LayoutGrid className="h-3.5 w-3.5 text-muted" />
              {t("sync.overview")}
            </>,
          )}
          {navItem(
            "review",
            <>
              <AlertTriangle className="h-3.5 w-3.5 text-amber-500" />
              {t("sync.needsReview")}
            </>,
            conflicts.length > 0 && (
              <span className="app-badge !px-1.5 !py-0 !text-[11px]">{conflicts.length}</span>
            ),
          )}
          <div className="app-section-title px-2.5 pb-1 pt-4">{t("sync.hosts")}</div>
          {screen.hosts.map((h) => {
            const flags = h.status?.edited_here.length ?? 0;

            return navItem(
              hostKey(h),
              <>
                {settingUp && settingUp.hostId === h.id ? (
                  <RefreshCw className="h-3 w-3 animate-spin text-accent" />
                ) : (
                  <StateDot state={h.state} />
                )}
                <span className="truncate">{h.name || t("sync.thisHost")}</span>
                {h.id === null && (
                  <span className="text-[11px] text-faint">{t("sync.thisHost")}</span>
                )}
              </>,
              flags > 0 && (
                <span className="text-[11px] text-amber-600 dark:text-amber-400">{flags}</span>
              ),
            );
          })}
          {settingUp &&
            !settingUp.hostId &&
            navItem(
              "add",
              <>
                <RefreshCw className="h-3 w-3 animate-spin text-accent" />
                <span className="truncate">{settingUp.name}</span>
                <span className="text-[11px] text-faint">{t("sync.add.settingUpShort")}</span>
              </>,
            )}
          <button
            type="button"
            onClick={() => setSelection("add")}
            className="mt-1 flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-[13px] text-muted hover:bg-surface-hover hover:text-secondary"
          >
            <Plus className="h-3.5 w-3.5" />
            {t("sync.add.title")}
          </button>
        </aside>

        <main className="min-w-0 overflow-y-auto bg-surface p-5">
          {current === "overview" && (
            <Overview
              hosts={screen.hosts.filter((h) => h.inSync)}
              history={screen.history}
              onOpen={(h) => setSelection(hostKey(h))}
            />
          )}
          {current === "review" && <Review conflicts={conflicts} onResolved={screen.refresh} />}
          {(current === "add" || (settingUp && settingUp.hostId === current)) && (
            <AddHost add={add} />
          )}
          {host && !(settingUp && settingUp.hostId === current) && (
            <HostPane
              key={hostKey(host)}
              host={host}
              history={screen.history}
              onChanged={screen.refresh}
              onRemoved={() => setSelection("overview")}
            />
          )}
        </main>
      </div>
    </div>
  );
}
