import { Check, DownloadCloud, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import type { ManagedSkill } from "../lib/tauri";
import { cn } from "../utils";
import { GithubIcon } from "./GithubIcon";

interface GitInstallTabProps {
  gitUrl: string;
  /** Admission request in flight; execution is observed beside this source. */
  gitLoading: boolean;
  findInstalledByGitUrl: (url: string) => ManagedSkill | undefined;
  onGitUrlChange: (url: string) => void;
  onPreview: () => void;
}

/** The install page's git tab: clone a repo URL to preview its skills. */
export function GitInstallTab({
  gitUrl,
  gitLoading,
  findInstalledByGitUrl,
  onGitUrlChange,
  onPreview,
}: GitInstallTabProps) {
  const { t } = useTranslation();

  return (
    <div className="animate-in fade-in duration-300">
      <div className="app-panel max-w-lg p-5">
        <div className="mb-4 flex h-10 w-10 items-center justify-center rounded-lg border border-border bg-surface-hover">
          <GithubIcon className="h-5 w-5 text-tertiary" />
        </div>
        <h2 className="mb-1 text-[14px] font-semibold text-primary">{t("install.gitTitle")}</h2>
        <p className="mb-4 text-[13px] text-muted">{t("install.gitDesc")}</p>

        <div className="space-y-3">
          <div>
            <label className="mb-1 block text-[13px] font-medium text-tertiary">
              {t("install.repoUrl")}
            </label>
            <input
              type="text"
              value={gitUrl}
              onChange={(e) => onGitUrlChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !gitLoading && gitUrl.trim()) onPreview();
              }}
              placeholder={t("install.repoUrlPlaceholder")}
              disabled={gitLoading}
              className="app-input w-full bg-background"
            />
          </div>
          {gitUrl.trim() && findInstalledByGitUrl(gitUrl) && (
            <div className="flex items-center gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-[13px] text-amber-400">
              <Check className="h-3.5 w-3.5 shrink-0" />
              <span>
                {t("install.gitAlreadyInstalled", { name: findInstalledByGitUrl(gitUrl)!.name })}
              </span>
            </div>
          )}
          <div className="flex gap-2 pt-2">
            <button
              onClick={onPreview}
              disabled={gitLoading || !gitUrl.trim()}
              className={cn(
                "flex w-full",
                gitUrl.trim() && findInstalledByGitUrl(gitUrl)
                  ? "app-button-secondary bg-background"
                  : "app-button-primary",
              )}
            >
              {gitLoading ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <DownloadCloud className="h-3.5 w-3.5" />
              )}
              {gitLoading
                ? t("operations.submitting")
                : gitUrl.trim() && findInstalledByGitUrl(gitUrl)
                  ? t("install.gitReinstall")
                  : t("install.installClone")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
