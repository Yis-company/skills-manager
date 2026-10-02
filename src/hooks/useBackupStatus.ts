import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { mapGitErrorMessage } from "../lib/gitErrors";
import * as api from "../lib/tauri";
import type { GitBackupSizeReport, GitBackupStatus, GitBackupVersion } from "../lib/tauri";

/**
 * The backup page's view of this computer's backup: repo status, snapshots,
 * pending conflicts and the saved backup settings. Loaded once on mount
 * (after moving any URL-embedded token to the keychain) and kept current by
 * the background auto-backup's `backup-auto-completed` event.
 */
export function useBackupStatus() {
  const { t } = useTranslation();
  const { refreshManagedSkills, refreshPresets } = useApp();
  const [gitStatus, setGitStatus] = useState<GitBackupStatus | null>(null);
  const [remoteInput, setRemoteInput] = useState("");
  const [remoteConfig, setRemoteConfig] = useState("");
  const [versions, setVersions] = useState<GitBackupVersion[]>([]);
  const [versionsLoading, setVersionsLoading] = useState(false);
  const [backupError, setBackupError] = useState<null | string>(null);
  const [sizeReport, setSizeReport] = useState<GitBackupSizeReport | null>(null);
  const [deviceName, setDeviceName] = useState("");
  const [autoBackupEnabled, setAutoBackupEnabled] = useState(true);
  const [pendingConflicts, setPendingConflicts] = useState<api.PendingConflict[]>([]);
  // §3.1 disconnect matrix rows 2–3 + reconnect guidance after revocation.
  const [authMethod, setAuthMethod] = useState("");
  const [backupErrorRaw, setBackupErrorRaw] = useState("");

  const mapGitError = useCallback((cause: unknown) => mapGitErrorMessage(cause, t), [t]);

  const refreshGitStatus = useCallback(async (fetchRemote = false) => {
    try {
      if (fetchRemote) {
        await api.gitBackupFetch().catch(() => {});
      }

      const status = await api.gitBackupStatus();
      setGitStatus(status);

      return status;
    } catch {
      return null;
    }
  }, []);

  // Callers mark `versionsLoading` before starting the load.
  const loadVersions = useCallback(
    () =>
      api
        .gitBackupListVersions(50)
        .catch(() => [])
        .then((items) => {
          setVersions(items);
          setVersionsLoading(false);
        }),
    [],
  );

  const refreshVersions = useCallback(() => {
    setVersionsLoading(true);

    return loadVersions();
  }, [loadVersions]);

  // "Needs attention" sync conflicts (merge-engine design §4).
  const refreshPendingConflicts = useCallback(async () => {
    try {
      setPendingConflicts(await api.gitBackupPendingConflicts());
    } catch {
      setPendingConflicts([]);
    }
  }, []);

  useEffect(() => {
    void (async () => {
      // §3.7: move any token embedded in the remote URL into the OS keychain
      // before the URL is read or displayed. Idempotent and best-effort —
      // offline machines simply retry on the next visit.
      const migrated = await api.gitBackupMigrateCredentials().catch(() => null);

      if (migrated) {
        toast.info(t("backup.credentialsMigrated"));
      }

      api
        .backupDeviceName()
        .then(setDeviceName)
        .catch(() => {});
      api
        .getSettings("backup_auto_enabled")
        .then((v) => {
          const normalized = (v ?? "").trim().toLowerCase();
          setAutoBackupEnabled(!["off", "false", "0", "no"].includes(normalized));
        })
        .catch(() => {});
      // A failed automatic backup persists until a backup succeeds (§3.4) —
      // resurface it when the page opens.
      api
        .getSettings("backup_last_auto_error")
        .then((v) => {
          const raw = (v ?? "").trim();

          if (raw) {
            setBackupError(mapGitError(raw));
            setBackupErrorRaw(raw);
          }
        })
        .catch(() => {});
      api
        .getSettings("github_auth_method")
        .then((v) => setAuthMethod((v ?? "").trim()))
        .catch(() => {});

      const savedRemote =
        (await api.getSettings("git_backup_remote_url").catch(() => null))?.trim() || "";

      setRemoteInput(savedRemote);
      setRemoteConfig(savedRemote);
      const status = await refreshGitStatus(true);

      if (status?.is_repo) {
        await refreshVersions();
        void refreshPendingConflicts();
        api
          .gitBackupSizeReport()
          .then(setSizeReport)
          .catch(() => setSizeReport(null));
      }
    })();
  }, [mapGitError, refreshGitStatus, refreshPendingConflicts, refreshVersions, t]);

  // Live updates from the background auto-backup rounds.
  useEffect(() => {
    const unlistenPromise = listen<{ ok: boolean; pending: boolean; error: null | string }>(
      "backup-auto-completed",
      (event) => {
        setBackupError(event.payload.error ? mapGitError(event.payload.error) : null);
        setBackupErrorRaw(event.payload.error ?? "");
        void refreshGitStatus();
        void refreshVersions();
        void refreshPendingConflicts();

        // A completed background round may have merged remote changes into the
        // library (multi-device auto-sync reindexes skills + presets into the
        // DB). The merge is an app-internal write, so the file watcher's
        // self-write mute can swallow it — refresh here so the sidebar reflects
        // remote presets/skills without waiting for a restart (#302).
        if (event.payload.ok && !event.payload.pending) {
          void refreshManagedSkills();
          void refreshPresets();
        }
      },
    );

    return () => {
      void unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [
    mapGitError,
    refreshGitStatus,
    refreshPendingConflicts,
    refreshVersions,
    refreshManagedSkills,
    refreshPresets,
  ]);

  const isRepo = gitStatus?.is_repo;
  const [prevIsRepo, setPrevIsRepo] = useState(isRepo);

  if (isRepo !== prevIsRepo) {
    setPrevIsRepo(isRepo);

    if (isRepo) setVersionsLoading(true);
    else setVersions([]);
  }

  useEffect(() => {
    if (isRepo) void loadVersions();
  }, [isRepo, loadVersions]);

  return {
    gitStatus,
    refreshGitStatus,
    remoteInput,
    setRemoteInput,
    remoteConfig,
    setRemoteConfig,
    versions,
    versionsLoading,
    refreshVersions,
    pendingConflicts,
    refreshPendingConflicts,
    sizeReport,
    backupError,
    setBackupError,
    backupErrorRaw,
    setBackupErrorRaw,
    deviceName,
    setDeviceName,
    autoBackupEnabled,
    setAutoBackupEnabled,
    authMethod,
    setAuthMethod,
  };
}
