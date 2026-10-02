import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, Download, Loader2, X } from "lucide-react";
import { type KeyboardEvent, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { getErrorMessage } from "../lib/error";
import { getActiveHostId } from "../lib/hostCall";
import * as api from "../lib/tauri";

type Stage = "checking" | "downloading" | "installing" | "reconnecting";

interface Props {
  hostId: null | string;
  host: api.RemoteHost | undefined;
  refreshRemoteHosts: () => Promise<void>;
  switchHost: (hostId: null | string, allowDuringRemoteUpdate?: boolean) => Promise<boolean>;
  getHostSwitchToken: () => number;
  onClose: () => void;
  onBusyChange: (hostId: null | string) => void;
}

export function RemoteCliUpdateDialog({
  hostId,
  host,
  refreshRemoteHosts,
  switchHost,
  getHostSwitchToken,
  onClose,
  onBusyChange,
}: Props) {
  const { t } = useTranslation();
  const [probe, setProbe] = useState<api.RemoteProbe | null>(null);
  const [stage, setStage] = useState<null | Stage>(null);
  const [error, setError] = useState<null | string>(null);
  const [installed, setInstalled] = useState(false);
  const [probeLoading, setProbeLoading] = useState(hostId !== null);
  const [shownHostId, setShownHostId] = useState(hostId);
  const runRef = useRef(false);
  const generationRef = useRef(0);
  const selectedHostRef = useRef<null | string>(null);
  const installSwitchTokenRef = useRef<null | number>(null);
  const dialogRef = useRef<HTMLElement>(null);
  const busy = stage !== null;

  // Each opened host starts from a fresh check.
  if (hostId !== shownHostId) {
    setShownHostId(hostId);
    setProbe(null);
    setInstalled(false);
    setProbeLoading(hostId !== null);
    setStage(null);
    setError(null);
  }

  const loadProbe = useCallback(
    (id: string, generation: number, isCurrent: () => boolean = () => true) => {
      const current = () => generationRef.current === generation && isCurrent();

      return api
        .remoteHostProbe(id)
        .then(
          (result) => {
            if (current()) setProbe(result);
          },
          (e) => {
            if (current()) setError(getErrorMessage(e, t("common.error")));
          },
        )
        .finally(() => {
          if (current()) setProbeLoading(false);
        });
    },
    [t],
  );

  useEffect(() => {
    const generation = ++generationRef.current;

    if (!hostId) return;
    let active = true;

    const previousFocus =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    requestAnimationFrame(() =>
      dialogRef.current?.querySelector<HTMLElement>("button:not([disabled])")?.focus(),
    );
    void loadProbe(hostId, generation, () => active);

    return () => {
      active = false;
      previousFocus?.focus();
    };
  }, [hostId, loadProbe]);

  useEffect(() => {
    if (!hostId) return;

    const unlisten = listen<api.RemoteCliInstallProgress>(
      "remote-cli-install-progress",
      ({ payload }) => {
        if (payload.host_id === hostId && runRef.current) setStage(payload.stage);
      },
    );

    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [hostId]);

  const close = () => {
    if (!busy) onClose();
  };

  const handleDialogKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close();

      return;
    }

    if (event.key !== "Tab" || !dialogRef.current) return;
    const items = [...dialogRef.current.querySelectorAll<HTMLElement>("button:not([disabled])")];

    if (items.length === 0) {
      event.preventDefault();
      dialogRef.current.focus();

      return;
    }

    const first = items[0];
    const last = items[items.length - 1];

    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  const run = async () => {
    if (!hostId || runRef.current || !probe || probe.compatible || probeLoading) return;
    const selectedHostId = getActiveHostId();
    const switchToken = getHostSwitchToken();
    selectedHostRef.current = selectedHostId;
    installSwitchTokenRef.current = switchToken;
    runRef.current = true;
    setError(null);
    onBusyChange(hostId);

    try {
      setStage("checking");
      await api.remoteHostInstallCli(hostId);
      setInstalled(true);
      await refreshRemoteHosts();
      setStage("reconnecting");

      if (getActiveHostId() !== selectedHostId || getHostSwitchToken() !== switchToken) {
        setError(t("remoteHosts.installedButNotReconnected"));

        return;
      }

      if (selectedHostId === hostId) {
        await api.remoteHostDisconnect();

        if (getActiveHostId() !== selectedHostId || getHostSwitchToken() !== switchToken) {
          setError(t("remoteHosts.installedButNotReconnected"));

          return;
        }
      }

      const connected = await switchHost(hostId, true);

      if (!connected) {
        if (getHostSwitchToken() === switchToken + 1) {
          selectedHostRef.current = getActiveHostId();
          installSwitchTokenRef.current = getHostSwitchToken();
        }

        setError(t("remoteHosts.installedReconnectFailed"));

        return;
      }

      onClose();
    } catch (e) {
      setError(getErrorMessage(e, t("common.error")));
    } finally {
      runRef.current = false;
      setStage(null);
      onBusyChange(null);
    }
  };

  if (!hostId) return null;
  const upToDate = Boolean(probe?.compatible);

  const actionLabel = installed
    ? t("remoteHosts.reconnect")
    : t("remoteHosts.installMatchingReconnect");

  const retry = async () => {
    if (!hostId || runRef.current) return;

    if (installed) {
      const selectedHostId = selectedHostRef.current;
      const switchToken = getHostSwitchToken();

      if (getActiveHostId() !== selectedHostId || switchToken !== installSwitchTokenRef.current) {
        setError(t("remoteHosts.installedButNotReconnected"));

        return;
      }

      runRef.current = true;
      onBusyChange(hostId);
      setStage("reconnecting");

      try {
        if (selectedHostId === hostId) {
          await api.remoteHostDisconnect();

          if (getActiveHostId() !== selectedHostId || getHostSwitchToken() !== switchToken) {
            setError(t("remoteHosts.installedButNotReconnected"));

            return;
          }
        }

        if (getActiveHostId() !== selectedHostId || getHostSwitchToken() !== switchToken) {
          setError(t("remoteHosts.installedButNotReconnected"));

          return;
        }

        if (!(await switchHost(hostId, true))) {
          if (getHostSwitchToken() === switchToken + 1)
            installSwitchTokenRef.current = getHostSwitchToken();
          setError(t("remoteHosts.installedReconnectFailed"));
        } else onClose();
      } catch (e) {
        setError(getErrorMessage(e, t("common.error")));
      } finally {
        runRef.current = false;
        setStage(null);
        onBusyChange(null);
      }

      return;
    }

    if (probe?.compatible) return;

    if (probe) {
      void run();

      return;
    }

    const generation = generationRef.current;
    setProbe(null);
    setError(null);
    setProbeLoading(true);
    await loadProbe(hostId, generation);
  };

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center" role="presentation">
      <button
        aria-label={t("common.cancel")}
        className="absolute inset-0 bg-black/70 backdrop-blur-sm"
        onClick={close}
        disabled={busy}
      />
      <section
        ref={dialogRef}
        tabIndex={-1}
        onKeyDown={handleDialogKeyDown}
        role="dialog"
        aria-modal="true"
        aria-labelledby="remote-cli-update-title"
        className="relative mx-4 w-full max-w-lg rounded-xl border border-border bg-surface p-5 shadow-2xl outline-none"
      >
        <header className="mb-4 flex items-start justify-between gap-3">
          <div>
            <h2
              id="remote-cli-update-title"
              className="flex items-center gap-2 text-[14px] font-semibold text-primary"
            >
              <AlertTriangle className="h-4 w-4 text-amber-500" />
              {t("remoteHosts.updateTitle")}
            </h2>
            <p
              role={upToDate ? "status" : undefined}
              className="mt-1 text-[12px] leading-relaxed text-muted"
            >
              {probeLoading
                ? t("remoteHosts.checkingDescription")
                : upToDate
                  ? t("remoteHosts.upToDate", { version: probe?.version })
                  : t("remoteHosts.updateDescription")}
            </p>
          </div>
          <button
            onClick={close}
            disabled={busy}
            aria-label={t("common.cancel")}
            className="rounded p-1 text-muted hover:text-secondary disabled:opacity-50"
          >
            <X className="h-4 w-4" />
          </button>
        </header>
        <dl className="mb-4 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[12px]">
          <dt className="text-muted">{t("remoteHosts.hostLabel")}</dt>
          <dd className="text-primary">{host?.name ?? hostId}</dd>
          <dt className="text-muted">{t("remoteHosts.currentVersion")}</dt>
          <dd className="font-mono text-primary">{probe?.version ?? "—"}</dd>
          <dt className="text-muted">{t("remoteHosts.requiredVersion")}</dt>
          <dd className="font-mono text-primary">{probe?.app_version ?? "—"}</dd>
          <dt className="text-muted">{t("remoteHosts.source")}</dt>
          <dd className="text-primary">Yis-company/skills-manager</dd>
          {host?.cli_path && (
            <>
              <dt className="text-muted">{t("remoteHosts.customPath")}</dt>
              <dd className="break-all font-mono text-tertiary">{host.cli_path}</dd>
            </>
          )}
        </dl>
        <p className="mb-4 text-[12px] leading-relaxed text-tertiary">
          {t("remoteHosts.cliOnlyNotice")}
        </p>
        {(stage || probeLoading) && (
          <p role="status" className="mb-3 flex items-center gap-2 text-[12px] text-accent">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            {t(`remoteHosts.stage.${stage ?? "checking"}`)}
          </p>
        )}
        {error && (
          <p
            role="alert"
            className="mb-3 rounded-md border border-red-500/30 bg-red-500/10 p-2 text-[12px] text-red-500"
          >
            {installed ? `${t("remoteHosts.installedLabel")} ${error}` : error}
          </p>
        )}
        <footer className="flex justify-end gap-2">
          <button onClick={close} disabled={busy} className="app-button-secondary">
            {upToDate ? t("remoteHosts.close") : t("common.cancel")}
          </button>
          {!upToDate && (
            <button
              onClick={() => void retry()}
              disabled={busy || probeLoading || (!probe && !error)}
              className="app-button-secondary gap-1.5 border-accent bg-accent text-white disabled:opacity-50"
            >
              {busy ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Download className="h-3.5 w-3.5" />
              )}
              {busy ? t(`remoteHosts.stage.${stage}`) : error ? t("common.retry") : actionLabel}
            </button>
          )}
        </footer>
      </section>
    </div>
  );
}
