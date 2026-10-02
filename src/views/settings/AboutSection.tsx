import { openUrl } from "@tauri-apps/plugin-opener";
import { check as checkUpdater } from "@tauri-apps/plugin-updater";
import {
  BookOpen,
  Bug,
  Download,
  ExternalLink,
  FileArchive,
  Github,
  Loader2,
  RefreshCw,
  Settings2,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../../context/AppContext";
import { getErrorMessage } from "../../lib/error";
import * as api from "../../lib/tauri";
import { ACTION_BUTTON_CLASS, GITHUB_URL } from "./shared";

const IS_WINDOWS = navigator.userAgent.includes("Windows");

const IS_MACOS = navigator.userAgent.includes("Mac");

/** Platforms whose updater artifact can replace the running install.
 *
 *  Linux is excluded on purpose: only the AppImage can be updated in place,
 *  and a .deb/.rpm install is indistinguishable from it here, so those users
 *  keep the download link rather than a button that fails for half of them. */
const CAN_INSTALL_IN_APP = IS_WINDOWS || IS_MACOS;

const RESTART_TOAST_ID = "app-update-restart";

interface AboutSectionProps {
  reportingIssue: boolean;
  onReportIssue: () => void;
}

export function AboutSection({ reportingIssue, onReportIssue }: AboutSectionProps) {
  const { t } = useTranslation();
  const { openHelp, appUpdate, refreshAppUpdate } = useApp();
  const [openingGithub, setOpeningGithub] = useState(false);
  const [exportingLogs, setExportingLogs] = useState(false);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installing, setInstalling] = useState(false);

  const handleOpenGithub = async () => {
    try {
      setOpeningGithub(true);
      await openUrl(GITHUB_URL);
    } catch (error) {
      console.error("Failed to open GitHub repository", error);
      toast.error(t("common.error"));
    } finally {
      setOpeningGithub(false);
    }
  };

  const handleExportLogs = async () => {
    setExportingLogs(true);

    try {
      const result = await api.exportLogsZip();
      toast.success(t("settings.exportLogsDone", { count: result.file_count }), {
        description: result.zip_path,
      });
    } catch (error) {
      console.error("Failed to export logs", error);
      toast.error(t("settings.exportLogsFailed"));
    } finally {
      setExportingLogs(false);
    }
  };

  const handleCheckUpdate = async () => {
    setCheckingUpdate(true);

    try {
      const info = await refreshAppUpdate();

      if (info.has_update) {
        toast.info(t("settings.updateAvailable", { version: info.latest_version }));
      } else {
        toast.success(t("settings.noUpdate"));
      }
    } catch {
      toast.error(t("settings.updateError"));
    } finally {
      setCheckingUpdate(false);
    }
  };

  const handleAutoUpdate = async () => {
    setInstalling(true);

    try {
      // Read-only image or Gatekeeper-translocated copy: the updater would
      // download the whole bundle and only then fail to swap it, so stop first
      // and say what to do instead.
      const blocker = await api.updateInstallBlocker();

      if (blocker) {
        toast.error(t("settings.updateRelocate"));

        return;
      }

      // The updater plugin does not inherit the app's proxy setting the way
      // `check_app_update` does. Without this, a user behind a proxy is told a
      // new version exists and then cannot install it. The proxy given to
      // check() is carried through to the download. The update is this
      // computer's, so is the proxy, even while a host is active.
      const proxy = (await api.getLocalSettings("proxy_url")) || undefined;
      const update = await checkUpdater(proxy ? { proxy } : undefined);

      if (!update) {
        toast.success(t("settings.noUpdate"));

        return;
      }

      toast.info(t("settings.installing"));
      await update.downloadAndInstall();
      // Installing was the user's choice; restarting is a second one. Offered
      // as a toast action rather than a modal so a stray keypress cannot end
      // the session mid-task, and it stays up until acted on.
      toast.success(t("settings.restartToApply"), {
        id: RESTART_TOAST_ID,
        duration: Infinity,
        action: {
          label: t("settings.restartNow"),
          onClick: () => {
            api.restartApp().catch((err) => {
              toast.error(getErrorMessage(err, t("common.error")));
            });
          },
        },
      });
    } catch (err) {
      console.error("In-app update failed:", err);
      toast.error(t("settings.updateError"));

      if (appUpdate?.release_url) {
        await openUrl(appUpdate.release_url);
      }
    } finally {
      setInstalling(false);
    }
  };

  return (
    <section>
      <h2 className="app-section-title mb-3">{t("settings.about")}</h2>
      <div className="app-panel flex flex-wrap items-start justify-between gap-3 p-4">
        <div className="flex min-w-[260px] flex-1 items-center gap-3">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-border bg-surface-hover">
            <Settings2 className="h-4 w-4 text-accent" />
          </div>
          <div>
            <h3 className="text-[13px] font-semibold text-primary">{t("settings.version")}</h3>
            <p className="text-[13px] text-muted">
              {t("settings.tagline")}
              {appUpdate?.has_update && (
                <span className="ml-2 font-medium text-amber-500">
                  {t("settings.updateAvailable", { version: appUpdate.latest_version })}
                </span>
              )}
            </p>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          {appUpdate?.has_update ? (
            CAN_INSTALL_IN_APP ? (
              <>
                <button
                  type="button"
                  onClick={handleAutoUpdate}
                  disabled={installing}
                  className={`${ACTION_BUTTON_CLASS} border-accent bg-accent text-white hover:opacity-90`}
                >
                  {installing ? (
                    <Loader2 className="h-3 w-3 animate-spin" />
                  ) : (
                    <Download className="h-3 w-3" />
                  )}
                  {installing ? t("settings.installing") : t("settings.installUpdate")}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    openUrl(appUpdate.release_url).catch(() => {});
                  }}
                  className={`${ACTION_BUTTON_CLASS} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
                >
                  <ExternalLink className="h-3 w-3" /> {t("settings.download")}
                </button>
              </>
            ) : (
              <button
                type="button"
                onClick={() => {
                  openUrl(appUpdate.release_url).catch(() => {});
                }}
                className={`${ACTION_BUTTON_CLASS} border-accent bg-accent text-white hover:opacity-90`}
              >
                <Download className="h-3 w-3" /> {t("settings.download")}
              </button>
            )
          ) : (
            <button
              type="button"
              onClick={handleCheckUpdate}
              disabled={checkingUpdate}
              className={`${ACTION_BUTTON_CLASS} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
            >
              {checkingUpdate ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <RefreshCw className="h-3 w-3" />
              )}
              {checkingUpdate ? t("settings.checking") : t("settings.checkUpdate")}
            </button>
          )}
          <button
            type="button"
            onClick={openHelp}
            className={`${ACTION_BUTTON_CLASS} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
          >
            <BookOpen className="h-3 w-3" /> {t("settings.help")}
          </button>
          <button
            type="button"
            onClick={onReportIssue}
            disabled={reportingIssue}
            title={t("settings.reportIssueHint")}
            className={`${ACTION_BUTTON_CLASS} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
          >
            {reportingIssue ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <Bug className="h-3 w-3" />
            )}
            {t("settings.reportIssue")}
          </button>
          <button
            type="button"
            onClick={handleExportLogs}
            disabled={exportingLogs}
            title={t("settings.exportLogsHint")}
            className={`${ACTION_BUTTON_CLASS} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
          >
            {exportingLogs ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <FileArchive className="h-3 w-3" />
            )}
            {t("settings.exportLogs")}
          </button>
          <button
            type="button"
            onClick={handleOpenGithub}
            disabled={openingGithub}
            className={`${ACTION_BUTTON_CLASS} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
          >
            <Github className="h-3 w-3" /> GitHub
          </button>
        </div>
      </div>
    </section>
  );
}
