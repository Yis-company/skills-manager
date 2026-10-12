import { RefreshCw, RotateCcw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { ConfirmDialog } from "../../components/ConfirmDialog";
import { RemoveHostDialog } from "../../components/RemoveHostDialog";
import { useApp } from "../../context/AppContext";
import type { SyncHost } from "../../hooks/useSyncScreen";
import { formatWhen } from "../../lib/backupFormat";
import { getErrorMessage } from "../../lib/error";
import * as api from "../../lib/tauri";
import { cn } from "../../utils";
import { ThisHostSettings } from "../Backup";
import { StatePill } from "./parts";

type Tab = "details" | "history" | "log";

/** One host: its state and actions, then its agent log, its changes and details. */
export function HostPane({
  host,
  history,
  onChanged,
  onRemoved,
}: {
  host: SyncHost;
  history: api.SyncHistoryEntry[];
  onChanged: () => Promise<void>;
  onRemoved: () => void;
}) {
  const { t } = useTranslation();
  const { switchHost } = useApp();
  const [tab, setTab] = useState<Tab>(host.id === null && !host.inSync ? "details" : "log");
  const [syncing, setSyncing] = useState(false);

  const run = async (command: string, args: api.SyncCallArgs = {}) => {
    try {
      await api.syncCall(host.id, command, args);
      await onChanged();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    }
  };

  const syncNow = async () => {
    setSyncing(true);
    await run("sync_now");
    setSyncing(false);
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h2 className="text-[18px] font-semibold text-primary">
              {host.name || t("sync.thisHost")}
            </h2>
            <StatePill state={syncing ? "syncing" : host.state} />
          </div>
          <div className="mt-0.5 font-mono text-[12px] text-faint">
            {[
              host.target ?? t("sync.thisComputer"),
              host.status?.version,
              host.status?.last_sync
                ? t("sync.lastSync", { when: formatWhen(host.status.last_sync) })
                : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          </div>
          {host.reason && <div className="mt-1 text-[12px] text-muted">{host.reason}</div>}
        </div>
        {host.inSync && (
          <div className="flex flex-wrap gap-1.5">
            <button
              type="button"
              className="app-button-secondary !py-1.5"
              disabled={syncing}
              onClick={syncNow}
            >
              <RefreshCw className={cn("h-3.5 w-3.5", syncing && "animate-spin")} />
              {t("sync.syncNow")}
            </button>
            <button
              type="button"
              className="app-button-secondary !py-1.5"
              onClick={() => run(host.state === "paused" ? "sync_resume" : "sync_pause")}
            >
              {host.state === "paused" ? t("sync.resume") : t("sync.pause")}
            </button>
            {host.id !== null && (
              <button
                type="button"
                className="app-button-secondary !py-1.5"
                onClick={() => void switchHost(host.id)}
              >
                {t("sync.openHost")}
              </button>
            )}
          </div>
        )}
      </div>

      <div className="flex gap-4 border-b border-border-subtle text-[13px]">
        {(["log", "history", "details"] as const).map((key) => (
          <button
            key={key}
            type="button"
            onClick={() => setTab(key)}
            className={cn(
              "-mb-px border-b-2 pb-2",
              tab === key
                ? "border-accent font-medium text-primary"
                : "border-transparent text-muted hover:text-secondary",
            )}
          >
            {t(`sync.tabs.${key}`)}
          </button>
        ))}
      </div>

      {tab === "log" && <AgentLog host={host} onChanged={onChanged} />}
      {tab === "history" && (
        <Changes entries={history.filter((e) => e.host === host.name)} onChanged={onChanged} />
      )}
      {tab === "details" && <Details host={host} onChanged={onChanged} onRemoved={onRemoved} />}
    </div>
  );
}

function AgentLog({ host, onChanged }: { host: SyncHost; onChanged: () => Promise<void> }) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<api.AgentLogEntry[] | null>(null);
  const edited = host.status?.edited_here ?? [];

  const load = useCallback(() => {
    api
      .syncCall<api.AgentLogEntry[]>(host.id, "sync_agent_log")
      .then(setEntries)
      .catch(() => setEntries([]));
  }, [host.id]);

  useEffect(load, [load]);

  const choose = async (copy: api.EditedCopy, keepMine: boolean) => {
    try {
      await api.syncCall(host.id, "resolve_edited_copy", {
        skillId: copy.skill_id,
        tool: copy.agent,
        keepMine,
      });
      await onChanged();
      load();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    }
  };

  return (
    <div className="space-y-4 text-[13px]">
      {edited.length > 0 && (
        <ul className="space-y-1.5">
          {edited.map((copy) => (
            <li
              key={`${copy.skill_id}:${copy.agent}`}
              className="text-amber-600 dark:text-amber-400"
            >
              {t(copy.left ? "sync.log.editedLeft" : "sync.log.edited", {
                item: copy.name,
                agent: copy.agent,
              })}
              <span className="ml-2 inline-flex gap-1">
                <button
                  type="button"
                  className="text-[12px] font-medium text-accent hover:underline"
                  onClick={() => choose(copy, false)}
                >
                  {t("sync.log.useSync")}
                </button>
                ·
                <button
                  type="button"
                  className="text-[12px] font-medium text-accent hover:underline"
                  onClick={() => choose(copy, true)}
                >
                  {t("sync.log.keepMine")}
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
      {entries !== null && entries.length === 0 && edited.length === 0 && (
        <p className="text-muted">{t("sync.log.empty")}</p>
      )}
      <ul className="space-y-1.5">
        {(entries ?? []).map((entry) => (
          <li
            key={entry.id}
            className={cn("text-secondary", !entry.success && "text-red-600 dark:text-red-300")}
          >
            <span className="mr-3 text-[12px] text-faint">{formatWhen(entry.ts * 1000)}</span>
            {t(`sync.log.${entry.success ? entry.action : `${entry.action}_failed`}`, {
              item: entry.skill_name ?? entry.skill_id ?? "",
              agent: entry.tool ?? "",
              detail: entry.detail ?? "",
              defaultValue: entry.action,
            })}
          </li>
        ))}
      </ul>
    </div>
  );
}

function Changes({
  entries,
  onChanged,
}: {
  entries: api.SyncHistoryEntry[];
  onChanged: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [restoring, setRestoring] = useState<api.SyncHistoryEntry | null>(null);

  const restore = async () => {
    if (!restoring) return;

    try {
      await api.gitBackupRestoreVersion(restoring.commit);
      toast.success(
        t("mySkills.gitVersionRestoreSuccess", { when: formatWhen(restoring.committed_at) }),
      );
      toast.info(t("backup.restoreSafetyPoint"));
      await onChanged();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    }
  };

  return (
    <>
      <ul className="divide-y divide-border-subtle text-[13px]">
        {entries.length === 0 && <li className="py-3 text-muted">{t("sync.noChanges")}</li>}
        {entries.map((entry) => (
          <li key={entry.commit} className="flex items-center gap-3 py-2">
            <span className="w-36 shrink-0 text-[12px] text-faint">
              {formatWhen(entry.committed_at)}
            </span>
            <span className="min-w-0 flex-1 truncate text-secondary">{entry.message}</span>
            <button
              type="button"
              onClick={() => setRestoring(entry)}
              className="inline-flex items-center gap-1 text-[12px] font-medium text-accent hover:underline"
            >
              <RotateCcw className="h-3 w-3" />
              {t("mySkills.gitVersionRestore")}
            </button>
          </li>
        ))}
      </ul>
      <ConfirmDialog
        open={restoring !== null}
        title={t("mySkills.gitVersionRestoreTitle")}
        message={t("mySkills.gitVersionRestoreConfirm", {
          when: formatWhen(restoring?.committed_at ?? null) ?? "",
        })}
        tone="warning"
        confirmLabel={t("mySkills.gitVersionRestore")}
        onClose={() => setRestoring(null)}
        onConfirm={restore}
      />
    </>
  );
}

function Details({
  host,
  onChanged,
  onRemoved,
}: {
  host: SyncHost;
  onChanged: () => Promise<void>;
  onRemoved: () => void;
}) {
  const { t } = useTranslation();
  const [copies, setCopies] = useState<string[]>([]);
  const [undoing, setUndoing] = useState<null | string>(null);
  const [removing, setRemoving] = useState(false);

  const load = useCallback(() => {
    api
      .syncCall<string[]>(host.id, "sync_pre_join_copies")
      .then(setCopies)
      .catch(() => setCopies([]));
  }, [host.id]);

  useEffect(load, [load]);

  const deleteCopy = async (name: string) => {
    try {
      await api.syncCall(host.id, "sync_delete_pre_join_copy", { name });
      load();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    }
  };

  const undoJoin = async () => {
    try {
      await api.syncCall(host.id, "sync_undo_join", { copy: undoing });
      toast.success(t("sync.details.undone"));
      await onChanged();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    }
  };

  const latestCopy = copies.at(-1);

  return (
    <div className="space-y-5">
      <dl className="grid grid-cols-[160px_1fr] gap-y-2 text-[13px]">
        {host.id !== null && (
          <>
            <dt className="text-muted">{t("sync.details.runner")}</dt>
            <dd className="text-secondary">
              {host.timer ? t("sync.details.timer") : t("sync.details.onConnect")}
            </dd>
            <dt className="text-muted">{t("sync.details.access")}</dt>
            <dd className="text-secondary">
              {host.keyTitle
                ? t("sync.details.deployKey", { key: host.keyTitle })
                : t("sync.details.notInSync")}
            </dd>
          </>
        )}
        {latestCopy && (
          <>
            <dt className="text-muted">{t("sync.details.preJoinCopy")}</dt>
            <dd className="flex flex-wrap gap-3">
              <span className="font-mono text-[12px] text-faint">{latestCopy}</span>
              <button
                type="button"
                className="text-[13px] font-medium text-accent hover:underline"
                onClick={() => setUndoing(latestCopy)}
              >
                {t("sync.details.undoJoin")}
              </button>
              <button
                type="button"
                className="text-[13px] font-medium text-accent hover:underline"
                onClick={() => deleteCopy(latestCopy)}
              >
                {t("sync.details.deleteBackup")}
              </button>
            </dd>
          </>
        )}
        {host.id !== null && (
          <>
            <dt className="text-muted">{t("sync.details.dangerZone")}</dt>
            <dd>
              <button
                type="button"
                className="text-[13px] font-medium text-red-600 hover:underline dark:text-red-300"
                onClick={() => setRemoving(true)}
              >
                {t("sync.details.removeHost")}
              </button>
            </dd>
          </>
        )}
      </dl>
      {host.id === null && <ThisHostSettings />}
      <ConfirmDialog
        open={undoing !== null}
        title={t("sync.details.undoJoin")}
        message={t("sync.details.undoConfirm", { copy: undoing ?? "" })}
        tone="warning"
        confirmLabel={t("sync.details.undoJoin")}
        onClose={() => setUndoing(null)}
        onConfirm={undoJoin}
      />
      {host.id !== null && (
        <RemoveHostDialog
          host={removing ? { id: host.id, name: host.name } : null}
          onClose={() => setRemoving(false)}
          onRemoved={onRemoved}
        />
      )}
    </div>
  );
}
