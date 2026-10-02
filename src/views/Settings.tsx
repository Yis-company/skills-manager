import { Link, Navigate, useParams } from "@tanstack/react-router";
import { writeText as clipboardWriteText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AlertTriangle,
  Bot,
  Bug,
  GitBranch,
  Globe,
  Info,
  Library,
  Loader2,
  type LucideIcon,
  Server,
  Settings2,
  SlidersHorizontal,
} from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import * as api from "../lib/tauri";
import { cn } from "../utils";
import { AboutSection } from "./settings/AboutSection";
import { AgentsSection } from "./settings/AgentsSection";
import { BackupSection } from "./settings/BackupSection";
import {
  resolveSettingsCategory,
  SETTINGS_CATEGORIES,
  type SettingsCategory,
  settingsLink,
} from "./settings/categories";
import { GeneralSection } from "./settings/GeneralSection";
import { LibrarySection } from "./settings/LibrarySection";
import { NetworkSection } from "./settings/NetworkSection";
import { RemoteHostsSection } from "./settings/RemoteHostsSection";
import { ACTION_BUTTON_CLASS, GITHUB_URL } from "./settings/shared";

const CATEGORY_ICONS: Record<SettingsCategory, LucideIcon> = {
  general: SlidersHorizontal,
  agents: Bot,
  library: Library,
  backup: GitBranch,
  network: Globe,
  remote: Server,
  about: Info,
};

export function Settings() {
  const { t, i18n } = useTranslation();
  const { category: categoryParam } = useParams({ from: "/settings/{-$category}" });
  const { tools, appUpdate } = useApp();
  const [reportingIssue, setReportingIssue] = useState(false);
  const [lastPanic, setLastPanic] = useState<api.PanicInfo | null>(null);
  const [repoWarnings, setRepoWarnings] = useState<string[]>([]);

  useEffect(() => {
    api
      .checkLastPanic()
      .then(setLastPanic)
      .catch(() => {});
    api
      .getCentralRepoWarnings()
      .then(setRepoWarnings)
      .catch(() => {});
  }, []);

  const handleDismissPanic = async () => {
    try {
      await api.clearLastPanic();
    } catch (err) {
      console.warn("Failed to clear last_panic.log", err);
    }

    setLastPanic(null);
  };

  const handleReportIssue = async () => {
    setReportingIssue(true);

    try {
      const [info, logExcerpt, panicInfo] = await Promise.all([
        api.getDiagnosticInfo(),
        api.getRecentLogExcerpt().catch((err) => {
          console.warn("Failed to read log excerpt", err);

          return null;
        }),
        api.checkLastPanic().catch(() => null),
      ]);

      const enabledTools = tools.filter((tool) => tool.installed && tool.enabled);

      const enabledBuiltin = enabledTools.filter((tool) => !tool.is_custom).map((tool) => tool.key);

      const enabledCustomCount = enabledTools.filter((tool) => tool.is_custom).length;

      const agentsLine =
        enabledBuiltin.length === 0 && enabledCustomCount === 0
          ? "(none)"
          : [
              enabledBuiltin.join(", "),
              enabledCustomCount > 0 ? `${enabledCustomCount} custom` : "",
            ]
              .filter(Boolean)
              .join(", ");

      const parts = [
        "**Diagnostics** (auto-collected by Agents Manager)",
        "",
        `- App version: \`${info.app_version}\``,
        `- OS: \`${info.os} ${info.os_version} (${info.arch})\``,
        `- UI locale: \`${i18n.language}\``,
        `- Enabled agents: ${agentsLine}`,
        `- Central repo: \`${info.central_repo_path}\`${info.central_repo_path_overridden ? " (custom path)" : ""}`,
      ];

      if (panicInfo) {
        parts.push(
          "",
          `**Last panic** (${panicInfo.timestamp})`,
          "",
          "```",
          panicInfo.message,
          "```",
        );
      }

      if (logExcerpt) {
        parts.push(
          "",
          `**Recent log** (\`${logExcerpt.log_path}\`, ${logExcerpt.line_count} lines${logExcerpt.has_warnings ? ", includes warnings/errors" : ""})`,
          "",
          "```log",
          logExcerpt.excerpt,
          "```",
          "",
          `> ${t("settings.reportIssueExportHint")}`,
        );
      }

      const md = parts.join("\n");
      let copied = false;

      try {
        await clipboardWriteText(md);
        copied = true;
      } catch (err) {
        console.error("Clipboard write failed", err);

        try {
          await navigator.clipboard.writeText(md);
          copied = true;
        } catch (err2) {
          console.error("Browser clipboard fallback also failed", err2);
        }
      }

      try {
        await openUrl(`${GITHUB_URL}/issues/new?template=bug_report.md`);
      } catch (err) {
        console.error("Failed to open issue page", err);
      }

      if (copied) {
        toast.success(t("settings.diagnosticsCopied"));

        if (panicInfo) {
          try {
            await api.clearLastPanic();
          } catch (err) {
            console.warn("Failed to clear last_panic.log", err);
          }

          setLastPanic(null);
        }
      } else {
        toast.message(t("settings.diagnosticsCopyManual"), { description: md });
      }
    } catch (error) {
      console.error("Failed to prepare diagnostics", error);
      toast.error(t("common.error"));
    } finally {
      setReportingIssue(false);
    }
  };

  const category = resolveSettingsCategory(categoryParam);

  if (!category) {
    return <Navigate {...settingsLink()} replace />;
  }

  return (
    <div className="app-page">
      <div className="app-page-header">
        <h1 className="app-page-title flex items-center gap-2">
          <Settings2 className="h-4 w-4 text-accent" />
          {t("settings.title")}
        </h1>
      </div>

      {(repoWarnings.length > 0 || lastPanic) && (
        <div className="space-y-2">
          {repoWarnings.length > 0 && (
            <div className="app-panel flex flex-wrap items-start gap-2 border border-amber-500/40 bg-amber-500/10 p-3">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-700 dark:text-amber-300" />
              <div className="min-w-0 flex-1 space-y-1 text-[13px] text-amber-800 dark:text-amber-300">
                {repoWarnings.map((code) => (
                  <p key={code}>{t(`settings.repoWarning_${code}`)}</p>
                ))}
              </div>
            </div>
          )}
          {lastPanic && (
            <div className="app-panel flex flex-wrap items-center justify-between gap-2 border border-red-500/40 bg-red-500/10 p-3">
              <div className="flex min-w-0 items-center gap-2 text-[13px] text-red-700 dark:text-red-300">
                <AlertTriangle className="h-4 w-4 shrink-0" />
                <span>{t("settings.panicBanner", { time: lastPanic.timestamp })}</span>
              </div>
              <div className="flex gap-2">
                <button
                  type="button"
                  onClick={handleReportIssue}
                  disabled={reportingIssue}
                  className={`${ACTION_BUTTON_CLASS} border-red-600 bg-red-600 text-white hover:bg-red-700`}
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
                  onClick={handleDismissPanic}
                  className={`${ACTION_BUTTON_CLASS} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
                >
                  {t("settings.panicDismiss")}
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      <div className="flex flex-col gap-6 md:flex-row">
        <nav
          aria-label={t("settings.title")}
          className="md:sticky md:top-[48px] md:w-[180px] md:shrink-0 md:self-start"
        >
          <ul className="scrollbar-hide flex gap-1 overflow-x-auto md:flex-col md:gap-0.5 md:overflow-visible">
            {SETTINGS_CATEGORIES.map((slug) => {
              const Icon = CATEGORY_ICONS[slug];
              const isActive = slug === category;

              return (
                <li key={slug} className="shrink-0">
                  <Link
                    {...settingsLink(slug)}
                    aria-current={isActive ? "page" : undefined}
                    className={cn(
                      "flex items-center gap-2.5 whitespace-nowrap px-2.5 py-[7px] rounded-md text-sm font-medium transition-colors outline-none",
                      "focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent",
                      isActive
                        ? "bg-surface-active text-primary"
                        : "text-tertiary hover:text-secondary hover:bg-surface-hover",
                    )}
                  >
                    <Icon
                      className={cn("w-4 h-4 shrink-0", isActive ? "text-accent" : "text-muted")}
                    />
                    {t(`settings.categories.${slug}`)}
                    {slug === "about" && appUpdate?.has_update && (
                      <span
                        className="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400"
                        title={t("settings.updateAvailable", { version: appUpdate.latest_version })}
                      />
                    )}
                  </Link>
                </li>
              );
            })}
          </ul>
        </nav>

        {/* Every category stays mounted so switching keeps half-typed input and
            open forms, and doesn't re-fetch; only the active one is shown. */}
        <div className="min-w-0 flex-1">
          <div hidden={category !== "general"}>
            <GeneralSection />
          </div>
          <div hidden={category !== "agents"}>
            <AgentsSection />
          </div>
          <div hidden={category !== "library"}>
            <LibrarySection />
          </div>
          <div hidden={category !== "backup"}>
            <BackupSection />
          </div>
          <div hidden={category !== "network"}>
            <NetworkSection />
          </div>
          <div hidden={category !== "remote"}>
            <RemoteHostsSection />
          </div>
          <div hidden={category !== "about"}>
            <AboutSection reportingIssue={reportingIssue} onReportIssue={handleReportIssue} />
          </div>
        </div>
      </div>
    </div>
  );
}
