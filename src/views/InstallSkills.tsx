import { useNavigate, useSearch } from "@tanstack/react-router";
import { Box, Github, UploadCloud } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { GitInstallTab } from "../components/GitInstallTab";
import { GitPreviewDialog } from "../components/GitPreviewDialog";
import { InstallationProgress } from "../components/InstallationProgress";
import { LocalInstallTab } from "../components/LocalInstallTab";
import { MarketTab } from "../components/MarketTab";
import { useApp } from "../context/AppContext";
import { useOperations } from "../context/OperationsContext";
import { useGitPreview } from "../hooks/useGitPreview";
import { useLocalScan } from "../hooks/useLocalScan";
import { useMarketSearch } from "../hooks/useMarketSearch";
import { useSourceOverflow } from "../hooks/useSourceOverflow";
import { getErrorMessage } from "../lib/error";
import { findInstalledByGitUrl as findInstalledSkillByGitUrl } from "../lib/gitUrl";
import { type InstallSource, isItemActive, isLibraryOperation } from "../lib/operations";
import { pickPath } from "../lib/pickPath";
import { cn } from "../utils";
import type { InstallTab } from "./installSearch";

export function InstallSkills() {
  const { t } = useTranslation();
  const { operations, install } = useOperations();

  const { managedSkills, activeHostId, activeHost } = useApp();

  const navigate = useNavigate();
  const { tab: tabParam } = useSearch({ from: "/install" });
  const [activeTab, setActiveTab] = useState<InstallTab>(tabParam ?? "market");
  const [prevTabParam, setPrevTabParam] = useState(tabParam);

  if (tabParam !== prevTabParam) {
    setPrevTabParam(tabParam);

    if (tabParam) setActiveTab(tabParam);
  }

  const market = useMarketSearch(activeTab === "market");

  const {
    gitUrl,
    setGitUrl,
    gitLoading,
    gitPreview,
    gitSelections,
    setGitSelections,
    gitConfirmLoading,
    handleGitPreview,
    handleGitPreviewClose,
    handleGitConfirm,
    reviewOperation,
  } = useGitPreview();

  const { scanResult, scanLoading, localError, setLocalError, runScan } = useLocalScan(
    activeTab === "local",
  );

  const importingPaths = new Set(
    operations.flatMap((operation) =>
      operation.items.flatMap((item) =>
        isItemActive(item) && item.source?.kind === "local" ? [item.source.path] : [],
      ),
    ),
  );

  const importingAll = importingPaths.size > 0;

  const currentScan = scanResult
    ? {
        ...scanResult,
        groups: scanResult.groups.map((group) => ({
          ...group,
          imported:
            group.imported ||
            group.locations.some((location) =>
              managedSkills.some((skill) => skill.source_ref === location.found_path),
            ),
        })),
      }
    : null;

  const [renameEditing, setRenameEditing] = useState<Record<string, string>>({});
  const sourceOverflow = useSourceOverflow(market.sourceOptions);

  const installedSourceRefs = useMemo(() => {
    const set = new Set<string>();

    for (const skill of managedSkills) {
      if (skill.source_type === "skillssh" && skill.source_ref) {
        set.add(skill.source_ref);
      }
    }

    return set;
  }, [managedSkills]);

  const findInstalledByGitUrl = useCallback(
    (url: string) => findInstalledSkillByGitUrl(managedSkills, url),
    [managedSkills],
  );

  const switchTab = (tab: InstallTab) => {
    setActiveTab(tab);
    navigate({ to: "/install", search: { tab } });
  };

  const submitSources = async (sources: InstallSource[]) => {
    try {
      await install(sources);
    } catch (error) {
      const message = getErrorMessage(error, t("common.error"));
      setLocalError(message);
      toast.error(message);
    }
  };

  const installLocalSource = async (sourcePath: string) => {
    try {
      await install([{ kind: "local", path: sourcePath }]);
    } catch (error) {
      const message = getErrorMessage(error, t("common.error"));
      setLocalError(message);
      toast.error(message);
    }
  };

  const handleLocalFolderInstall = async () => {
    try {
      const selected = await pickPath({ directory: true });

      if (!selected) return;
      installLocalSource(selected);
    } catch (error: unknown) {
      const message = getErrorMessage(error, t("common.error"));
      setLocalError(message);
      toast.error(message);
    }
  };

  const handleLocalFileInstall = async () => {
    try {
      const selected = await pickPath({ files: ["zip", "skill"], filterName: "Skills" });

      if (!selected) return;
      installLocalSource(selected);
    } catch (error: unknown) {
      const message = getErrorMessage(error, t("common.error"));
      setLocalError(message);
      toast.error(message);
    }
  };

  const handleBatchImportFolder = async () => {
    try {
      const selected = await pickPath({ directory: true });

      if (selected) await submitSources([{ kind: "local_batch", path: selected }]);
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    }
  };

  const handleImportDiscovered = (path: string, name: string) =>
    submitSources([{ kind: "local", path, name, origin: "discovered" }]);

  const handleImportAllDiscovered = () =>
    submitSources(
      (currentScan?.groups ?? []).flatMap((group) => {
        const location = group.locations[0];

        return !group.imported && location
          ? [
              {
                kind: "local" as const,
                path: location.found_path,
                name: renameEditing[group.name] ?? group.name,
                origin: "discovered" as const,
              },
            ]
          : [];
      }),
    );

  const libraryOperations = operations.filter(isLibraryOperation);

  const sourceMatches = (source: InstallSource, tab: InstallTab) =>
    tab === "market"
      ? source.kind === "marketplace"
      : tab === "git"
        ? source.kind === "git"
        : source.kind === "local" || source.kind === "local_batch";

  const forSource = (operation: (typeof operations)[number], tab: InstallTab) =>
    (
      operation.sources ?? operation.items.flatMap((item) => (item.source ? [item.source] : []))
    ).some((source) => sourceMatches(source, tab));

  return (
    <div className="app-page gap-4">
      <div className="app-page-header border-b-0 pb-0">
        <h1 className="app-page-title mb-2">{t("install.title")}</h1>
        <p className="mb-4 text-[13px] text-secondary">
          {t("operations.installDestination", {
            host: activeHost?.name ?? t("operations.hostLocal"),
          })}
        </p>
        <div className="flex gap-1 border-b border-border-subtle">
          {[
            { id: "market" as const, label: t("install.browseMarket"), icon: Box },
            { id: "local" as const, label: t("install.localInstall"), icon: UploadCloud },
            { id: "git" as const, label: t("install.gitInstall"), icon: Github },
          ].map((tab) => {
            const Icon = tab.icon;
            const isActive = activeTab === tab.id;

            return (
              <button
                key={tab.id}
                onClick={() => switchTab(tab.id)}
                className={cn(
                  "mr-4 flex items-center gap-1.5 border-b-2 px-1 pb-1.5 text-[13px] font-medium transition-colors outline-none disabled:cursor-not-allowed disabled:opacity-50",
                  isActive
                    ? "border-accent text-accent"
                    : "border-transparent text-muted hover:text-tertiary",
                )}
              >
                <Icon className="h-3.5 w-3.5" />
                {tab.label}
                {libraryOperations.some((o) => !o.settled && forSource(o, tab.id)) && (
                  <span className="text-secondary">
                    {libraryOperations.filter((o) => !o.settled && forSource(o, tab.id)).length}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>

      {libraryOperations
        .filter((operation) => forSource(operation, activeTab))
        .map((operation) => (
          <InstallationProgress
            key={operation.operation_id}
            operation={operation}
            onReview={() => reviewOperation(operation.operation_id)}
          />
        ))}
      {activeTab === "market" && (
        <MarketTab
          market={market}
          sourceOverflow={sourceOverflow}
          installedSourceRefs={installedSourceRefs}
          hostId={activeHostId}
        />
      )}

      {activeTab === "local" && (
        <LocalInstallTab
          scanResult={currentScan}
          scanLoading={scanLoading}
          localError={localError}
          importingPaths={importingPaths}
          importingAll={importingAll}
          renameEditing={renameEditing}
          setRenameEditing={setRenameEditing}
          onInstallFolder={handleLocalFolderInstall}
          onInstallArchive={handleLocalFileInstall}
          onBatchImport={handleBatchImportFolder}
          onRescan={runScan}
          onImportAll={handleImportAllDiscovered}
          onImportOne={handleImportDiscovered}
        />
      )}

      {activeTab === "git" && (
        <GitInstallTab
          gitUrl={gitUrl}
          gitLoading={gitLoading}
          findInstalledByGitUrl={findInstalledByGitUrl}
          onGitUrlChange={setGitUrl}
          onPreview={handleGitPreview}
        />
      )}

      {/* Git preview / selection dialog */}
      {gitPreview && (
        <GitPreviewDialog
          selections={gitSelections}
          setSelections={setGitSelections}
          confirmLoading={gitConfirmLoading}
          onClose={handleGitPreviewClose}
          onConfirm={handleGitConfirm}
        />
      )}
    </div>
  );
}
