import { useQueryClient } from "@tanstack/react-query";
import { Navigate, useNavigate, useParams } from "@tanstack/react-router";
import {
  ChevronRight,
  CircleSlash,
  Download,
  FileText,
  Globe,
  LayoutGrid,
  List,
  Loader2,
  Plus,
  RefreshCw,
  Search,
  Square,
  SquareCheck,
  Trash2,
  Unlink,
  Upload,
} from "lucide-react";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { AddSkillsSheet } from "../components/AddSkillsSheet";
import { AgentIcon } from "../components/AgentIcon";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { CreatorBadge } from "../components/CreatorBadge";
import { DetailSheet } from "../components/DetailSheet";
import { DocumentDiffViewer } from "../components/DocumentDiffViewer";
import { MultiSelectToolbar } from "../components/MultiSelectToolbar";
import { PresetBar } from "../components/PresetBar";
import { ResourceWorkspace } from "../components/ResourceWorkspace";
import { SkillMarkdown } from "../components/SkillMarkdown";
import { useApp } from "../context/AppContext";
import { useMultiSelect } from "../hooks/useMultiSelect";
import { managedSkillsQueryOptions, refreshQuery, toolsQueryOptions } from "../lib/appQueries";
import { getErrorMessage } from "../lib/error";
import { invokeHost } from "../lib/hostCall";
import { copyCreator, type SkillCreator } from "../lib/skillCreator";
import {
  getTagActiveColor,
  getTagColor,
  pruneStaleTagFilters,
  UNTAGGED_FILTER,
} from "../lib/skillTags";
import { matchesTagFilter } from "../lib/tagFilter";
import * as api from "../lib/tauri";
import type { ManagedSkill, ProjectSkill } from "../lib/tauri";
import { cn } from "../utils";
import {
  CODING_WORKSPACE_CONFIG,
  LOBSTER_WORKSPACE_CONFIG,
  type WorkspaceConfig,
} from "./workspaceConfigs";

function compactHomePath(path: string) {
  return path.replace(/^\/Users\/[^/]+/, "~");
}

interface WorkspaceSkillCardTag {
  label: string;
  className: string;
}

interface WorkspaceSkillCardStatus {
  label: string;
  className: string;
}

interface BatchWorkspaceTarget {
  hostId: null | string;
  agentKey: string;
  agentName: string;
  viewVersion: number;
  skills: ProjectSkill[];
}

function WorkspaceSkillCard({
  viewMode,
  title,
  description,
  tags = [],
  status,
  creator,
  fileCount = 0,
  active = false,
  actions,
  actionsHover = false,
  selectable = false,
  selected = false,
  onClick,
}: {
  viewMode: "grid" | "list";
  title: string;
  description?: null | string;
  tags?: WorkspaceSkillCardTag[];
  status: WorkspaceSkillCardStatus;
  creator: SkillCreator;
  fileCount?: number;
  active?: boolean;
  actions?: ReactNode;
  actionsHover?: boolean;
  /** Multi-select mode: the leading slot becomes a checkbox and clicks select. */
  selectable?: boolean;
  selected?: boolean;
  onClick: () => void;
}) {
  const leadingSlot = selectable ? (
    selected ? (
      <SquareCheck className="h-3.5 w-3.5 text-accent" />
    ) : (
      <Square className="h-3.5 w-3.5 text-faint" />
    )
  ) : (
    <span
      className={cn(
        "h-2 w-2 rounded-full",
        active ? "bg-accent-light shadow-[0_0_0_3px_var(--color-accent-bg)]" : "bg-surface-active",
      )}
    />
  );

  if (viewMode === "list") {
    return (
      <div
        className={cn(
          "app-panel group relative flex cursor-pointer items-center gap-3.5 rounded-xl border-transparent px-3.5 py-3 transition-all hover:border-border hover:bg-surface-hover",
          selectable && selected && "ring-1 ring-accent border-accent/40",
        )}
        onClick={onClick}
      >
        <div className="flex h-4 w-4 shrink-0 items-center justify-center">{leadingSlot}</div>
        <h3
          className="w-[180px] shrink-0 truncate text-[14px] font-semibold text-secondary group-hover:text-primary"
          title={title}
        >
          {title}
        </h3>
        <p className="min-w-0 flex-1 truncate text-[13px] text-muted">{description || "-"}</p>
        {tags.length > 0 && (
          <div className="flex shrink-0 items-center gap-1.5">
            {tags.map((tag) => (
              <span
                key={tag.label}
                className={cn(
                  "inline-flex items-center rounded-full px-1.5 py-0.5 text-[11px] font-medium",
                  tag.className,
                )}
              >
                {tag.label}
              </span>
            ))}
          </div>
        )}
        <div className="flex shrink-0 items-center gap-2.5">
          <CreatorBadge creator={creator} size="md" hideLocal className="max-w-[160px]" />
          <span
            className={cn("rounded-full px-2 py-0.5 text-[12px] font-medium", status.className)}
          >
            {status.label}
          </span>
          {fileCount > 0 && (
            <span className="flex items-center gap-1 text-[12px] text-faint">
              <FileText className="h-3 w-3" />
              {fileCount}
            </span>
          )}
        </div>
        {actions && (
          <div
            className={cn(
              "flex shrink-0 items-center gap-1",
              actionsHover && "opacity-0 transition-opacity group-hover:opacity-100",
            )}
          >
            {actions}
          </div>
        )}
      </div>
    );
  }

  return (
    <div
      className={cn(
        "app-panel group relative flex h-full cursor-pointer flex-col overflow-hidden shadow-card transition-all hover:-translate-y-px hover:border-border hover:shadow-card-hover",
        selectable && selected && "ring-1 ring-accent border-accent/40",
      )}
      onClick={onClick}
    >
      <div className="flex items-center gap-2.5 px-3.5 pb-1.5 pt-3">
        <div className="flex h-4 w-4 shrink-0 items-center justify-center">{leadingSlot}</div>
        <h3
          className="flex-1 truncate text-[14px] font-semibold text-primary group-hover:text-accent-light"
          title={title}
        >
          {title}
        </h3>
        {fileCount > 0 && (
          <span className="flex shrink-0 items-center gap-1 text-[12px] text-faint">
            <FileText className="h-3 w-3" />
            {fileCount}
          </span>
        )}
      </div>
      <div className="px-3.5 pb-3">
        <p className="truncate text-[13px] leading-[18px] text-muted">{description || "-"}</p>
        {tags.length > 0 && (
          <div className="mt-2 flex flex-wrap items-center gap-1">
            {tags.map((tag) => (
              <span
                key={tag.label}
                className={cn(
                  "inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium",
                  tag.className,
                )}
              >
                {tag.label}
              </span>
            ))}
          </div>
        )}
      </div>
      <div className="mt-auto flex items-center justify-between gap-2 border-t border-border-faint px-3.5 py-2.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <span
            className={cn(
              "shrink-0 rounded-full px-2 py-0.5 text-[12px] font-medium",
              status.className,
            )}
          >
            {status.label}
          </span>
          <CreatorBadge creator={creator} hideLocal />
        </div>
        {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
      </div>
    </div>
  );
}

function getLocalStatusMeta(t: (key: string) => string, status: ProjectSkill["sync_status"]) {
  switch (status) {
    case "in_sync":
      return {
        label: t("globalWorkspace.localSkills.status.inSync"),
        className: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
      };
    case "project_newer":
      return {
        label: t("globalWorkspace.localSkills.status.localNewer"),
        className: "bg-amber-500/10 text-amber-700 dark:text-amber-300",
      };
    case "center_newer":
      return {
        label: t("globalWorkspace.localSkills.status.centerNewer"),
        className: "bg-sky-500/10 text-sky-700 dark:text-sky-300",
      };
    case "diverged":
      return {
        label: t("globalWorkspace.localSkills.status.diverged"),
        className: "bg-violet-500/10 text-violet-700 dark:text-violet-300",
      };
    default:
      return {
        label: t("globalWorkspace.localSkills.status.localOnly"),
        className: "bg-surface-hover text-muted",
      };
  }
}

export function WorkspaceView({ config }: { config: WorkspaceConfig }) {
  const { agentKey } = useParams({ from: config.routePath });
  const navigate = useNavigate();
  const { t } = useTranslation();

  const { tools, managedSkills, presets, refreshManagedSkills, refreshTools, activeHostId } =
    useApp();

  const queryClient = useQueryClient();

  const [viewMode, setViewMode] = useState<"grid" | "list">("grid");
  const [search, setSearch] = useState("");
  const [tagFilters, setTagFilters] = useState<Set<string>>(new Set());
  const [addDialogOpen, setAddDialogOpen] = useState(false);
  const [removingLocalSkillId, setRemovingLocalSkillId] = useState<null | string>(null);
  const [localSkills, setLocalSkills] = useState<ProjectSkill[]>([]);
  const [localSkillsLoading, setLocalSkillsLoading] = useState(false);
  const [localActionKey, setLocalActionKey] = useState<null | string>(null);
  const [localDetailSkill, setLocalDetailSkill] = useState<null | ProjectSkill>(null);
  const [localDocContent, setLocalDocContent] = useState<null | string>(null);
  const [localCenterDocContent, setLocalCenterDocContent] = useState<null | string>(null);
  const [localDocLoading, setLocalDocLoading] = useState(false);
  const [localCenterDocLoading, setLocalCenterDocLoading] = useState(false);
  const [localContentTab, setLocalContentTab] = useState<"center" | "diff" | "local">("local");
  const [uploadConfirmSkill, setUploadConfirmSkill] = useState<null | ProjectSkill>(null);
  const [pullConfirmSkill, setPullConfirmSkill] = useState<null | ProjectSkill>(null);
  const [deleteLocalConfirmSkill, setDeleteLocalConfirmSkill] = useState<null | ProjectSkill>(null);
  const [batchUnsyncTarget, setBatchUnsyncTarget] = useState<BatchWorkspaceTarget | null>(null);
  const [batchDeleteTarget, setBatchDeleteTarget] = useState<BatchWorkspaceTarget | null>(null);
  const [batchRunning, setBatchRunning] = useState(false);
  const [batchAction, setBatchAction] = useState<"local-delete" | "unsync" | null>(null);

  const [batchFailures, setBatchFailures] = useState<{
    unsync: Array<{ name: string; error: string }>;
    localDelete: Array<{ name: string; error: string }>;
  }>({ unsync: [], localDelete: [] });

  const batchSelectionLocked =
    batchRunning || batchUnsyncTarget !== null || batchDeleteTarget !== null;

  const localDetailRequestRef = useRef(0);

  // Async batch work compares against this to tell whether its view is still shown.
  const workspaceIdentityRef = useRef({
    hostId: activeHostId,
    agentKey: agentKey ?? null,
    version: 0,
  });

  useLayoutEffect(() => {
    workspaceIdentityRef.current = {
      hostId: activeHostId,
      agentKey: agentKey ?? null,
      version: workspaceIdentityRef.current.version + 1,
    };
  }, [activeHostId, agentKey]);

  const workspaceMountedRef = useRef(true);
  useEffect(() => {
    workspaceMountedRef.current = true;

    return () => {
      workspaceMountedRef.current = false;
    };
  }, []);

  // Cross-category redirect: a deep link like /global-workspace/openclaw should
  // land on /lobster-workspace/openclaw. Compute it before any filtering so a
  // category mismatch doesn't briefly render "agent not found".
  const requestedTool = useMemo(
    () => (agentKey ? (tools.find((t) => t.key === agentKey) ?? null) : null),
    [agentKey, tools],
  );

  const needsRedirect = !!agentKey && !!requestedTool && requestedTool.category !== config.category;

  const redirectTarget =
    needsRedirect && requestedTool
      ? requestedTool.category === "lobster"
        ? LOBSTER_WORKSPACE_CONFIG.routePath
        : CODING_WORKSPACE_CONFIG.routePath
      : null;

  const installedTools = useMemo(
    () => tools.filter((t) => t.installed && t.enabled && t.category === config.category),
    [tools, config.category],
  );

  const skillCountByAgent = useMemo(() => {
    const map: Record<string, number> = {};

    for (const tool of installedTools) {
      map[tool.key] = managedSkills.filter((s) =>
        s.targets.some((target) => target.tool === tool.key),
      ).length;
    }

    return map;
  }, [installedTools, managedSkills]);

  // Overview cards should reflect each agent's ACTUAL on-disk skill count —
  // including skills installed outside Skills Manager — to match the per-agent
  // detail badge. The managed-only count above reads 0 for an agent whose
  // skills all live on disk but were never imported (#287). We fill this from a
  // per-agent scan and fall back to the managed count until it resolves.
  const [localCountByAgent, setLocalCountByAgent] = useState<Record<string, number>>({});
  const overviewCountsRef = useRef(0);

  const currentTool = useMemo(
    () => (agentKey ? (installedTools.find((t) => t.key === agentKey) ?? null) : null),
    [agentKey, installedTools],
  );

  // Preset actions must target what is actually rendered: a single agent when
  // `currentTool` resolves, otherwise every installed agent in this category.
  // Falling back to the raw URL `agentKey` would let a stale deep link (a
  // bookmarked route for a since-disabled or uninstalled agent) mutate the
  // hidden agent while the overview is shown.
  const presetBarAgentKeys = useMemo(
    () => (currentTool ? [currentTool.key] : installedTools.map((t) => t.key)),
    [currentTool, installedTools],
  );

  const currentToolKey = currentTool?.key ?? null;

  const [workspaceScope, setWorkspaceScope] = useState({
    hostId: activeHostId,
    toolKey: currentToolKey,
  });

  if (workspaceScope.hostId !== activeHostId || workspaceScope.toolKey !== currentToolKey) {
    setWorkspaceScope({ hostId: activeHostId, toolKey: currentToolKey });
    setBatchUnsyncTarget(null);
    setBatchDeleteTarget(null);
    setBatchFailures({ unsync: [], localDelete: [] });

    // The overview has no local skill list.
    if (!currentToolKey) setLocalSkills([]);

    if (workspaceScope.toolKey !== currentToolKey) {
      setLocalDetailSkill(null);
      setUploadConfirmSkill(null);
      setPullConfirmSkill(null);
      setDeleteLocalConfirmSkill(null);
      setTagFilters(new Set());
    }
  }

  const localSkillsRequestRef = useRef(0);

  const loadLocalSkills = useCallback(async () => {
    const requestId = ++localSkillsRequestRef.current;

    if (!currentToolKey) {
      setLocalSkills([]);

      return;
    }

    setLocalSkillsLoading(true);

    try {
      const skills = await api.getGlobalLocalSkills(currentToolKey);

      if (localSkillsRequestRef.current === requestId) setLocalSkills(skills);
    } catch (error: unknown) {
      if (localSkillsRequestRef.current === requestId) {
        toast.error(getErrorMessage(error, t("common.error")));
        setLocalSkills([]);
      }
    } finally {
      if (localSkillsRequestRef.current === requestId) setLocalSkillsLoading(false);
    }
  }, [currentToolKey, t]);

  const loadedAgentKeyRef = useRef<null | string>(null);
  const localSkillsScopeKey = `${activeHostId ?? "local"}:${currentToolKey ?? ""}`;
  useEffect(() => {
    if (!currentToolKey) {
      loadedAgentKeyRef.current = null;

      return;
    }

    if (loadedAgentKeyRef.current === localSkillsScopeKey) return;
    loadedAgentKeyRef.current = localSkillsScopeKey;
    void loadLocalSkills();

    return () => {
      localSkillsRequestRef.current += 1;
      loadedAgentKeyRef.current = null;
    };
  }, [currentToolKey, localSkillsScopeKey, loadLocalSkills]);

  // Load real on-disk skill counts for every installed agent while the overview
  // is shown (#287). Scoped to the overview (currentToolKey === null); the
  // detail view derives its own count from `localSkills`.
  useEffect(() => {
    if (currentToolKey) return;

    const requestId = ++overviewCountsRef.current;
    void (async () => {
      const entries = await Promise.all(
        installedTools.map(async (tool) => {
          try {
            const skills = await api.getGlobalLocalSkills(tool.key);

            return [tool.key, skills.length] as const;
          } catch {
            // Keep the managed-count fallback for an agent that fails to scan.
            return [tool.key, null] as const;
          }
        }),
      );

      if (overviewCountsRef.current !== requestId) return;
      // Rebuild the map from this scan's results (don't merge into the previous
      // map): agents whose scan failed are omitted so they fall back to the
      // managed count rather than showing a stale value, and counts for agents
      // no longer installed are dropped.
      const next: Record<string, number> = {};

      for (const [key, count] of entries) {
        if (count !== null) next[key] = count;
      }

      setLocalCountByAgent(next);
    })();
    // Depend on the managedSkills array reference (not just its length): a
    // target-only enable/disable or an externally added unmanaged skill changes
    // on-disk presence without changing the managed count, but still produces a
    // fresh array via refreshManagedSkills (the file watcher triggers it), so
    // the overview counts re-scan and stay accurate (#287).
  }, [currentToolKey, installedTools, managedSkills]);

  // Drop any in-flight detail load from the previous agent.
  useEffect(() => {
    localDetailRequestRef.current += 1;
  }, [currentToolKey]);

  const agentSkills = useMemo(
    () =>
      agentKey
        ? managedSkills.filter((skill) => skill.targets.some((target) => target.tool === agentKey))
        : [],
    [agentKey, managedSkills],
  );

  const allLocalTags = useMemo(() => {
    const tags = new Set<string>();

    for (const skill of localSkills) {
      for (const tag of skill.tags) {
        if (tag.trim()) tags.add(tag);
      }
    }

    return Array.from(tags).sort((a, b) => a.localeCompare(b));
  }, [localSkills]);

  // Prune tag filters whose pill disappeared (e.g. its last skill was deleted),
  // otherwise a stale filter silently hides everything. An empty list is also
  // what a failed load and the overview leave behind (`setLocalSkills([])`), and
  // that says nothing about which tags are valid — wait for a real list.
  const [prunedForSkills, setPrunedForSkills] = useState(localSkills);

  if (prunedForSkills !== localSkills) {
    setPrunedForSkills(localSkills);

    if (localSkills.length > 0) {
      const hasUntagged = localSkills.some((skill) => skill.tags.length === 0);
      setTagFilters((prev) => pruneStaleTagFilters(prev, allLocalTags, hasUntagged));
    }
  }

  const visibleLocalSkills = useMemo(() => {
    const q = search.trim().toLowerCase();

    return localSkills
      .filter((skill) => {
        if (q) {
          const matchesQuery =
            skill.name.toLowerCase().includes(q) ||
            skill.dir_name.toLowerCase().includes(q) ||
            (skill.description || "").toLowerCase().includes(q);

          if (!matchesQuery) return false;
        }

        if (!matchesTagFilter(skill.tags, tagFilters)) return false;

        return true;
      })
      .sort((a, b) => {
        const priority: Record<ProjectSkill["sync_status"], number> = {
          project_only: 0,
          project_newer: 1,
          diverged: 2,
          center_newer: 3,
          in_sync: 4,
        };

        return priority[a.sync_status] - priority[b.sync_status] || a.name.localeCompare(b.name);
      });
  }, [localSkills, search, tagFilters]);

  const inSyncLocalCount = useMemo(
    () => localSkills.filter((skill) => skill.sync_status === "in_sync").length,
    [localSkills],
  );

  const installedIds = useMemo(() => new Set(agentSkills.map((s) => s.id)), [agentSkills]);

  const managedById = useMemo(
    () => new Map(managedSkills.map((skill) => [skill.id, skill])),
    [managedSkills],
  );

  const creatorOf = (skill: ProjectSkill) =>
    copyCreator(
      skill.center_skill_id ? managedById.get(skill.center_skill_id) : undefined,
      skill.author,
    );

  const managedLocalIds = useMemo(
    () =>
      new Set(
        localSkills
          .map((skill) => skill.center_skill_id)
          .filter((id): id is string => !!id && installedIds.has(id)),
      ),
    [installedIds, localSkills],
  );

  const managedLocalCount = useMemo(
    () =>
      localSkills.filter(
        (skill) => !!skill.center_skill_id && managedLocalIds.has(skill.center_skill_id),
      ).length,
    [localSkills, managedLocalIds],
  );

  const localSkillKey = (skill: ProjectSkill) => `${skill.agent}:${skill.relative_path}`;

  const {
    isMultiSelect,
    setIsMultiSelect,
    selectedIds,
    toggleSelect,
    isAllSelected,
    handleSelectAll,
    exitMultiSelect,
    removeSelected,
  } = useMultiSelect({
    items: localSkills,
    filtered: visibleLocalSkills,
    getKey: localSkillKey,
    isItemActive: () => true,
    filterSignal: JSON.stringify([search, [...tagFilters].sort()]),
    scopeSignal: `${activeHostId ?? "local"}:${agentKey ?? ""}`,
    escapeEnabled: !batchUnsyncTarget && !batchDeleteTarget && !batchRunning,
  });

  /**
   * "Remove" means two different things here, so the toolbar never merges them:
   * a managed skill is unsynced (the central copy survives), while an unmanaged
   * skill is deleted from disk for good. Sync status does not gate deletion: a
   * hand-copied folder that happens to match a library skill is still the
   * agent's own copy, and the library copy is untouched either way.
   */
  const selectedUnsyncable = useMemo(
    () =>
      visibleLocalSkills.filter(
        (skill) =>
          selectedIds.has(localSkillKey(skill)) &&
          !!skill.center_skill_id &&
          managedLocalIds.has(skill.center_skill_id),
      ),
    [visibleLocalSkills, selectedIds, managedLocalIds],
  );

  const selectedDeletable = useMemo(
    () =>
      visibleLocalSkills.filter(
        (skill) =>
          selectedIds.has(localSkillKey(skill)) &&
          !(skill.center_skill_id && managedLocalIds.has(skill.center_skill_id)),
      ),
    [visibleLocalSkills, selectedIds, managedLocalIds],
  );

  const refreshWorkspaceAfterBatch = async (
    hostId: null | string,
    toolKey: string,
    viewVersion: number,
  ) => {
    const isCurrentView = () =>
      workspaceMountedRef.current &&
      workspaceIdentityRef.current.version === viewVersion &&
      workspaceIdentityRef.current.hostId === hostId &&
      workspaceIdentityRef.current.agentKey === toolKey;

    const localRequestId = isCurrentView() ? ++localSkillsRequestRef.current : null;

    if (localRequestId !== null) setLocalSkillsLoading(true);

    const [managedResult, toolsResult, localResult] = await Promise.allSettled([
      refreshQuery(queryClient, managedSkillsQueryOptions(hostId)),
      refreshQuery(queryClient, toolsQueryOptions(hostId)),
      invokeHost<ProjectSkill[]>(hostId, "get_global_local_skills", { agent: toolKey }),
    ]);

    if (managedResult.status === "rejected")
      console.error(
        "Failed to refresh managed skills after workspace batch action:",
        managedResult.reason,
      );

    if (toolsResult.status === "rejected")
      console.error("Failed to refresh agents after workspace batch action:", toolsResult.reason);

    const refreshFailed =
      managedResult.status === "rejected" ||
      toolsResult.status === "rejected" ||
      localResult.status === "rejected";

    if (localResult.status === "rejected") {
      console.error(
        "Failed to refresh local workspace skills after batch action:",
        localResult.reason,
      );
    } else if (
      localRequestId !== null &&
      isCurrentView() &&
      localSkillsRequestRef.current === localRequestId
    ) {
      setLocalSkills(localResult.value);
    }

    if (
      refreshFailed &&
      localRequestId !== null &&
      isCurrentView() &&
      localSkillsRequestRef.current === localRequestId
    ) {
      toast.error(t("globalWorkspace.batchRefreshFailed"));
    }

    if (
      localRequestId !== null &&
      isCurrentView() &&
      localSkillsRequestRef.current === localRequestId
    ) {
      setLocalSkillsLoading(false);
    }
  };

  const isCurrentBatchTarget = (target: BatchWorkspaceTarget) =>
    workspaceMountedRef.current &&
    workspaceIdentityRef.current.version === target.viewVersion &&
    workspaceIdentityRef.current.hostId === target.hostId &&
    workspaceIdentityRef.current.agentKey === target.agentKey;

  const handleBatchUnsync = async () => {
    const target = batchUnsyncTarget;

    if (!target || batchRunning) return;

    if (!isCurrentBatchTarget(target)) {
      setBatchUnsyncTarget(null);

      return;
    }

    setBatchRunning(true);
    setBatchAction("unsync");
    setBatchFailures((current) => ({ ...current, unsync: [] }));
    const removedKeys: string[] = [];
    const failures: Array<{ name: string; error: string }> = [];

    try {
      for (const skill of target.skills) {
        if (!skill.center_skill_id) {
          failures.push({ name: skill.name, error: t("common.error") });
          continue;
        }

        try {
          await invokeHost<void>(target.hostId, "unsync_skill_from_tool", {
            skillId: skill.center_skill_id,
            tool: target.agentKey,
          });
          removedKeys.push(localSkillKey(skill));
        } catch (error: unknown) {
          failures.push({ name: skill.name, error: getErrorMessage(error, t("common.error")) });
        }
      }

      if (isCurrentBatchTarget(target)) {
        removeSelected(removedKeys);
        setBatchFailures((current) => ({ ...current, unsync: failures }));

        if (removedKeys.length > 0)
          toast.success(t("globalWorkspace.batchRemoved", { count: removedKeys.length }));

        if (failures.length > 0)
          toast.error(t("globalWorkspace.batchRemoveFailed", { count: failures.length }));

        if (failures.length === 0 && selectedIds.size === removedKeys.length) exitMultiSelect();
        setBatchUnsyncTarget(null);
      }
    } finally {
      await refreshWorkspaceAfterBatch(target.hostId, target.agentKey, target.viewVersion);
      setBatchRunning(false);
      setBatchAction(null);

      if (isCurrentBatchTarget(target)) setBatchUnsyncTarget(null);
    }
  };

  const handleBatchDeleteLocal = async () => {
    const target = batchDeleteTarget;

    if (!target || batchRunning) return;

    if (!isCurrentBatchTarget(target)) {
      setBatchDeleteTarget(null);

      return;
    }

    setBatchRunning(true);
    setBatchAction("local-delete");
    setBatchFailures((current) => ({ ...current, localDelete: [] }));
    const deletedKeys: string[] = [];
    const failures: Array<{ name: string; error: string }> = [];

    try {
      for (const skill of target.skills) {
        try {
          await invokeHost<void>(target.hostId, "delete_global_local_skill", {
            agent: target.agentKey,
            skillRelativePath: skill.relative_path,
          });
          deletedKeys.push(localSkillKey(skill));
        } catch (error: unknown) {
          failures.push({ name: skill.name, error: getErrorMessage(error, t("common.error")) });
        }
      }

      if (isCurrentBatchTarget(target)) {
        removeSelected(deletedKeys);
        setBatchFailures((current) => ({ ...current, localDelete: failures }));

        if (deletedKeys.length > 0)
          toast.success(
            t("globalWorkspace.localSkills.deletedLocalBatch", { count: deletedKeys.length }),
          );

        if (failures.length > 0)
          toast.error(
            t("globalWorkspace.localSkills.deleteLocalBatchFailed", { count: failures.length }),
          );

        if (failures.length === 0 && selectedIds.size === deletedKeys.length) exitMultiSelect();
        setBatchDeleteTarget(null);
      }
    } finally {
      await refreshWorkspaceAfterBatch(target.hostId, target.agentKey, target.viewVersion);
      setBatchRunning(false);
      setBatchAction(null);

      if (isCurrentBatchTarget(target)) setBatchDeleteTarget(null);
    }
  };

  const captureBatchTarget = (skills: ProjectSkill[]): BatchWorkspaceTarget | null => {
    if (!currentTool || !currentToolKey) return null;

    return {
      hostId: activeHostId,
      agentKey: currentToolKey,
      agentName: currentTool.display_name,
      viewVersion: workspaceIdentityRef.current.version,
      skills: [...skills],
    };
  };

  const handleRemoveLocalManagedSkill = async (skill: ProjectSkill) => {
    if (!agentKey || !skill.center_skill_id || !managedLocalIds.has(skill.center_skill_id)) return;
    setRemovingLocalSkillId(skill.relative_path);

    try {
      await api.unsyncSkillFromTool(skill.center_skill_id, agentKey);
      await Promise.all([refreshManagedSkills(), refreshTools(), loadLocalSkills()]);
      toast.success(t("globalWorkspace.removedToast", { name: skill.name }));
    } catch (e) {
      toast.error(getErrorMessage(e, t("common.error")));
    } finally {
      setRemovingLocalSkillId(null);
    }
  };

  const handleSheetInstalled = useCallback(async () => {
    await Promise.all([refreshManagedSkills(), refreshTools(), loadLocalSkills()]);
  }, [loadLocalSkills, refreshManagedSkills, refreshTools]);

  const handleUploadLocalSkill = useCallback(
    async (skill: ProjectSkill) => {
      if (!currentTool) return;
      const key = `upload:${skill.relative_path}`;
      setLocalActionKey(key);

      try {
        await api.importGlobalLocalSkillToCenter(currentTool.key, skill.relative_path);
        toast.success(
          t("globalWorkspace.localSkills.uploadedToast", {
            name: skill.name,
            agent: currentTool.display_name,
          }),
        );
        await Promise.all([loadLocalSkills(), refreshManagedSkills()]);
      } catch (error: unknown) {
        toast.error(getErrorMessage(error, t("common.error")));
      } finally {
        setLocalActionKey(null);
        setUploadConfirmSkill(null);
      }
    },
    [currentTool, loadLocalSkills, refreshManagedSkills, t],
  );

  const handleDeleteLocalSkill = useCallback(
    async (skill: ProjectSkill) => {
      if (!currentTool) return;
      const key = `delete:${skill.relative_path}`;
      setLocalActionKey(key);

      try {
        await api.deleteGlobalLocalSkill(currentTool.key, skill.relative_path);
        toast.success(
          t("globalWorkspace.localSkills.deletedLocalToast", {
            name: skill.name,
            agent: currentTool.display_name,
          }),
        );
        await loadLocalSkills();
      } catch (error: unknown) {
        toast.error(getErrorMessage(error, t("common.error")));
      } finally {
        setLocalActionKey(null);
        setDeleteLocalConfirmSkill(null);
      }
    },
    [currentTool, loadLocalSkills, t],
  );

  const handlePullLocalSkill = useCallback(
    async (skill: ProjectSkill) => {
      if (!currentTool) return;
      const key = `pull:${skill.relative_path}`;
      setLocalActionKey(key);

      try {
        await api.updateGlobalLocalSkillFromCenter(currentTool.key, skill.relative_path);
        toast.success(
          t("globalWorkspace.localSkills.pulledToast", {
            name: skill.name,
            agent: currentTool.display_name,
          }),
        );
        await loadLocalSkills();
      } catch (error: unknown) {
        toast.error(getErrorMessage(error, t("common.error")));
      } finally {
        setLocalActionKey(null);
        setPullConfirmSkill(null);
      }
    },
    [currentTool, loadLocalSkills, t],
  );

  const openLocalDetail = useCallback(
    async (skill: ProjectSkill) => {
      if (!currentTool) return;
      const requestId = localDetailRequestRef.current + 1;
      localDetailRequestRef.current = requestId;
      setLocalDetailSkill(skill);
      setLocalContentTab("local");
      setLocalDocContent(null);
      setLocalCenterDocContent(null);
      setLocalDocLoading(true);
      setLocalCenterDocLoading(!!skill.center_skill_id);

      api
        .getGlobalLocalSkillDocument(currentTool.key, skill.relative_path)
        .then((doc) => {
          if (localDetailRequestRef.current === requestId) setLocalDocContent(doc.content);
        })
        .catch(() => {
          if (localDetailRequestRef.current === requestId) setLocalDocContent(null);
        })
        .finally(() => {
          if (localDetailRequestRef.current === requestId) setLocalDocLoading(false);
        });

      if (skill.center_skill_id) {
        api
          .getSkillDocument(skill.center_skill_id)
          .then((doc) => {
            if (localDetailRequestRef.current === requestId) setLocalCenterDocContent(doc.content);
          })
          .catch(() => {
            if (localDetailRequestRef.current === requestId) setLocalCenterDocContent(null);
          })
          .finally(() => {
            if (localDetailRequestRef.current === requestId) setLocalCenterDocLoading(false);
          });
      }
    },
    [currentTool],
  );

  const existsInGlobal = useCallback(
    (skill: ManagedSkill, agentK: string) => skill.targets.some((target) => target.tool === agentK),
    [],
  );

  const handlePresetAdd = useCallback(async (skill: ManagedSkill, agentK: string) => {
    await api.syncSkillToTool(skill.id, agentK);
  }, []);

  const handlePresetRemove = useCallback(async (skill: ManagedSkill, agentK: string) => {
    await api.unsyncSkillFromTool(skill.id, agentK);
  }, []);

  const handlePresetComplete = useCallback(async () => {
    await Promise.all([refreshManagedSkills(), refreshTools(), loadLocalSkills()]);
  }, [loadLocalSkills, refreshManagedSkills, refreshTools]);

  const renderLocalSkillActions = (skill: ProjectSkill, variant: "grid" | "list") => {
    const uploadKey = `upload:${skill.relative_path}`;
    const pullKey = `pull:${skill.relative_path}`;
    const deleteKey = `delete:${skill.relative_path}`;
    const canPull = skill.sync_status === "center_newer" || skill.sync_status === "diverged";
    const isInSync = skill.sync_status === "in_sync";
    const isManaged = !!skill.center_skill_id && managedLocalIds.has(skill.center_skill_id);
    const removing = removingLocalSkillId === skill.relative_path;

    const buttonClassName =
      variant === "grid"
        ? "rounded px-2 py-1 text-[13px] font-medium text-muted transition-colors outline-none hover:bg-surface-hover hover:text-secondary disabled:opacity-50"
        : "rounded p-0.5 text-muted transition-colors hover:bg-surface-hover hover:text-secondary disabled:opacity-50";

    return (
      <>
        {!isInSync && canPull && (
          <button
            onClick={(e) => {
              e.stopPropagation();
              setPullConfirmSkill(skill);
            }}
            disabled={localActionKey === pullKey}
            className={buttonClassName}
            title={t("globalWorkspace.localSkills.pull")}
          >
            {localActionKey === pullKey ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Download className="h-3.5 w-3.5" />
            )}
          </button>
        )}

        {!isInSync && (
          <button
            onClick={(e) => {
              e.stopPropagation();

              if (skill.sync_status === "project_only") {
                void handleUploadLocalSkill(skill);
              } else {
                setUploadConfirmSkill(skill);
              }
            }}
            disabled={localActionKey === uploadKey}
            className={buttonClassName}
            title={t("globalWorkspace.localSkills.upload")}
          >
            {localActionKey === uploadKey ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Upload className="h-3.5 w-3.5" />
            )}
          </button>
        )}

        {isManaged ? (
          <button
            onClick={(e) => {
              e.stopPropagation();
              void handleRemoveLocalManagedSkill(skill);
            }}
            disabled={removing}
            title={t("globalWorkspace.localSkills.removeManaged")}
            className={cn(buttonClassName, "hover:bg-red-500/10 hover:text-red-500")}
          >
            {removing ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Trash2 className="h-3.5 w-3.5" />
            )}
          </button>
        ) : (
          <button
            onClick={(e) => {
              e.stopPropagation();
              setDeleteLocalConfirmSkill(skill);
            }}
            disabled={localActionKey === deleteKey}
            title={t("globalWorkspace.localSkills.deleteLocal")}
            className={cn(buttonClassName, "hover:bg-red-500/10 hover:text-red-500")}
          >
            {localActionKey === deleteKey ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Trash2 className="h-3.5 w-3.5" />
            )}
          </button>
        )}
      </>
    );
  };

  if (redirectTarget) {
    return <Navigate to={redirectTarget} params={{ agentKey }} replace />;
  }

  if (installedTools.length === 0) {
    return (
      <ResourceWorkspace
        scope={{ kind: "global", category: config.category }}
        skills={
          <div className="app-page">
            <div className="app-panel flex flex-col items-center justify-center py-16 text-center">
              <div className="mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-surface-hover">
                <Globe className="h-5 w-5 text-muted" />
              </div>
              <p className="text-[13px] font-medium text-secondary">
                {t(config.i18nKeys.noAgents)}
              </p>
              <p className="mt-1 max-w-[260px] text-[12px] leading-relaxed text-muted">
                {t(config.i18nKeys.noAgentsHint)}
              </p>
            </div>
          </div>
        }
      />
    );
  }

  if (!currentTool) {
    return (
      <ResourceWorkspace
        scope={{ kind: "global", category: config.category }}
        skills={
          <div className="app-page">
            <div className="app-page-header flex flex-col gap-2.5 pb-3 pr-2">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div className="min-w-0 flex-1">
                  <h1 className="app-page-title flex items-center gap-2.5">
                    <Globe className="h-5 w-5 text-accent" />
                    {t(config.i18nKeys.title)}
                    <span className="app-badge">{installedTools.length}</span>
                  </h1>
                </div>
              </div>

              {presets.length > 0 && (
                <PresetBar
                  presets={presets}
                  managedSkills={managedSkills}
                  agentKeys={presetBarAgentKeys}
                  existsInWorkspace={existsInGlobal}
                  onAddSkill={handlePresetAdd}
                  onRemoveSkill={handlePresetRemove}
                  onComplete={handlePresetComplete}
                />
              )}
            </div>

            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
              {installedTools.map((tool) => {
                const count = localCountByAgent[tool.key] ?? skillCountByAgent[tool.key] ?? 0;

                return (
                  <button
                    key={tool.key}
                    onClick={() =>
                      navigate({ to: config.routePath, params: { agentKey: tool.key } })
                    }
                    className="app-panel group flex items-center gap-3 p-3.5 text-left transition-all hover:border-border hover:bg-surface-hover"
                  >
                    <AgentIcon
                      agentKey={tool.key}
                      displayName={tool.display_name}
                      className="h-9 w-9 rounded-lg transition-colors group-hover:border-border"
                    />
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-[13px] font-semibold text-secondary">
                        {tool.display_name}
                      </p>
                      <p className="text-[12px] text-muted">
                        {t("globalWorkspace.skillCount", { count })}
                      </p>
                    </div>
                    <ChevronRight className="h-4 w-4 shrink-0 text-faint transition-transform group-hover:translate-x-0.5" />
                  </button>
                );
              })}
            </div>
          </div>
        }
      />
    );
  }

  return (
    <ResourceWorkspace
      scope={{ kind: "global", agentKey: currentTool.key, category: config.category }}
      skills={
        <div className="app-page">
          {/* Header */}
          <div className="app-page-header flex flex-col gap-2.5 pb-3 pr-2">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="min-w-0 flex-[1_1_360px]">
                <h1 className="app-page-title flex items-center gap-2.5">
                  <AgentIcon
                    agentKey={currentTool.key}
                    displayName={currentTool.display_name}
                    className="h-7 w-7 rounded-lg"
                  />
                  {currentTool.display_name}
                  <span className="app-badge">{localSkills.length}</span>
                </h1>
                <p className="mt-1 truncate text-[13px] text-muted" title={currentTool.skills_dir}>
                  {compactHomePath(currentTool.skills_dir)}
                  <span className="px-1.5">·</span>
                  {t("globalWorkspace.localSkills.summary", {
                    total: localSkills.length,
                    managed: managedLocalCount,
                    synced: inSyncLocalCount,
                  })}
                </p>
              </div>

              <div className="flex min-w-0 flex-[2_1_520px] flex-wrap items-center justify-end gap-2">
                <div className="relative w-full min-w-[220px] max-w-[320px]">
                  <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted" />
                  <input
                    type="text"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                    disabled={batchSelectionLocked}
                    placeholder={t("globalWorkspace.localSkills.searchPlaceholder")}
                    className="app-input w-full pl-8 font-medium"
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                  />
                </div>

                <div className="app-segmented app-toolbar-segmented shrink-0">
                  <button
                    onClick={() => void loadLocalSkills()}
                    disabled={localSkillsLoading || batchSelectionLocked}
                    className="rounded-md p-2 text-muted outline-none transition-colors hover:text-tertiary disabled:opacity-50"
                    title={t("settings.refresh")}
                  >
                    <RefreshCw className={cn("h-4 w-4", localSkillsLoading && "animate-spin")} />
                  </button>
                  <button
                    onClick={() => setViewMode("grid")}
                    className={cn(
                      "rounded-md p-2 transition-colors outline-none",
                      viewMode === "grid"
                        ? "bg-surface-active text-secondary"
                        : "text-muted hover:text-tertiary",
                    )}
                  >
                    <LayoutGrid className="h-4 w-4" />
                  </button>
                  <button
                    onClick={() => setViewMode("list")}
                    className={cn(
                      "rounded-md p-2 transition-colors outline-none",
                      viewMode === "list"
                        ? "bg-surface-active text-secondary"
                        : "text-muted hover:text-tertiary",
                    )}
                  >
                    <List className="h-4 w-4" />
                  </button>

                  {/* Selection can stay active in either view. */}
                  <span
                    aria-hidden="true"
                    className="mx-1 h-5 w-px shrink-0 self-center bg-border-subtle"
                  />
                  <button
                    type="button"
                    aria-pressed={isMultiSelect}
                    disabled={batchSelectionLocked}
                    onClick={() => (isMultiSelect ? exitMultiSelect() : setIsMultiSelect(true))}
                    className={cn(
                      "app-segmented-button inline-flex items-center gap-1.5 hover:bg-surface-hover focus-visible:ring-2 focus-visible:ring-border",
                      isMultiSelect &&
                        "app-segmented-button-active hover:bg-surface-active hover:text-secondary",
                    )}
                  >
                    <SquareCheck className="h-4 w-4" />
                    {isMultiSelect
                      ? t("globalWorkspace.cancelSelect")
                      : t("globalWorkspace.selectMode")}
                  </button>
                </div>

                <button
                  onClick={() => setAddDialogOpen(true)}
                  className="app-toolbar-button app-toolbar-button-primary"
                >
                  <Plus className="h-3.5 w-3.5" />
                  {t("globalWorkspace.addSkill")}
                </button>
              </div>
            </div>

            {allLocalTags.length > 0 && (
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="text-[12px] text-muted">{t("mySkills.tags.filter")}</span>
                <button
                  disabled={batchSelectionLocked}
                  onClick={() => setTagFilters(new Set())}
                  className={cn(
                    "rounded-full px-2.5 py-0.5 text-[12px] font-medium transition-colors",
                    tagFilters.size === 0
                      ? "bg-accent text-white dark:bg-accent dark:text-white"
                      : "bg-surface-hover text-muted hover:text-secondary",
                  )}
                >
                  {t("mySkills.tags.allTags")}
                </button>
                {localSkills.some((s) => s.tags.length === 0) &&
                  (() => {
                    const isActive = tagFilters.has(UNTAGGED_FILTER);

                    return (
                      <button
                        disabled={batchSelectionLocked}
                        onClick={() => {
                          setTagFilters((prev) => {
                            const next = new Set(prev);

                            if (next.has(UNTAGGED_FILTER)) next.delete(UNTAGGED_FILTER);
                            else next.add(UNTAGGED_FILTER);

                            return next;
                          });
                        }}
                        className={cn(
                          "inline-flex items-center gap-1 rounded-full px-2.5 py-0.5 text-[12px] font-medium transition-colors",
                          isActive
                            ? "bg-surface-active text-primary"
                            : "border border-dashed border-border text-muted hover:text-secondary",
                        )}
                        title={t("mySkills.tags.untagged")}
                      >
                        <CircleSlash className="h-3 w-3" />
                        {t("mySkills.tags.untagged")}
                      </button>
                    );
                  })()}
                {allLocalTags.map((tag) => {
                  const active = tagFilters.has(tag);

                  return (
                    <button
                      key={tag}
                      disabled={batchSelectionLocked}
                      onClick={() => {
                        setTagFilters((prev) => {
                          const next = new Set(prev);

                          if (next.has(tag)) next.delete(tag);
                          else next.add(tag);

                          return next;
                        });
                      }}
                      className={cn(
                        "rounded-full px-2.5 py-0.5 text-[12px] font-medium transition-colors",
                        active
                          ? getTagActiveColor(tag, allLocalTags)
                          : getTagColor(tag, allLocalTags),
                      )}
                    >
                      {tag}
                    </button>
                  );
                })}
              </div>
            )}

            {/* Preset bar */}
            {presets.length > 0 && (
              <PresetBar
                presets={presets}
                managedSkills={managedSkills}
                agentKeys={presetBarAgentKeys}
                existsInWorkspace={existsInGlobal}
                onAddSkill={handlePresetAdd}
                onRemoveSkill={handlePresetRemove}
                onComplete={handlePresetComplete}
              />
            )}
          </div>

          {localSkillsLoading ? (
            <div className="flex items-center gap-2 py-4 text-[13px] text-muted">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              {t("common.loading")}
            </div>
          ) : visibleLocalSkills.length === 0 ? (
            <div className="flex min-h-[260px] flex-col items-center justify-center px-4 text-center">
              <Globe className="mb-4 h-12 w-12 text-faint" />
              <h3 className="mb-1.5 text-[14px] font-semibold text-tertiary">
                {localSkills.length === 0
                  ? t("globalWorkspace.localSkills.empty")
                  : t("mySkills.noMatch")}
              </h3>
              {localSkills.length === 0 && (
                <button
                  onClick={() => setAddDialogOpen(true)}
                  className="hover:bg-accent-hover mt-4 inline-flex items-center gap-1.5 rounded-md bg-accent px-4 py-2 text-[13px] font-medium text-white transition-colors"
                >
                  <Plus className="h-3.5 w-3.5" />
                  {t("globalWorkspace.addSkill")}
                </button>
              )}
            </div>
          ) : (
            <>
              {isMultiSelect && (
                <MultiSelectToolbar
                  selectedCount={selectedIds.size}
                  isAllSelected={isAllSelected}
                  actions={[
                    ...(selectedUnsyncable.length > 0
                      ? [
                          {
                            key: "unsync",
                            label: t("globalWorkspace.deleteSelected", {
                              count: selectedUnsyncable.length,
                              agent: currentTool?.display_name ?? "",
                            }),
                            icon: <Unlink className="h-3.5 w-3.5" />,
                            busy: batchRunning && batchAction === "unsync",
                            onSelect: () => {
                              const target = captureBatchTarget(selectedUnsyncable);

                              if (target && !batchSelectionLocked) setBatchUnsyncTarget(target);
                            },
                          },
                        ]
                      : []),
                    ...(selectedDeletable.length > 0
                      ? [
                          {
                            key: "delete",
                            tone: "danger" as const,
                            label: t("globalWorkspace.localSkills.deleteLocalSelected", {
                              count: selectedDeletable.length,
                            }),
                            icon: <Trash2 className="h-3.5 w-3.5" />,
                            busy: batchRunning && batchAction === "local-delete",
                            onSelect: () => {
                              const target = captureBatchTarget(selectedDeletable);

                              if (target && !batchSelectionLocked) setBatchDeleteTarget(target);
                            },
                          },
                        ]
                      : []),
                  ]}
                  labels={{
                    hint: t("globalWorkspace.selectHint"),
                    selected: t("globalWorkspace.selectedCount", { count: selectedIds.size }),
                    selectAll: t("globalWorkspace.selectAll"),
                    deselectAll: t("globalWorkspace.deselectAll"),
                    cancel: t("common.cancel"),
                    more: t("mySkills.moreActions"),
                  }}
                  onSelectAll={handleSelectAll}
                  onCancel={exitMultiSelect}
                  disabled={batchSelectionLocked}
                />
              )}
              {(batchFailures.unsync.length > 0 || batchFailures.localDelete.length > 0) && (
                <div
                  role="alert"
                  className="border-danger/30 bg-danger/5 mb-3 space-y-1 rounded-lg border px-3 py-2 text-[12px] text-danger"
                >
                  {batchFailures.unsync.length > 0 && (
                    <p>
                      {t("globalWorkspace.batchRemovePartialFailure", {
                        count: batchFailures.unsync.length,
                        agent: currentTool?.display_name ?? "",
                        details: batchFailures.unsync
                          .map(({ name, error }) => `${name}: ${error}`)
                          .join("; "),
                      })}
                    </p>
                  )}
                  {batchFailures.localDelete.length > 0 && (
                    <p>
                      {t("globalWorkspace.localSkills.deleteLocalBatchPartialFailure", {
                        count: batchFailures.localDelete.length,
                        agent: currentTool?.display_name ?? "",
                        details: batchFailures.localDelete
                          .map(({ name, error }) => `${name}: ${error}`)
                          .join("; "),
                      })}
                    </p>
                  )}
                </div>
              )}
              <div
                className={cn(
                  "pb-8",
                  viewMode === "grid"
                    ? "grid grid-cols-2 gap-3 lg:grid-cols-3"
                    : "flex flex-col gap-0.5",
                )}
              >
                {visibleLocalSkills.map((skill) => {
                  const statusMeta = getLocalStatusMeta(t, skill.sync_status);

                  const isManaged =
                    !!skill.center_skill_id && managedLocalIds.has(skill.center_skill_id);

                  const key = localSkillKey(skill);

                  return (
                    <WorkspaceSkillCard
                      key={key}
                      viewMode={viewMode}
                      title={skill.name}
                      description={skill.description || skill.relative_path}
                      tags={skill.tags.map((tag) => ({
                        label: tag,
                        className: getTagColor(tag, allLocalTags),
                      }))}
                      status={statusMeta}
                      creator={creatorOf(skill)}
                      fileCount={skill.files.length}
                      active={isManaged}
                      actions={isMultiSelect ? undefined : renderLocalSkillActions(skill, viewMode)}
                      actionsHover={viewMode === "list"}
                      selectable={isMultiSelect}
                      selected={selectedIds.has(key)}
                      onClick={() =>
                        isMultiSelect
                          ? !batchSelectionLocked && toggleSelect(key)
                          : void openLocalDetail(skill)
                      }
                    />
                  );
                })}
              </div>
            </>
          )}

          {currentTool && (
            <AddSkillsSheet
              open={addDialogOpen}
              onClose={() => setAddDialogOpen(false)}
              target={{
                kind: "global",
                agentKey: currentTool.key,
                agentDisplayName: currentTool.display_name,
                installedSkillIds: installedIds,
              }}
              managedSkills={managedSkills}
              onInstalled={handleSheetInstalled}
            />
          )}

          <DetailSheet
            open={!!localDetailSkill}
            title={localDetailSkill?.name ?? ""}
            description={localDetailSkill?.description}
            meta={
              localDetailSkill ? (
                <div className="flex flex-wrap items-center gap-2">
                  <CreatorBadge creator={creatorOf(localDetailSkill)} size="md" />
                  <span
                    className={cn(
                      "rounded-full px-2.5 py-1 text-[12px] font-medium",
                      getLocalStatusMeta(t, localDetailSkill.sync_status).className,
                    )}
                  >
                    {getLocalStatusMeta(t, localDetailSkill.sync_status).label}
                  </span>
                  <span className="rounded-full bg-surface-hover px-2.5 py-1 text-[12px] text-muted">
                    {localDetailSkill.relative_path}
                  </span>
                </div>
              ) : null
            }
            onClose={() => setLocalDetailSkill(null)}
          >
            {localDetailSkill?.center_skill_id && (
              <div className="mb-4 flex flex-wrap items-center gap-2">
                {(["local", "diff", "center"] as const).map((tab) => (
                  <button
                    key={tab}
                    type="button"
                    onClick={() => setLocalContentTab(tab)}
                    className={cn(
                      "rounded-full px-3 py-1.5 text-[12px] font-medium transition-colors",
                      localContentTab === tab
                        ? "bg-accent text-white"
                        : "bg-surface-hover text-muted hover:text-secondary",
                    )}
                    disabled={(tab === "diff" || tab === "center") && localCenterDocLoading}
                  >
                    {tab === "local"
                      ? t("mySkills.docTabs.local")
                      : tab === "diff"
                        ? t("mySkills.docTabs.diff")
                        : t("project.docTabs.center")}
                  </button>
                ))}
              </div>
            )}

            {localDocLoading ? (
              <div className="mt-12 text-center text-[13px] text-muted">{t("common.loading")}</div>
            ) : localContentTab === "diff" ? (
              localDocContent && localCenterDocContent ? (
                <DocumentDiffViewer original={localDocContent} updated={localCenterDocContent} />
              ) : localCenterDocLoading ? (
                <div className="mt-12 text-center text-[13px] text-muted">
                  {t("common.loading")}
                </div>
              ) : (
                <div className="mt-12 text-center text-[13px] text-muted">
                  {t("mySkills.sourceDiffUnavailable")}
                </div>
              )
            ) : localContentTab === "center" ? (
              localCenterDocLoading ? (
                <div className="mt-12 text-center text-[13px] text-muted">
                  {t("common.loading")}
                </div>
              ) : localCenterDocContent ? (
                <SkillMarkdown content={localCenterDocContent} />
              ) : (
                <div className="mt-12 text-center text-[13px] text-muted">
                  {t("mySkills.sourceDiffUnavailable")}
                </div>
              )
            ) : localDocContent ? (
              <SkillMarkdown content={localDocContent} />
            ) : (
              <div className="mt-12 text-center text-[13px] text-muted">
                {t("common.documentMissing")}
              </div>
            )}
          </DetailSheet>

          <ConfirmDialog
            open={!!uploadConfirmSkill}
            title={t("globalWorkspace.localSkills.uploadConfirmTitle")}
            message={t("globalWorkspace.localSkills.uploadConfirmMessage", {
              name: uploadConfirmSkill?.name ?? "",
            })}
            tone="warning"
            confirmLabel={t("globalWorkspace.localSkills.upload")}
            onClose={() => setUploadConfirmSkill(null)}
            onConfirm={() =>
              uploadConfirmSkill ? handleUploadLocalSkill(uploadConfirmSkill) : Promise.resolve()
            }
          />
          <ConfirmDialog
            open={!!pullConfirmSkill}
            title={t("globalWorkspace.localSkills.pullConfirmTitle")}
            message={t("globalWorkspace.localSkills.pullConfirmMessage", {
              name: pullConfirmSkill?.name ?? "",
              agent: currentTool?.display_name ?? "",
            })}
            tone="danger"
            confirmLabel={t("globalWorkspace.localSkills.pull")}
            onClose={() => setPullConfirmSkill(null)}
            onConfirm={() =>
              pullConfirmSkill ? handlePullLocalSkill(pullConfirmSkill) : Promise.resolve()
            }
          />
          <ConfirmDialog
            open={!!batchUnsyncTarget}
            title={t("globalWorkspace.removeSkill")}
            message={t("globalWorkspace.batchRemoveConfirm", {
              count: batchUnsyncTarget?.skills.length ?? 0,
              agent: batchUnsyncTarget?.agentName ?? "",
            })}
            details={batchUnsyncTarget?.skills.map((skill) => skill.name)}
            tone="warning"
            confirmLabel={t("globalWorkspace.removeSkill")}
            lockWhilePending
            onClose={() => setBatchUnsyncTarget(null)}
            onConfirm={handleBatchUnsync}
          />

          <ConfirmDialog
            open={!!batchDeleteTarget}
            title={t("globalWorkspace.localSkills.deleteLocalConfirmTitle")}
            message={t("globalWorkspace.localSkills.deleteLocalBatchConfirm", {
              count: batchDeleteTarget?.skills.length ?? 0,
              agent: batchDeleteTarget?.agentName ?? "",
            })}
            details={batchDeleteTarget?.skills.map((skill) => skill.relative_path)}
            confirmLabel={t("common.delete")}
            lockWhilePending
            onClose={() => setBatchDeleteTarget(null)}
            onConfirm={handleBatchDeleteLocal}
          />

          <ConfirmDialog
            open={!!deleteLocalConfirmSkill}
            title={t("globalWorkspace.localSkills.deleteLocalConfirmTitle")}
            message={t("globalWorkspace.localSkills.deleteLocalConfirmMessage", {
              name: deleteLocalConfirmSkill?.name ?? "",
              agent: currentTool?.display_name ?? "",
            })}
            tone="danger"
            confirmLabel={t("common.delete")}
            onClose={() => setDeleteLocalConfirmSkill(null)}
            onConfirm={() =>
              deleteLocalConfirmSkill
                ? handleDeleteLocalSkill(deleteLocalConfirmSkill)
                : Promise.resolve()
            }
          />
        </div>
      }
    />
  );
}
