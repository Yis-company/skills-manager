import {
  closestCenter,
  DndContext,
  type DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { useQueryClient } from "@tanstack/react-query";
import { Link, useLocation, useNavigate } from "@tanstack/react-router";
import {
  ChevronDown,
  ChevronRight,
  CloudUpload,
  Download,
  FolderOpen,
  Globe,
  GripVertical,
  Layers,
  LayoutDashboard,
  Link2,
  Pencil,
  Plus,
  Settings,
  Trash2,
} from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { type PresetsData, queryKeys } from "../lib/appQueries";
import { getPresetIconOption } from "../lib/presetIcons";
import { applyStoredOrder } from "../lib/storedOrder";
import * as api from "../lib/tauri";
import type { SyncHealth, ToolCategory, ToolInfo } from "../lib/tauri";
import { cn } from "../utils";
import { isSettingsPath } from "../views/settings/categories";
import {
  CODING_WORKSPACE_CONFIG,
  LOBSTER_WORKSPACE_CONFIG,
  type WorkspaceConfig,
} from "../views/workspaceConfigs";
import { AddProjectDialog } from "./AddProjectDialog";
import { AgentIcon } from "./AgentIcon";
import { ConfirmDialog } from "./ConfirmDialog";
import { CreatePresetDialog } from "./CreatePresetDialog";
import { HostSwitcher } from "./HostSwitcher";
import { RenamePresetDialog } from "./RenamePresetDialog";
import { SortableItem } from "./SortableItem";

function getSyncHealthIndicator(
  health: SyncHealth,
  skillCount: number,
): { color: string; title: string } | null {
  if (skillCount === 0) return null;

  if (health.diverged > 0) return { color: "bg-red-400", title: `${health.diverged} diverged` };

  if (health.project_newer > 0 || health.center_newer > 0) {
    const parts: string[] = [];

    if (health.project_newer > 0) parts.push(`${health.project_newer} project newer`);

    if (health.center_newer > 0) parts.push(`${health.center_newer} center newer`);

    return { color: "bg-amber-400", title: parts.join(", ") };
  }

  if (health.project_only > 0)
    return { color: "bg-blue-400", title: `${health.project_only} project only` };

  if (health.in_sync === skillCount) return { color: "bg-emerald-400", title: "All in sync" };

  return null;
}

/** Returns the list with the dragged item moved to its drop position, or null if nothing moved. */
function moveDragged<T>(
  items: T[],
  { active, over }: DragEndEvent,
  idOf: (item: T) => string,
): null | T[] {
  if (!over || active.id === over.id) return null;
  const from = items.findIndex((item) => idOf(item) === active.id);
  const to = items.findIndex((item) => idOf(item) === over.id);

  if (from === -1 || to === -1) return null;

  return arrayMove(items, from, to);
}

function readStoredOrder(key: string): string[] {
  const stored = localStorage.getItem(key);

  return stored ? JSON.parse(stored) : [];
}

/** Local state seeded from `source`, starting over whenever `source` changes. */
function useStateFrom<S, T>(source: S, init: (source: S) => T) {
  const [state, setState] = useState(() => init(source));
  const [seenSource, setSeenSource] = useState(source);

  if (source !== seenSource) {
    setSeenSource(source);
    setState(init(source));
  }

  return [state, setState] as const;
}

export function Sidebar() {
  const { t } = useTranslation();
  const location = useLocation();
  const navigate = useNavigate();

  const {
    presets,
    viewedPreset,
    setViewedPresetId,
    refreshPresets,
    refreshManagedSkills,
    projects,
    refreshProjects,
    tools,
    managedSkills,
    appUpdate,
    activeHostId,
  } = useApp();

  const queryClient = useQueryClient();
  const [showCreate, setShowCreate] = useState(false);
  const [showAddProject, setShowAddProject] = useState(false);

  const [renameTarget, setRenameTarget] = useState<{
    id: string;
    name: string;
    icon?: null | string;
  } | null>(null);

  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);

  const [deleteProjectTarget, setDeleteProjectTarget] = useState<{
    id: string;
    name: string;
  } | null>(null);

  const installedTools = useMemo(() => tools.filter((t) => t.installed && t.enabled), [tools]);

  const installedCodingTools = useMemo(
    () => installedTools.filter((t) => t.category === "coding"),
    [installedTools],
  );

  const installedLobsterTools = useMemo(
    () => installedTools.filter((t) => t.category === "lobster"),
    [installedTools],
  );

  const [orderedPresets, setOrderedPresets] = useStateFrom(presets, (items) => items);
  const [orderedProjects, setOrderedProjects] = useStateFrom(projects, (items) => items);

  const [orderedCodingTools, setOrderedCodingTools] = useStateFrom(installedCodingTools, (items) =>
    applyStoredOrder(items, readStoredOrder("skills-manager:tool-order")),
  );

  const [orderedLobsterTools, setOrderedLobsterTools] = useStateFrom(
    installedLobsterTools,
    (items) => applyStoredOrder(items, readStoredOrder("skills-manager:lobster-tool-order")),
  );

  const presetReorderQueueRef = useRef<Promise<void>>(Promise.resolve());
  const projectReorderQueueRef = useRef<Promise<void>>(Promise.resolve());
  const [presetsOpen, setPresetsOpen] = useState(true);
  const [projectsOpen, setProjectsOpen] = useState(true);
  const [globalWorkspaceOpen, setGlobalWorkspaceOpen] = useState(true);
  const [lobsterWorkspaceOpen, setLobsterWorkspaceOpen] = useState(true);

  const globalSkillsByAgent = useMemo(() => {
    const map: Record<string, number> = {};

    for (const tool of installedTools) {
      map[tool.key] = managedSkills.filter((skill) =>
        skill.targets.some((target) => target.tool === tool.key),
      ).length;
    }

    return map;
  }, [installedTools, managedSkills]);

  const dragSensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const handleDragEnd = (event: DragEndEvent) => {
    const reordered = moveDragged(orderedPresets, event, (s) => s.id);

    if (!reordered) return;
    const previousOrder = orderedPresets;
    const hostId = activeHostId;
    setOrderedPresets(reordered);

    presetReorderQueueRef.current = presetReorderQueueRef.current
      .catch(() => undefined)
      .then(async () => {
        try {
          await api.reorderPresets(reordered.map((s) => s.id));
        } catch {
          await refreshPresets();
          setOrderedPresets((current) =>
            current === reordered
              ? (queryClient.getQueryData<PresetsData>(queryKeys.presets(hostId))?.presets ??
                previousOrder)
              : current,
          );
          toast.error(t("common.error"));
        }
      });
  };

  const handleProjectDragEnd = (event: DragEndEvent) => {
    const reordered = moveDragged(orderedProjects, event, (p) => p.id);

    if (!reordered) return;
    const previousOrder = orderedProjects;
    const hostId = activeHostId;
    setOrderedProjects(reordered);

    projectReorderQueueRef.current = projectReorderQueueRef.current
      .catch(() => undefined)
      .then(async () => {
        try {
          await api.reorderProjects(reordered.map((p) => p.id));
        } catch {
          await refreshProjects();
          setOrderedProjects((current) =>
            current === reordered
              ? (queryClient.getQueryData(queryKeys.projects(hostId)) ?? previousOrder)
              : current,
          );
          toast.error(t("common.error"));
        }
      });
  };

  const handleToolDragEnd = (category: ToolCategory) => (event: DragEndEvent) => {
    const current = category === "lobster" ? orderedLobsterTools : orderedCodingTools;
    const reordered = moveDragged(current, event, (t) => t.key);

    if (!reordered) return;

    if (category === "lobster") {
      setOrderedLobsterTools(reordered);
      localStorage.setItem(
        "skills-manager:lobster-tool-order",
        JSON.stringify(reordered.map((t) => t.key)),
      );
    } else {
      setOrderedCodingTools(reordered);
      localStorage.setItem(
        "skills-manager:tool-order",
        JSON.stringify(reordered.map((t) => t.key)),
      );
    }
  };

  const NAV_ITEMS = [
    { name: t("sidebar.dashboard"), path: "/", icon: LayoutDashboard },
    { name: t("sidebar.mySkills"), path: "/my-skills", icon: Layers },
    { name: t("sidebar.installSkills"), path: "/install", icon: Download },
    { name: t("sidebar.backup"), path: "/backup", icon: CloudUpload },
  ] as const;

  const handleSwitchPreset = (id: string) => {
    setViewedPresetId(id);

    if (location.pathname !== "/my-skills") {
      navigate({ to: "/my-skills" });
    }
  };

  const handleCreatePreset = async (name: string, description?: string, icon?: string) => {
    await api.createPreset(name, description, icon);
    await Promise.all([refreshPresets(), refreshManagedSkills()]);

    if (isSettingsPath(location.pathname)) {
      navigate({ to: "/my-skills" });
    }

    toast.success(t("preset.created"));
  };

  const handleRenamePreset = async (newName: string, icon?: string) => {
    if (!renameTarget) return;
    const preset = presets.find((s) => s.id === renameTarget.id);

    if (!preset) return;
    await api.updatePreset(
      renameTarget.id,
      newName,
      preset.description || undefined,
      icon || preset.icon || undefined,
    );
    await refreshPresets();
    toast.success(t("preset.renamed"));
  };

  const handleDeletePreset = async () => {
    if (!deleteTarget) return;
    await api.deletePreset(deleteTarget.id);
    await Promise.all([refreshPresets(), refreshManagedSkills()]);

    if (isSettingsPath(location.pathname)) {
      navigate({ to: "/my-skills" });
    }

    toast.success(t("preset.deleted"));
  };

  const handleRenameClick = (
    event: React.MouseEvent,
    preset: { id: string; name: string; icon?: null | string },
  ) => {
    event.preventDefault();
    event.stopPropagation();
    setRenameTarget(preset);
  };

  const handleDeleteClick = (event: React.MouseEvent, preset: { id: string; name: string }) => {
    event.preventDefault();
    event.stopPropagation();
    setDeleteTarget(preset);
  };

  const handleDeleteProject = async () => {
    if (!deleteProjectTarget) return;
    await api.removeProject(deleteProjectTarget.id);
    await refreshProjects();

    if (location.pathname.startsWith("/project/")) {
      navigate({ to: "/" });
    }

    toast.success(t("project.removed"));
  };

  // Renders one workspace category section (Global Workspace for coding agents,
  // Lobster Agents for lobster agents). Both sections share identical UX —
  // collapsible heading, "All Agents" overview entry, and a drag-orderable list.
  const renderToolGroup = (group: {
    category: ToolCategory;
    headingLabel: string;
    allAgentsLabel: string;
    emptyLabel: string;
    basePath: string;
    routePath: WorkspaceConfig["routePath"];
    tools: ToolInfo[];
    isOpen: boolean;
    onToggle: () => void;
    hideWhenEmpty: boolean;
  }) => {
    if (group.hideWhenEmpty && group.tools.length === 0) return null;

    return (
      <>
        <div className="mb-1.5 flex items-center gap-1 px-2.5">
          <button
            onClick={group.onToggle}
            className="flex min-w-0 flex-1 items-center gap-1 text-left outline-none"
          >
            {group.isOpen ? (
              <ChevronDown className="h-3 w-3 shrink-0 text-faint" />
            ) : (
              <ChevronRight className="h-3 w-3 shrink-0 text-faint" />
            )}
            <span className="truncate whitespace-nowrap text-[12px] font-semibold tracking-[0.01em] text-muted">
              {group.headingLabel}
            </span>
          </button>
        </div>
        {group.isOpen && (
          <>
            {/* Pinned overview item */}
            {(() => {
              const isActive = location.pathname === group.basePath;

              return (
                <Link
                  to={group.routePath}
                  className={cn(
                    "mb-0.5 flex items-center gap-2 px-2.5 py-[7px] rounded-md text-sm transition-colors outline-none",
                    isActive
                      ? "bg-surface-active font-medium text-primary"
                      : "text-tertiary hover:text-secondary hover:bg-surface-hover",
                  )}
                >
                  <span
                    className={cn(
                      "flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded border",
                      isActive
                        ? "border-accent/30 bg-accent/10 text-accent"
                        : "border-border bg-surface text-muted",
                    )}
                  >
                    <Globe className="h-3 w-3" />
                  </span>
                  <span className="flex-1 truncate">{group.allAgentsLabel}</span>
                </Link>
              );
            })()}
            {group.tools.length === 0 ? (
              <p className="px-5 py-1.5 text-[12px] text-faint">{group.emptyLabel}</p>
            ) : (
              <DndContext
                sensors={dragSensors}
                collisionDetection={closestCenter}
                onDragEnd={handleToolDragEnd(group.category)}
              >
                <SortableContext
                  items={group.tools.map((t) => t.key)}
                  strategy={verticalListSortingStrategy}
                >
                  <div className="space-y-0.5">
                    {group.tools.map((tool) => {
                      const skillCount = globalSkillsByAgent[tool.key] ?? 0;
                      const isActive = location.pathname === `${group.basePath}/${tool.key}`;

                      return (
                        <SortableItem
                          key={tool.key}
                          id={tool.key}
                          className={cn(
                            "group relative flex items-center rounded-md transition-colors",
                            isActive ? "bg-surface-active" : "hover:bg-surface-hover",
                          )}
                        >
                          {(handleProps) => (
                            <>
                              <button
                                onClick={() =>
                                  navigate({ to: group.routePath, params: { agentKey: tool.key } })
                                }
                                className={cn(
                                  "flex min-w-0 flex-1 items-center gap-2 px-2.5 py-[7px] text-left text-sm leading-5 outline-none",
                                  isActive
                                    ? "font-medium text-primary"
                                    : "text-tertiary group-hover:text-secondary",
                                )}
                              >
                                <AgentIcon
                                  agentKey={tool.key}
                                  displayName={tool.display_name}
                                  className={cn(
                                    "h-[20px] w-[20px] rounded border transition-colors",
                                    isActive
                                      ? "border-accent/30 bg-accent/10"
                                      : "group-hover:border-border",
                                  )}
                                />
                                <span className="flex-1 truncate">{tool.display_name}</span>
                                <span className="ml-auto flex h-[18px] w-[32px] shrink-0 items-center justify-end group-hover:hidden">
                                  {skillCount > 0 && (
                                    <span
                                      className={cn(
                                        "min-w-[18px] rounded-full px-1.5 text-center text-[12px] font-medium leading-[18px] tabular-nums",
                                        isActive
                                          ? "bg-accent-bg text-accent-light"
                                          : "bg-surface-hover text-muted",
                                      )}
                                    >
                                      {skillCount}
                                    </span>
                                  )}
                                </span>
                              </button>
                              <div
                                className={cn(
                                  "absolute right-1 flex items-center rounded-md invisible opacity-0 transition-opacity group-hover:visible group-hover:opacity-100",
                                  isActive ? "bg-surface-active" : "bg-surface-hover",
                                )}
                              >
                                <div
                                  {...handleProps}
                                  data-testid="drag-handle"
                                  className="cursor-grab rounded p-1 text-faint active:cursor-grabbing"
                                >
                                  <GripVertical className="h-3 w-3" />
                                </div>
                              </div>
                            </>
                          )}
                        </SortableItem>
                      );
                    })}
                  </div>
                </SortableContext>
              </DndContext>
            )}
          </>
        )}
      </>
    );
  };

  return (
    <>
      <div className="relative z-10 flex h-full w-[220px] flex-shrink-0 select-none flex-col border-r border-border-subtle bg-bg-secondary">
        {/* Traffic-light safe zone */}
        <div className="h-[38px] shrink-0" />
        {/* App logo — sits below macOS window controls */}
        <div className="flex shrink-0 items-center gap-3 px-3 pb-2.5">
          <img src="/icons/32x32.png" alt="logo" className="h-[24px] w-[24px] shrink-0" />
          <span className="truncate text-[16px] font-semibold leading-[22px] tracking-tight text-secondary">
            {t("app.name")}
          </span>
        </div>
        <HostSwitcher />

        {/* Nav */}
        <div className="shrink-0 space-y-0.5 px-2.5">
          {NAV_ITEMS.map((item) => {
            const Icon = item.icon;
            const isActive = location.pathname === item.path;

            return (
              <Link
                key={item.path}
                to={item.path}
                className={cn(
                  "flex items-center gap-2.5 px-2.5 py-[7px] rounded-md text-sm font-medium transition-colors outline-none",
                  isActive
                    ? "bg-surface-active text-primary"
                    : "text-tertiary hover:text-secondary hover:bg-surface-hover",
                )}
              >
                <Icon className={cn("w-4 h-4 shrink-0", isActive ? "text-accent" : "text-muted")} />
                {item.name}
              </Link>
            );
          })}
        </div>

        {/* Divider */}
        <div className="mx-3 mb-2.5 mt-3.5 border-t border-border-subtle" />

        {/* Scrollable section */}
        <div className="scrollbar-hide min-h-0 flex-1 overflow-y-auto px-2.5">
          {/* ── Presets ── */}
          <div className="mb-1.5 flex items-center gap-1 px-2.5">
            <button
              onClick={() => setPresetsOpen((v) => !v)}
              className="flex min-w-0 flex-1 items-center gap-1 text-left outline-none"
            >
              {presetsOpen ? (
                <ChevronDown className="h-3 w-3 shrink-0 text-faint" />
              ) : (
                <ChevronRight className="h-3 w-3 shrink-0 text-faint" />
              )}
              <span className="truncate whitespace-nowrap text-[12px] font-semibold tracking-[0.01em] text-muted">
                {t("sidebar.presets")}
              </span>
            </button>
          </div>
          {presetsOpen && (
            <>
              <DndContext
                sensors={dragSensors}
                collisionDetection={closestCenter}
                onDragEnd={handleDragEnd}
              >
                <SortableContext
                  items={orderedPresets.map((s) => s.id)}
                  strategy={verticalListSortingStrategy}
                >
                  <div className="space-y-0.5">
                    {orderedPresets.map((preset) => {
                      const isActive = viewedPreset?.id === preset.id;
                      const presetIcon = getPresetIconOption(preset);
                      const PresetIcon = presetIcon.icon;

                      return (
                        <SortableItem
                          key={preset.id}
                          id={preset.id}
                          className={cn(
                            "group relative flex items-center rounded-md transition-colors",
                            isActive ? "bg-surface-active" : "hover:bg-surface-hover",
                          )}
                        >
                          {(handleProps) => (
                            <>
                              <button
                                onClick={() => handleSwitchPreset(preset.id)}
                                className={cn(
                                  "flex min-w-0 flex-1 items-center gap-2 px-2.5 py-[7px] text-left text-sm leading-5 outline-none",
                                  isActive
                                    ? "font-medium text-primary"
                                    : "text-tertiary group-hover:text-secondary",
                                )}
                              >
                                <span
                                  className={cn(
                                    "flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded border",
                                    isActive
                                      ? `${presetIcon.activeClass} ${presetIcon.colorClass}`
                                      : "border-border bg-surface text-muted group-hover:border-border group-hover:text-tertiary",
                                  )}
                                >
                                  <PresetIcon className="h-3 w-3" />
                                </span>
                                <span className="flex-1 truncate">{preset.name}</span>
                                <span className="ml-auto flex h-[18px] w-[32px] shrink-0 items-center justify-end group-hover:hidden">
                                  {preset.skill_count > 0 && (
                                    <span
                                      className={cn(
                                        "min-w-[18px] rounded-full px-1.5 text-center text-[12px] font-medium leading-[18px] tabular-nums",
                                        isActive
                                          ? "bg-accent-bg text-accent-light"
                                          : "bg-surface-hover text-muted",
                                      )}
                                    >
                                      {preset.skill_count}
                                    </span>
                                  )}
                                </span>
                              </button>
                              <div
                                className={cn(
                                  "absolute right-1 flex items-center rounded-md invisible opacity-0 transition-opacity group-hover:visible group-hover:opacity-100",
                                  isActive ? "bg-surface-active" : "bg-surface-hover",
                                )}
                              >
                                <div
                                  {...handleProps}
                                  data-testid="drag-handle"
                                  className="cursor-grab rounded p-1 text-faint active:cursor-grabbing"
                                >
                                  <GripVertical className="h-3 w-3" />
                                </div>
                                <button
                                  onClick={(event) => handleRenameClick(event, preset)}
                                  className="rounded p-1 text-faint transition hover:text-secondary"
                                  title={t("common.rename")}
                                >
                                  <Pencil className="h-3 w-3" />
                                </button>
                                <button
                                  onClick={(event) => handleDeleteClick(event, preset)}
                                  className="rounded p-1 text-faint transition hover:text-red-400"
                                  title={t("common.delete")}
                                >
                                  <Trash2 className="h-3 w-3" />
                                </button>
                              </div>
                            </>
                          )}
                        </SortableItem>
                      );
                    })}
                  </div>
                </SortableContext>
              </DndContext>
              <button
                onClick={() => setShowCreate(true)}
                className="mt-1 flex w-full items-center gap-2 rounded-md px-2.5 py-[7px] text-sm text-muted outline-none transition-colors hover:bg-surface-hover hover:text-secondary"
              >
                <Plus className="h-3.5 w-3.5" />
                {t("sidebar.newPreset")}
              </button>
            </>
          )}

          {/* Divider */}
          <div className="mx-0.5 mb-2.5 mt-3.5 border-t border-border-subtle" />

          {renderToolGroup({
            category: "coding",
            headingLabel: t("sidebar.globalWorkspace"),
            allAgentsLabel: t("globalWorkspace.allAgents"),
            emptyLabel: t("globalWorkspace.noAgents"),
            basePath: CODING_WORKSPACE_CONFIG.basePath,
            routePath: CODING_WORKSPACE_CONFIG.routePath,
            tools: orderedCodingTools,
            isOpen: globalWorkspaceOpen,
            onToggle: () => setGlobalWorkspaceOpen((v) => !v),
            // Always show the Global Workspace section (even when empty) so users
            // with no detected coding agents still see the "All Agents" entry.
            hideWhenEmpty: false,
          })}

          {installedLobsterTools.length > 0 && (
            <>
              {/* Divider */}
              <div className="mx-0.5 mb-2.5 mt-3.5 border-t border-border-subtle" />

              {renderToolGroup({
                category: "lobster",
                headingLabel: t("sidebar.lobsterAgents"),
                allAgentsLabel: t("lobsterWorkspace.allAgents"),
                emptyLabel: t("lobsterWorkspace.noAgents"),
                basePath: LOBSTER_WORKSPACE_CONFIG.basePath,
                routePath: LOBSTER_WORKSPACE_CONFIG.routePath,
                tools: orderedLobsterTools,
                isOpen: lobsterWorkspaceOpen,
                onToggle: () => setLobsterWorkspaceOpen((v) => !v),
                hideWhenEmpty: true,
              })}
            </>
          )}

          {/* Divider */}
          <div className="mx-0.5 mb-2.5 mt-3.5 border-t border-border-subtle" />

          {/* ── Projects ── */}
          <div className="mb-1.5 flex items-center gap-1 px-2.5">
            <button
              onClick={() => setProjectsOpen((v) => !v)}
              className="flex min-w-0 flex-1 items-center gap-1 text-left outline-none"
            >
              {projectsOpen ? (
                <ChevronDown className="h-3 w-3 shrink-0 text-faint" />
              ) : (
                <ChevronRight className="h-3 w-3 shrink-0 text-faint" />
              )}
              <span className="truncate whitespace-nowrap text-[12px] font-semibold tracking-[0.01em] text-muted">
                {t("sidebar.projects")}
              </span>
            </button>
          </div>
          {projectsOpen && (
            <>
              <DndContext
                sensors={dragSensors}
                collisionDetection={closestCenter}
                onDragEnd={handleProjectDragEnd}
              >
                <SortableContext
                  items={orderedProjects.map((p) => p.id)}
                  strategy={verticalListSortingStrategy}
                >
                  <div className="space-y-0.5">
                    {orderedProjects.map((project) => {
                      const isActive = location.pathname === `/project/${project.id}`;

                      const healthIndicator = getSyncHealthIndicator(
                        project.sync_health,
                        project.skill_count,
                      );

                      return (
                        <SortableItem
                          key={project.id}
                          id={project.id}
                          className={cn(
                            "group relative flex items-center rounded-md transition-colors",
                            isActive ? "bg-surface-active" : "hover:bg-surface-hover",
                          )}
                        >
                          {(handleProps) => (
                            <>
                              <button
                                onClick={() =>
                                  navigate({ to: "/project/$id", params: { id: project.id } })
                                }
                                className={cn(
                                  "flex min-w-0 flex-1 items-center gap-2 px-2.5 py-[7px] text-left text-sm leading-5 outline-none",
                                  isActive
                                    ? "font-medium text-primary"
                                    : "text-tertiary group-hover:text-secondary",
                                )}
                              >
                                <span
                                  className={cn(
                                    "flex h-[20px] w-[20px] shrink-0 items-center justify-center rounded border",
                                    isActive
                                      ? project.workspace_type === "linked"
                                        ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-500"
                                        : "border-blue-500/30 bg-blue-500/10 text-blue-500"
                                      : "border-border bg-surface text-muted group-hover:border-border group-hover:text-tertiary",
                                  )}
                                >
                                  {project.workspace_type === "linked" ? (
                                    <Link2 className="h-3 w-3" />
                                  ) : (
                                    <FolderOpen className="h-3 w-3" />
                                  )}
                                </span>
                                <span className="flex-1 truncate">{project.name}</span>
                                <span className="ml-auto flex h-[18px] w-[52px] shrink-0 items-center justify-end gap-2 group-hover:hidden">
                                  {healthIndicator && (
                                    <span
                                      className={cn(
                                        "h-1.5 w-1.5 shrink-0 rounded-full",
                                        healthIndicator.color,
                                      )}
                                      title={healthIndicator.title}
                                    />
                                  )}
                                  {project.skill_count > 0 && (
                                    <span
                                      className={cn(
                                        "min-w-[24px] rounded-full px-1.5 text-center text-[12px] font-medium leading-[18px] tabular-nums",
                                        isActive
                                          ? "bg-accent-bg text-accent-light"
                                          : "bg-surface-hover text-muted",
                                      )}
                                    >
                                      {project.skill_count}
                                    </span>
                                  )}
                                </span>
                              </button>
                              <div
                                className={cn(
                                  "absolute right-1 flex items-center rounded-md invisible opacity-0 transition-opacity group-hover:visible group-hover:opacity-100",
                                  isActive ? "bg-surface-active" : "bg-surface-hover",
                                )}
                              >
                                <div
                                  {...handleProps}
                                  data-testid="drag-handle"
                                  className="cursor-grab rounded p-1 text-faint active:cursor-grabbing"
                                >
                                  <GripVertical className="h-3 w-3" />
                                </div>
                                <button
                                  onClick={(e) => {
                                    e.preventDefault();
                                    e.stopPropagation();
                                    setDeleteProjectTarget(project);
                                  }}
                                  className="rounded p-1 text-faint transition hover:text-red-400"
                                  title={t("common.delete")}
                                >
                                  <Trash2 className="h-3 w-3" />
                                </button>
                              </div>
                            </>
                          )}
                        </SortableItem>
                      );
                    })}
                  </div>
                </SortableContext>
              </DndContext>
              <button
                onClick={() => setShowAddProject(true)}
                className="mt-1 flex w-full items-center gap-2 rounded-md px-2.5 py-[7px] text-sm text-muted outline-none transition-colors hover:bg-surface-hover hover:text-secondary"
              >
                <Plus className="h-3.5 w-3.5" />
                {t("sidebar.addProject")}
              </button>
            </>
          )}
        </div>

        {/* Settings */}
        <div className="shrink-0 border-t border-border-subtle p-2.5">
          <Link
            to="/settings/{-$category}"
            className={cn(
              "flex items-center gap-2.5 px-2.5 py-[7px] rounded-md text-sm font-medium transition-colors outline-none",
              isSettingsPath(location.pathname)
                ? "bg-surface-active text-primary"
                : "text-tertiary hover:text-secondary hover:bg-surface-hover",
            )}
          >
            <Settings
              className={cn(
                "w-4 h-4 shrink-0",
                isSettingsPath(location.pathname) ? "text-accent" : "text-muted",
              )}
            />
            {t("sidebar.settings")}
            {/* A newer app version exists. Amber = "有更新" per the UI spec;
                the dot only points at Settings, where the user decides. */}
            {appUpdate?.has_update && (
              <span
                className="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400"
                title={t("settings.updateAvailable", { version: appUpdate.latest_version })}
              />
            )}
          </Link>
        </div>
      </div>

      <CreatePresetDialog
        open={showCreate}
        onClose={() => setShowCreate(false)}
        onCreate={handleCreatePreset}
      />

      <RenamePresetDialog
        open={renameTarget !== null}
        currentName={renameTarget?.name || ""}
        currentIcon={renameTarget?.icon}
        onClose={() => setRenameTarget(null)}
        onRename={handleRenamePreset}
      />

      <ConfirmDialog
        open={deleteTarget !== null}
        message={t("preset.deleteConfirm", { name: deleteTarget?.name || "" })}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDeletePreset}
      />

      <AddProjectDialog
        open={showAddProject}
        onClose={() => setShowAddProject(false)}
        onAdded={async () => {
          await refreshProjects();
          toast.success(t("project.workspaceAdded"));
        }}
      />

      <ConfirmDialog
        open={deleteProjectTarget !== null}
        message={t("project.removeConfirm", { name: deleteProjectTarget?.name || "" })}
        onClose={() => setDeleteProjectTarget(null)}
        onConfirm={handleDeleteProject}
      />
    </>
  );
}
