import { useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { Box, Github, UploadCloud } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { GitInstallTab } from "../components/GitInstallTab";
import { GitPreviewDialog } from "../components/GitPreviewDialog";
import { LocalInstallTab } from "../components/LocalInstallTab";
import { MarketTab } from "../components/MarketTab";
import { useApp } from "../context/AppContext";
import { useGitPreview } from "../hooks/useGitPreview";
import { useLocalScan } from "../hooks/useLocalScan";
import { useMarketSearch } from "../hooks/useMarketSearch";
import { useSourceOverflow } from "../hooks/useSourceOverflow";
import { managedSkillsQueryOptions, presetsQueryOptions, refreshQuery } from "../lib/appQueries";
import { getErrorKind, getErrorMessage } from "../lib/error";
import { findInstalledByGitUrl as findInstalledSkillByGitUrl } from "../lib/gitUrl";
import { getActiveHostId, invokeHost } from "../lib/hostCall";
import { listenOnActiveHost } from "../lib/hostEvents";
import { pickPath } from "../lib/pickPath";
import * as api from "../lib/tauri";
import type { BatchImportResult, SkillsShSkill } from "../lib/tauri";
import { cn } from "../utils";
import type { InstallTab } from "./installSearch";

interface InstallBatch {
  hostId: null | string;
  stopRequested: boolean;
  cancelKey: null | string;
}

export function InstallSkills() {
  const { t } = useTranslation();

  const { refreshPresets, refreshManagedSkills, managedSkills, openSkillDetailById, activeHostId } =
    useApp();

  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const { tab: tabParam } = useSearch({ from: "/install" });
  const [activeTab, setActiveTab] = useState<InstallTab>(tabParam ?? "market");
  const [prevTabParam, setPrevTabParam] = useState(tabParam);

  if (tabParam !== prevTabParam) {
    setPrevTabParam(tabParam);

    if (tabParam) setActiveTab(tabParam);
  }

  const market = useMarketSearch(activeTab === "market");
  const [installing, setInstalling] = useState<null | string>(null);

  const [bulkProgress, setBulkProgress] = useState<{ completed: number; total: number } | null>(
    null,
  );

  const [bulkLocked, setBulkLocked] = useState(false);
  const [bulkFailures, setBulkFailures] = useState<{ skill: SkillsShSkill; error: string }[]>([]);
  const [bulkRetrySkills, setBulkRetrySkills] = useState<SkillsShSkill[]>([]);

  const [bulkSummary, setBulkSummary] = useState<{
    installed: number;
    total: number;
    stopped: boolean;
  } | null>(null);

  const [bulkResultHostId, setBulkResultHostId] = useState<null | string>(null);

  const batchRef = useRef<InstallBatch | null>(null);

  const bulkEpochRef = useRef(0);
  const mountedRef = useRef(true);

  const {
    gitUrl,
    setGitUrl,
    gitLoading,
    gitCancelKey,
    gitPreview,
    gitSelections,
    setGitSelections,
    gitConfirmLoading,
    handleGitPreview,
    handleGitPreviewClose,
    handleGitConfirm,
  } = useGitPreview();

  const { scanResult, scanLoading, localError, setLocalError, runScan, runScanSilent } =
    useLocalScan(activeTab === "local");

  const [importingPaths, setImportingPaths] = useState<Set<string>>(new Set());
  const [importingAll, setImportingAll] = useState(false);
  const [renameEditing, setRenameEditing] = useState<Record<string, string>>({});
  const sourceOverflow = useSourceOverflow(market.sourceOptions);

  useEffect(() => {
    mountedRef.current = true;

    return () => {
      mountedRef.current = false;
      const batch = batchRef.current;

      if (!batch) return;
      batch.stopRequested = true;

      if (batch.cancelKey) {
        invokeHost<boolean>(batch.hostId, "cancel_install", { key: batch.cancelKey }).catch(
          () => {},
        );
      }
    };
  }, []);

  // A host or tab switch drops the bulk results. Progress is only ever set
  // while a batch on this host and tab runs, and that batch is stopped below.
  const [bulkScope, setBulkScope] = useState({ hostId: activeHostId, tab: activeTab });

  if (bulkScope.hostId !== activeHostId || bulkScope.tab !== activeTab) {
    setBulkScope({ hostId: activeHostId, tab: activeTab });
    setBulkFailures([]);
    setBulkRetrySkills([]);
    setBulkSummary(null);
    setBulkProgress(null);
  }

  useEffect(() => {
    bulkEpochRef.current++;
    const batch = batchRef.current;

    if (batch && (batch.hostId !== activeHostId || activeTab !== "market")) {
      batch.stopRequested = true;

      if (batch.cancelKey) {
        invokeHost<boolean>(batch.hostId, "cancel_install", { key: batch.cancelKey }).catch(
          () => {},
        );
      }
    }
  }, [activeHostId, activeTab]);

  const managedSkillsRef = useRef(managedSkills);
  useEffect(() => {
    managedSkillsRef.current = managedSkills;
  }, [managedSkills]);

  const goToSkill = useCallback(
    (skillName: string) => {
      // Use ref to get the latest managedSkills after refresh
      const skills = managedSkillsRef.current;

      const skill = skills.find((s) => s.name === skillName || s.source_ref === skillName);

      if (skill) {
        openSkillDetailById(skill.id);
      }

      navigate({ to: "/my-skills" });
    },
    [navigate, openSkillDetailById],
  );

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

  const warnRejected = (results: PromiseSettledResult<unknown>[], label: string) => {
    for (const r of results) {
      if (r.status === "rejected") console.warn(`${label} failed:`, r.reason);
    }
  };

  const installLocalSource = async (sourcePath: string) => {
    const name = sourcePath.split("/").pop() || sourcePath;
    const toastId = toast.loading(t("install.toast.installing", { name }));

    try {
      await api.installLocal(sourcePath);
    } catch (e) {
      const message = getErrorMessage(e, t("common.error"));
      setLocalError(message);
      toast.error(message, { id: toastId });

      return;
    }

    // Install succeeded — post-install refresh is best-effort and must not
    // surface as an install failure.
    const results = await Promise.allSettled([
      refreshPresets(),
      refreshManagedSkills(),
      runScanSilent(),
    ]);

    warnRejected(results, "post-install refresh");
    toast.success(t("install.toast.success", { name }), {
      id: toastId,
      action: {
        label: t("install.toast.view"),
        onClick: () => goToSkill(name),
      },
    });
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
    let unlisten: (() => void) | null = null;

    try {
      const selected = await pickPath({ directory: true });

      if (!selected) return;

      const toastId = toast.loading(t("install.local.batchImporting"));

      unlisten = await listenOnActiveHost<{ current: number; total: number; name: string }>(
        "batch-import-progress",
        (event) => {
          const { current, total, name } = event.payload;
          toast.loading(t("install.local.batchProgress", { current, total, name }), {
            id: toastId,
          });
        },
      );

      const result: BatchImportResult = await api.batchImportFolder(selected);

      if (result.errors.length > 0) {
        const previewErrors = result.errors.slice(0, 3).join("; ");
        const remaining = result.errors.length - 3;
        const detail = remaining > 0 ? `${previewErrors}; +${remaining} more` : previewErrors;
        toast.error(
          `${t("install.local.batchErrors", { count: result.errors.length })}: ${detail}`,
          { id: toastId },
        );
      } else if (result.imported === 0) {
        toast.info(t("install.local.batchAllSkipped", { skipped: result.skipped }), {
          id: toastId,
        });
      } else {
        toast.success(
          t("install.local.batchSuccess", {
            imported: result.imported,
            skipped: result.skipped,
          }),
          { id: toastId },
        );
      }

      await Promise.all([refreshPresets(), refreshManagedSkills()]);
      runScan();
    } catch (error: unknown) {
      const message = getErrorMessage(error, t("common.error"));
      setLocalError(message);
      toast.error(message);
    } finally {
      unlisten?.();
    }
  };

  const handleInstallSkillssh = async (skill: SkillsShSkill) => {
    if (batchRef.current) return;
    const displayName = skill.name || skill.skill_id;
    const cancelKey = `${skill.source}/${skill.skill_id}`;
    setInstalling(skill.id);

    const toastId = toast.loading(t("install.toast.cloning"));
    let unlisten: (() => void) | null = null;

    try {
      unlisten = await listenOnActiveHost<{ skill_id: string; phase: string; detail?: string }>(
        "install-progress",
        (event) => {
          if (event.payload.skill_id !== cancelKey) return;

          if (event.payload.phase === "cloning") {
            const detail = event.payload.detail?.trim();

            const msg = detail
              ? `${t("install.toast.cloning")}\n${detail}`
              : t("install.toast.cloning");

            toast.loading(msg, { id: toastId });
          } else if (event.payload.phase === "installing") {
            toast.loading(t("install.toast.installing", { name: displayName }), { id: toastId });
          }
        },
      );
      await api.installFromSkillssh(skill.source, skill.skill_id);
      await Promise.all([refreshPresets(), refreshManagedSkills()]);
      toast.success(t("install.toast.success", { name: displayName }), {
        id: toastId,
        action: {
          label: t("install.toast.view"),
          onClick: () => goToSkill(displayName),
        },
      });
    } catch (error: unknown) {
      if (getErrorKind(error) === "cancelled") {
        toast.info(t("install.toast.cancelled"), { id: toastId });
      } else {
        toast.error(getErrorMessage(error, t("common.error")), { id: toastId });
      }
    } finally {
      setInstalling(null);
      unlisten?.();
    }
  };

  const runBulkInstall = async (skills: SkillsShSkill[]): Promise<string[]> => {
    if (skills.length === 0 || batchRef.current || installing !== null) return [];
    const hostId = activeHostId;
    const epoch = bulkEpochRef.current;
    const batch: InstallBatch = { hostId, stopRequested: false, cancelKey: null };
    batchRef.current = batch;
    setBulkLocked(true);
    setBulkFailures([]);
    setBulkRetrySkills([]);
    setBulkSummary(null);
    setBulkResultHostId(hostId);
    setBulkProgress({ completed: 0, total: skills.length });
    let installed = 0;
    let completed = 0;
    let stopped = false;
    const failures: { skill: SkillsShSkill; error: string }[] = [];
    const retrySkills: SkillsShSkill[] = [];
    const installedIds: string[] = [];

    for (let index = 0; index < skills.length; index++) {
      const skill = skills[index];

      if (batch.stopRequested) {
        stopped = true;
        retrySkills.push(...skills.slice(index));
        break;
      }

      const cancelKey = `${skill.source}/${skill.skill_id}`;
      batch.cancelKey = cancelKey;

      try {
        await invokeHost<void>(hostId, "install_from_skillssh", {
          source: skill.source,
          skillId: skill.skill_id,
        });
        installed++;
        installedIds.push(skill.id);
      } catch (error: unknown) {
        if (batch.stopRequested || getErrorKind(error) === "cancelled") {
          stopped = true;
          retrySkills.push(...skills.slice(index));
          break;
        }

        failures.push({ skill, error: getErrorMessage(error, t("common.error")) });
        retrySkills.push(skill);
      }

      completed++;

      if (mountedRef.current && bulkEpochRef.current === epoch && getActiveHostId() === hostId) {
        setBulkProgress({ completed, total: skills.length });
      }
    }

    try {
      await Promise.all([
        refreshQuery(queryClient, presetsQueryOptions(hostId)),
        refreshQuery(queryClient, managedSkillsQueryOptions(hostId)),
      ]);
    } catch (error) {
      console.warn("Post-batch install refresh failed:", error);

      if (mountedRef.current && bulkEpochRef.current === epoch && getActiveHostId() === hostId) {
        toast.error(t("install.market.refreshError"));
      }
    }

    if (mountedRef.current && bulkEpochRef.current === epoch && getActiveHostId() === hostId) {
      setBulkFailures(failures);
      setBulkRetrySkills(retrySkills);
      setBulkSummary({ installed, total: skills.length, stopped });
      setBulkProgress(null);

      if (installed > 0) {
        toast.success(t("install.market.batchSummary", { installed, total: skills.length }));
      }

      if (failures.length > 0) {
        toast.error(t("install.market.batchInstallError", { count: failures.length }));
      }
    }

    if (batchRef.current === batch) batchRef.current = null;
    setBulkLocked(false);

    return installedIds;
  };

  const handleStopBulkInstall = () => {
    const batch = batchRef.current;

    if (!batch) return;
    batch.stopRequested = true;

    if (batch.cancelKey) {
      invokeHost<boolean>(batch.hostId, "cancel_install", { key: batch.cancelKey }).catch(() => {});
    }
  };

  const handleRetryBulkInstall = () => runBulkInstall(bulkRetrySkills);

  const handleCancelInstall = (cancelKey: string) => {
    api.cancelInstall(cancelKey).catch(() => {
      // Ignore race: install may have completed before cancel request arrives.
    });
  };

  const handleImportDiscovered = async (sourcePath: string, name: string) => {
    setImportingPaths((prev) => new Set(prev).add(sourcePath));

    try {
      try {
        await api.importExistingSkill(sourcePath, name);
      } catch (error: unknown) {
        toast.error(getErrorMessage(error, t("common.error")));

        return;
      }

      toast.success(t("install.scan.importedOne", { name }));

      const results = await Promise.allSettled([
        refreshPresets(),
        refreshManagedSkills(),
        runScanSilent(),
      ]);

      warnRejected(results, "post-import refresh");
    } finally {
      setImportingPaths((prev) => {
        const next = new Set(prev);
        next.delete(sourcePath);

        return next;
      });
    }
  };

  const handleImportAllDiscovered = async () => {
    setImportingAll(true);

    try {
      try {
        await api.importAllDiscovered();
      } catch (error: unknown) {
        toast.error(getErrorMessage(error, t("common.error")));

        return;
      }

      toast.success(t("install.scan.importedAll"));

      const results = await Promise.allSettled([
        refreshPresets(),
        refreshManagedSkills(),
        runScanSilent(),
      ]);

      warnRejected(results, "post-import refresh");
    } finally {
      setImportingAll(false);
    }
  };

  return (
    <div className="app-page gap-4">
      <div className="app-page-header border-b-0 pb-0">
        <h1 className="app-page-title mb-4">{t("install.title")}</h1>
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
                disabled={bulkLocked}
                className={cn(
                  "mr-4 flex items-center gap-1.5 border-b-2 px-1 pb-1.5 text-[13px] font-medium transition-colors outline-none disabled:cursor-not-allowed disabled:opacity-50",
                  isActive
                    ? "border-accent text-accent"
                    : "border-transparent text-muted hover:text-tertiary",
                )}
              >
                <Icon className="h-3.5 w-3.5" />
                {tab.label}
              </button>
            );
          })}
        </div>
      </div>

      {activeTab === "market" && (
        <MarketTab
          market={market}
          sourceOverflow={sourceOverflow}
          installedSourceRefs={installedSourceRefs}
          installing={installing}
          onInstall={handleInstallSkillssh}
          onCancelInstall={handleCancelInstall}
          bulkProgress={activeHostId === bulkResultHostId ? bulkProgress : null}
          bulkFailures={activeHostId === bulkResultHostId ? bulkFailures : []}
          bulkSummary={activeHostId === bulkResultHostId ? bulkSummary : null}
          retryCount={bulkRetrySkills.length}
          batchLocked={bulkLocked}
          hostId={activeHostId}
          onInstallSelected={runBulkInstall}
          onStopBulkInstall={handleStopBulkInstall}
          onRetryBulkInstall={handleRetryBulkInstall}
        />
      )}

      {activeTab === "local" && (
        <LocalInstallTab
          scanResult={scanResult}
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
          gitCancelKey={gitCancelKey}
          findInstalledByGitUrl={findInstalledByGitUrl}
          onGitUrlChange={setGitUrl}
          onPreview={handleGitPreview}
          onCancelInstall={handleCancelInstall}
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
