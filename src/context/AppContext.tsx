/* eslint-disable react-refresh/only-export-components */
import { createContext, useContext, useState, useEffect, useCallback, useRef, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import type { AppUpdateInfo, HostSessionInfo, ManagedSkill, Project, Preset, RemoteHost, ToolInfo } from "../lib/tauri";
import * as api from "../lib/tauri";
import { getActiveHostId, invokeHost, setActiveHostId } from "../lib/hostCall";
import {
  managedSkillsQueryOptions,
  presetsQueryOptions,
  projectsQueryOptions,
  refreshQuery,
  remoteHostsQueryOptions,
  toolsQueryOptions,
} from "../lib/appQueries";
import { listenOnActiveHost } from "../lib/hostEvents";
import { getErrorKind, getErrorMessage } from "../lib/error";
import i18n from "../i18n";
import { applyTextSize } from "../lib/textScale";
import { useNavigate } from "@tanstack/react-router";
import { settingsLink } from "../views/settings/categories";
import { toast } from "sonner";
import { RemoteCliUpdateDialog } from "../components/RemoteCliUpdateDialog";

/** The live link to the active remote host. */
export interface HostSession {
  info: HostSessionInfo;
  /** Set when the link dropped; the next call or Reconnect connects again. */
  lostMessage: string | null;
}

interface AppState {
  presets: Preset[];
  /** Backend-tracked "last applied to default targets". Drives the "Applied to..." status, not the sidebar selection. */
  activePreset: Preset | null;
  /** Frontend-only "currently being viewed/edited" preset. Persisted to localStorage. UI selection. */
  viewedPreset: Preset | null;
  tools: ToolInfo[];
  managedSkills: ManagedSkill[];
  projects: Project[];
  remoteHosts: RemoteHost[];
  /** The remote host the app operates on; null is this computer. Never
   *  remembered across launches: the app always starts on this computer. */
  activeHost: RemoteHost | null;
  activeHostId: string | null;
  hostSession: HostSession | null;
  /** The host a switch is connecting to, while it connects. */
  connectingHostId: string | null;
  updatingRemoteCliHostId: string | null;
  openRemoteCliUpdate: (hostId: string) => void;
  getHostSwitchToken: () => number;
  /** Operate on `hostId`, or on this computer for null. Resolves false when
   *  the host could not be reached; the app is then on this computer, or
   *  still on the host with its link marked lost when reconnecting. */
  switchHost: (hostId: string | null, allowDuringRemoteUpdate?: boolean) => Promise<boolean>;
  /** Start the active host's session again, e.g. so it picks up a new
   *  library path. Resolves like `switchHost`. */
  reconnectHost: () => Promise<boolean>;
  loading: boolean;
  appError: string | null;
  helpOpen: boolean;
  detailSkillId: string | null;
  /** Result of the last app-version check. Notification only: installing an
   *  update is always started by the user from Settings. */
  appUpdate: AppUpdateInfo | null;
  refreshAppUpdate: () => Promise<AppUpdateInfo>;
  refreshAppData: () => Promise<void>;
  refreshPresets: () => Promise<void>;
  refreshTools: () => Promise<void>;
  refreshManagedSkills: () => Promise<void>;
  refreshProjects: () => Promise<void>;
  refreshRemoteHosts: () => Promise<void>;
  setViewedPresetId: (id: string) => void;
  applyPresetToDefault: (id: string) => Promise<void>;
  openHelp: () => void;
  closeHelp: () => void;
  openSkillDetailById: (skillId: string) => void;
  closeSkillDetail: () => void;
}

const VIEWED_PRESET_LS_KEY = "skills-manager.viewedPresetId";
const LEGACY_VIEWED_PRESET_LS_KEY = "skills-manager.viewedScenarioId";
const EMPTY_PRESETS: Preset[] = [];
const EMPTY_TOOLS: ToolInfo[] = [];
const EMPTY_SKILLS: ManagedSkill[] = [];
const EMPTY_PROJECTS: Project[] = [];
const EMPTY_REMOTE_HOSTS: RemoteHost[] = [];

/** Preset ids belong to one machine's library, so each host keeps its own. */
function readViewedPresetId(hostId: string | null): string | null {
  try {
    if (hostId) return localStorage.getItem(`${VIEWED_PRESET_LS_KEY}:${hostId}`);
    return localStorage.getItem(VIEWED_PRESET_LS_KEY) || localStorage.getItem(LEGACY_VIEWED_PRESET_LS_KEY);
  } catch {
    return null;
  }
}

function storeViewedPresetId(id: string) {
  const hostId = getActiveHostId();
  try {
    localStorage.setItem(hostId ? `${VIEWED_PRESET_LS_KEY}:${hostId}` : VIEWED_PRESET_LS_KEY, id);
  } catch {
    // localStorage may be unavailable; selection is still tracked in memory.
  }
}

const AppContext = createContext<AppState | null>(null);

export function AppProvider({ children }: { children: ReactNode }) {
  const SKILL_UPDATE_TOAST_ID = "skill-update-available";
  const APP_UPDATE_TOAST_ID = "app-update-available";
  const navigate = useNavigate();
  const [viewedPresetId, setViewedPresetIdState] = useState<string | null>(() => readViewedPresetId(null));
  const [activeHostId, setActiveHostIdState] = useState<string | null>(null);
  const [hostSession, setHostSession] = useState<HostSession | null>(null);
  const [connectingHostId, setConnectingHostId] = useState<string | null>(null);
  const [remoteCliUpdateHostId, setRemoteCliUpdateHostId] = useState<string | null>(null);
  const [updatingRemoteCliHostId, setUpdatingRemoteCliHostId] = useState<string | null>(null);
  const [helpOpen, setHelpOpen] = useState(false);
  const [detailSkillId, setDetailSkillId] = useState<string | null>(null);
  const [appUpdate, setAppUpdate] = useState<AppUpdateInfo | null>(null);
  const autoCheckInFlightRef = useRef(false);
  const appUpdateCheckedRef = useRef(false);
  const lastUpdateNotificationRef = useRef<string | null>(null);
  const lastActivePresetIdRef = useRef<string | null>(null);
  const switchSeqRef = useRef(0);
  const queryClient = useQueryClient();
  const presetsQuery = useQuery(presetsQueryOptions(activeHostId));
  const toolsQuery = useQuery(toolsQueryOptions(activeHostId));
  const managedSkillsQuery = useQuery(managedSkillsQueryOptions(activeHostId));
  const projectsQuery = useQuery(projectsQueryOptions(activeHostId));
  const remoteHostsQuery = useQuery(remoteHostsQueryOptions());
  const presets = presetsQuery.data?.presets ?? EMPTY_PRESETS;
  const activePreset = presetsQuery.data?.activePreset ?? null;
  const tools = toolsQuery.data ?? EMPTY_TOOLS;
  const managedSkills = managedSkillsQuery.data ?? EMPTY_SKILLS;
  const projects = projectsQuery.data ?? EMPTY_PROJECTS;
  const remoteHosts = remoteHostsQuery.data ?? EMPTY_REMOTE_HOSTS;
  const loading = [presetsQuery, toolsQuery, managedSkillsQuery, projectsQuery, remoteHostsQuery]
    .some((query) => query.isPending);
  const appError =
    (presetsQuery.error && i18n.t("common.loadFailed", { item: i18n.t("common.presets") })) ||
    (toolsQuery.error && i18n.t("common.loadFailed", { item: i18n.t("common.agents") })) ||
    (managedSkillsQuery.error && i18n.t("common.loadFailed", { item: i18n.t("common.skills") })) ||
    (projectsQuery.error && i18n.t("common.loadFailed", { item: i18n.t("sidebar.projects") })) ||
    (remoteHostsQuery.error && i18n.t("common.loadFailed", { item: i18n.t("hostSwitcher.label") })) ||
    null;

  const refreshPresetsForHost = useCallback(async (hostId: string | null) => {
    try {
      await refreshQuery(queryClient, presetsQueryOptions(hostId));
    } catch (error) {
      console.error("Failed to load presets:", error);
    }
  }, [queryClient]);

  const refreshPresets = useCallback(() => refreshPresetsForHost(getActiveHostId()), [refreshPresetsForHost]);

  const refreshToolsForHost = useCallback(async (hostId: string | null) => {
    try {
      await refreshQuery(queryClient, toolsQueryOptions(hostId));
    } catch (error) {
      console.error("Failed to load tools:", error);
    }
  }, [queryClient]);

  const refreshTools = useCallback(() => refreshToolsForHost(getActiveHostId()), [refreshToolsForHost]);

  const refreshProjectsForHost = useCallback(async (hostId: string | null) => {
    try {
      await refreshQuery(queryClient, projectsQueryOptions(hostId));
    } catch (error) {
      console.error("Failed to load projects:", error);
    }
  }, [queryClient]);

  const refreshProjects = useCallback(() => refreshProjectsForHost(getActiveHostId()), [refreshProjectsForHost]);

  const refreshRemoteHosts = useCallback(async () => {
    try {
      await refreshQuery(queryClient, remoteHostsQueryOptions());
    } catch (error) {
      console.error("Failed to load remote hosts:", error);
    }
  }, [queryClient]);

  const refreshManagedSkillsForHost = useCallback(async (hostId: string | null) => {
    await Promise.all([
      (async () => {
        try {
          await refreshQuery(queryClient, managedSkillsQueryOptions(hostId));
        } catch (error) {
          console.error("Failed to load managed skills:", error);
        }
      })(),
      // Managed skill changes affect project sync health badges.
      refreshProjectsForHost(hostId),
    ]);
  }, [queryClient, refreshProjectsForHost]);

  const refreshManagedSkills = useCallback(
    () => refreshManagedSkillsForHost(getActiveHostId()),
    [refreshManagedSkillsForHost]
  );

  const refreshAppDataForHost = useCallback(async (hostId: string | null) => {
    await Promise.all([
      refreshPresetsForHost(hostId),
      refreshToolsForHost(hostId),
      // refreshAppData owns each resource once; this call must not nest a
      // second project refresh through refreshManagedSkills.
      (async () => {
        try {
          await refreshQuery(queryClient, managedSkillsQueryOptions(hostId));
        } catch (error) {
          console.error("Failed to load managed skills:", error);
        }
      })(),
      refreshProjectsForHost(hostId),
      refreshRemoteHosts(),
    ]);
  }, [queryClient, refreshPresetsForHost, refreshProjectsForHost, refreshRemoteHosts, refreshToolsForHost]);

  const refreshAppData = useCallback(
    () => refreshAppDataForHost(getActiveHostId()),
    [refreshAppDataForHost]
  );

  const setViewedPresetId = useCallback((id: string) => {
    setViewedPresetIdState(id);
    storeViewedPresetId(id);
  }, []);

  /** Point every host-scoped call at `hostId`; query keys keep each host's data separate. */
  const enterHost = useCallback((hostId: string | null) => {
    setActiveHostId(hostId);
    setActiveHostIdState(hostId);
    setViewedPresetIdState(readViewedPresetId(hostId));
    lastActivePresetIdRef.current = null;
    setDetailSkillId(null);
  }, []);

  const switchHost = useCallback(
    async (hostId: string | null, allowDuringRemoteUpdate = false): Promise<boolean> => {
      if (updatingRemoteCliHostId !== null && !allowDuringRemoteUpdate) return false;
      // Only the latest switch decides where the app ends up.
      const seq = ++switchSeqRef.current;
      const isLatest = () => switchSeqRef.current === seq;
      const previous = getActiveHostId();
      if (hostId === null) {
        setConnectingHostId(null);
        // Also abandons a connect in flight. This computer's data does not
        // wait for the host's session to close.
        api.remoteHostDisconnect().catch((e) => console.error("Failed to disconnect:", e));
        if (previous === null) return true;
        enterHost(null);
        setHostSession(null);
        await refreshAppDataForHost(null);
        return true;
      }
      setConnectingHostId(hostId);
      try {
        const info = await api.remoteHostConnect(hostId);
        if (!isLatest()) return false;
        if (previous !== hostId) enterHost(hostId);
        setHostSession({ info, lostMessage: null });
        await refreshAppDataForHost(hostId);
        return true;
      } catch (e) {
        if (!isLatest()) return false;
        const message = getErrorMessage(e, i18n.t("common.error"));
        const name = remoteHosts.find((h) => h.id === hostId)?.name ?? hostId;
        if (previous === hostId) {
          // Reconnecting: stay on the host with its link marked lost.
          setHostSession((s) => s && { ...s, lostMessage: message });
        } else if (previous !== null) {
          // Connecting closed the previous host's session.
          enterHost(null);
          setHostSession(null);
          await refreshAppDataForHost(null);
        }
        const versionMismatch = getErrorKind(e) === "remote_version_mismatch";
        toast.error(i18n.t("hostSwitcher.connectFailed", { name }), {
          description: message,
          duration: 10000,
          action: versionMismatch
            ? { label: i18n.t("remoteHosts.reviewUpdate"), onClick: () => setRemoteCliUpdateHostId(hostId) }
            : { label: i18n.t("common.retry"), onClick: () => void switchHost(hostId) },
        });
        return false;
      } finally {
        if (isLatest()) setConnectingHostId(null);
      }
    },
    [enterHost, refreshAppDataForHost, remoteHosts, updatingRemoteCliHostId]
  );

  const reconnectHost = useCallback(async () => {
    const hostId = getActiveHostId();
    if (hostId === null) return false;
    await api.remoteHostDisconnect().catch((e) => console.error("Failed to disconnect:", e));
    return switchHost(hostId);
  }, [switchHost]);

  // The app always starts on this computer (also after a webview reload,
  // which leaves the previous page's session open).
  useEffect(() => {
    api.remoteHostDisconnect().catch((e) => console.error("Failed to disconnect:", e));
  }, []);

  const handleApplyPresetToDefault = useCallback(
    async (id: string) => {
      await api.applyPresetToDefault(id);
      await Promise.all([refreshPresets(), refreshManagedSkills()]);
    },
    [refreshManagedSkills, refreshPresets]
  );

  // Resolve viewedPreset: persisted id > activePreset > first preset.
  // Persist whichever resolves so the next launch matches what the user saw.
  const viewedPreset = (() => {
    if (viewedPresetId) {
      const found = presets.find((s) => s.id === viewedPresetId);
      if (found) return found;
    }
    return activePreset ?? presets[0] ?? null;
  })();

  useEffect(() => {
    const previousActiveId = lastActivePresetIdRef.current;
    const nextActiveId = activePreset?.id ?? null;
    if (previousActiveId === nextActiveId) return;
    lastActivePresetIdRef.current = nextActiveId;
    // Follow an external active-preset change only when the viewer was on the
    // former active preset. The initial query result preserves stored choice.
    if (nextActiveId && previousActiveId !== null) {
      setViewedPresetIdState((current) => {
        if (current !== previousActiveId) return current;
        storeViewedPresetId(nextActiveId);
        return nextActiveId;
      });
    }
  }, [activeHostId, activePreset?.id]);

  useEffect(() => {
    if (!viewedPreset) return;
    if (viewedPreset.id !== viewedPresetId) {
      // Persist the resolved fallback so subsequent reads are stable.
      setViewedPresetIdState(viewedPreset.id);
      storeViewedPresetId(viewedPreset.id);
    }
  }, [viewedPreset, viewedPresetId]);

  useEffect(() => {
    async function init() {
      // Both events log performance.now() (ms since timeOrigin) so the
      // reader can compute duration as done - start. Keeping the unit
      // identical to the other frontend startup marks avoids ambiguity in
      // the log file (see codex review note on #153).
      api.logStartupEvent("refresh_app_data_start", performance.now()).catch(() => {});
      await Promise.allSettled([
        queryClient.fetchQuery(presetsQueryOptions(null)),
        queryClient.fetchQuery(toolsQueryOptions(null)),
        queryClient.fetchQuery(managedSkillsQueryOptions(null)),
        queryClient.fetchQuery(projectsQueryOptions(null)),
        queryClient.fetchQuery(remoteHostsQueryOptions()),
      ]);
      api.logStartupEvent("refresh_app_data_done", performance.now()).catch(() => {});
      // Apply saved text size on startup
      const savedSize = await api.getSettings("text_size").catch(() => null);
      if (savedSize) {
        applyTextSize(savedSize);
      }
    }
    init();
  }, [queryClient]);

  useEffect(() => {
    const unlistenPromise = listen("tray-open-updates", () => {
      // The tray checked this computer's skills.
      void switchHost(null);
      setDetailSkillId(null);
      navigate({ to: "/my-skills" });
    });

    return () => {
      unlistenPromise
        .then((unlisten) => unlisten())
        .catch((error) => {
          console.error("Failed to unlisten tray-open-updates:", error);
        });
    };
  }, [navigate, switchHost]);

  useEffect(() => {
    let refreshTimer: ReturnType<typeof setTimeout> | null = null;

    const unlistenPromise = listenOnActiveHost("app-files-changed", () => {
      const hostId = getActiveHostId();
      if (refreshTimer) {
        clearTimeout(refreshTimer);
      }
      refreshTimer = setTimeout(() => {
        refreshAppDataForHost(hostId).catch((error) => {
          console.error("Failed to refresh after filesystem change:", error);
        });
      }, 500);
    });

    return () => {
      if (refreshTimer) {
        clearTimeout(refreshTimer);
      }
      unlistenPromise
        .then((unlisten) => unlisten())
        .catch((error) => {
          console.error("Failed to unlisten app-files-changed:", error);
        });
    };
  }, [refreshAppDataForHost]);

  // The active host's link dropped (say so and offer Reconnect), or a call
  // connected it again.
  useEffect(() => {
    type SessionState = { host_id: string; state: string; message?: string };
    const unlistenPromise = listen<SessionState>("remote-session-state", ({ payload }) => {
      if (payload.host_id !== getActiveHostId()) return;
      const lostMessage = payload.state === "disconnected" ? payload.message ?? "" : null;
      setHostSession((s) => s && { ...s, lostMessage });
    });
    return () => {
      unlistenPromise
        .then((unlisten) => unlisten())
        .catch((error) => {
          console.error("Failed to unlisten remote-session-state:", error);
        });
    };
  }, []);

  const notifyUpdatableSkills = useCallback((skills: ManagedSkill[]) => {
    const updatable = skills
      .filter((s) => s.update_status === "update_available")
      .sort((a, b) => a.id.localeCompare(b.id));

    if (updatable.length === 0) {
      lastUpdateNotificationRef.current = null;
      toast.dismiss(SKILL_UPDATE_TOAST_ID);
      return;
    }

    const notificationSignature = updatable.map((skill) => skill.id).join("|");
    if (lastUpdateNotificationRef.current === notificationSignature) {
      return;
    }

    lastUpdateNotificationRef.current = notificationSignature;
    toast.info(
      i18n.t("mySkills.updateNotification", { count: updatable.length }),
      {
        id: SKILL_UPDATE_TOAST_ID,
        duration: 8000,
        action: {
          label: i18n.t("mySkills.viewUpdates"),
          onClick: () => {
            setDetailSkillId(null);
            navigate({ to: "/my-skills" });
          },
        },
      }
    );
  }, [navigate]);

  const refreshAppUpdate = useCallback(async () => {
    const info = await api.checkAppUpdate();
    setAppUpdate(info);
    return info;
  }, []);

  // Check for a newer app version on startup. This only ever *notifies* — the
  // download and install stay behind the button in Settings, so the user
  // decides whether to take an update. Deliberately unlike the skill
  // auto-update above, which has an opt-in "apply automatically" setting.
  //
  // Failures are logged, never toasted: this runs unprompted on every launch,
  // and users who cannot reach GitHub would otherwise get an error every time
  // they open the app.
  //
  // The ref makes it once per process, not once per initial-loading edge, so
  // file events and host changes do not re-hit the GitHub API.
  //
  // Set inside the timer, not before it: `loading` flipping back to true within
  // the delay (the file watcher emits a change event as it builds its initial
  // watch set) tears this effect down and clears the pending timer, and marking
  // it done up front would skip the check for the rest of the session.
  useEffect(() => {
    if (loading || appUpdateCheckedRef.current) return;
    const timer = setTimeout(() => {
      appUpdateCheckedRef.current = true;
      refreshAppUpdate()
        .then((info) => {
          if (!info.has_update) return;
          toast.info(
            i18n.t("settings.updateAvailable", { version: info.latest_version }),
            {
              id: APP_UPDATE_TOAST_ID,
              duration: 8000,
              action: {
                label: i18n.t("settings.viewUpdate"),
                onClick: () => navigate(settingsLink("about")),
              },
            }
          );
        })
        .catch((err) => {
          console.error("Startup app update check failed:", err);
        });
    }, 3000);
    return () => clearTimeout(timer);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loading]);

  // Check skill updates on startup (non-blocking, silent). When the user has
  // opted in via the Settings toggle, also apply any available updates.
  useEffect(() => {
    // The round is this computer's: a host runs its own from its own app.
    if (loading || activeHostId !== null || managedSkills.length === 0) return;
    const hasGitSkills = managedSkills.some(
      (s) => s.source_type === "git" || s.source_type === "skillssh"
    );
    if (!hasGitSkills || autoCheckInFlightRef.current) return;

    // Delay to avoid slowing down initial render
    const timer = setTimeout(() => {
      autoCheckInFlightRef.current = true;
      (async () => {
        try {
          await invokeHost(null, "check_all_skill_updates", { force: false });
          // This updater round belongs to this computer even if the app has
          // switched hosts while its local operation was running.
          if (getActiveHostId() !== null) return;
          const skills = await invokeHost<ManagedSkill[]>(null, "get_managed_skills");

          const autoUpdate = await invokeHost<string | null>(null, "get_settings", { key: "auto_update_apply" })
            .catch(() => null);
          if (autoUpdate === "on" && getActiveHostId() === null) {
            const ids = skills
              .filter(
                (s) =>
                  s.update_status === "update_available" &&
                  (s.source_type === "git" || s.source_type === "skillssh")
              )
              .map((s) => s.id);
            if (ids.length > 0) {
              const result = await invokeHost<Awaited<ReturnType<typeof api.batchUpdateSkills>>>(
                null,
                "batch_update_skills",
                { skillIds: ids }
              );
              if (result.refreshed > 0) {
                toast.success(
                  i18n.t("mySkills.autoUpdated", { count: result.refreshed })
                );
              }
              // Held back rather than applied: updating would have removed
              // files the new version does not have, and nobody was here to ask.
              if (result.held_back.length > 0) {
                toast.warning(
                  i18n.t("mySkills.batchHeldBack", {
                    count: result.held_back.length,
                    names: result.held_back.slice(0, 3).join("、"),
                  })
                );
              }
              if (result.failed.length > 0) {
                console.warn("Auto-update failures:", result.failed);
                toast.error(
                  i18n.t("mySkills.autoUpdateFailed", {
                    count: result.failed.length,
                  })
                );
              }
            }
          }

          if (getActiveHostId() !== null) return;
          const currentSkills = await refreshQuery(queryClient, managedSkillsQueryOptions(null));
          if (getActiveHostId() === null) notifyUpdatableSkills(currentSkills);
          invokeHost(null, "set_settings", {
            key: "auto_update_last_run_at",
            value: new Date().toISOString(),
          })
            .catch(() => {});
        } catch (err) {
          // Startup round is non-blocking and does not toast on failure, but
          // log so a broken check/update is still diagnosable.
          console.error("Startup skill update round failed:", err);
        } finally {
          autoCheckInFlightRef.current = false;
        }
      })();
    }, 3000);
    return () => clearTimeout(timer);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loading]);

  // Refresh after a background auto-update round (Rust scheduler) or the
  // tray "check for updates" action finishes.
  useEffect(() => {
    const unlistenPromise = listenOnActiveHost("skills-auto-updated", async () => {
      const hostId = getActiveHostId();
      try {
        const skills = await refreshQuery(queryClient, managedSkillsQueryOptions(hostId));
        if (getActiveHostId() === hostId) notifyUpdatableSkills(skills);
      } catch (error) {
        console.error("Failed to refresh after skills-auto-updated:", error);
      }
    });
    return () => {
      unlistenPromise
        .then((unlisten) => unlisten())
        .catch((error) => {
          console.error("Failed to unlisten skills-auto-updated:", error);
        });
    };
  }, [notifyUpdatableSkills, queryClient]);

  return (
    <AppContext.Provider
      value={{
        presets,
        activePreset,
        viewedPreset,
        tools,
        managedSkills,
        projects,
        remoteHosts,
        activeHost: remoteHosts.find((host) => host.id === activeHostId) ?? null,
        activeHostId,
        hostSession,
        connectingHostId,
        updatingRemoteCliHostId,
        openRemoteCliUpdate: (hostId: string) => setRemoteCliUpdateHostId(hostId),
        getHostSwitchToken: () => switchSeqRef.current,
        switchHost,
        reconnectHost,
        loading,
        appError,
        helpOpen,
        detailSkillId,
        appUpdate,
        refreshAppUpdate,
        refreshAppData,
        refreshPresets,
        refreshTools,
        refreshManagedSkills,
        refreshProjects,
        refreshRemoteHosts,
        setViewedPresetId,
        applyPresetToDefault: handleApplyPresetToDefault,
        openHelp: () => setHelpOpen(true),
        closeHelp: () => setHelpOpen(false),
        openSkillDetailById: (skillId: string) => setDetailSkillId(skillId),
        closeSkillDetail: () => setDetailSkillId(null),
      }}
    >
      {children}
      <RemoteCliUpdateDialog
        hostId={remoteCliUpdateHostId}
        onClose={() => setRemoteCliUpdateHostId(null)}
        onBusyChange={(hostId) => setUpdatingRemoteCliHostId(hostId)}
      />
    </AppContext.Provider>
  );
}

export function useApp() {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useApp must be used within AppProvider");
  return ctx;
}
