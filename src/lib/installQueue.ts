import type { QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import i18n from "../i18n";
import { managedSkillsQueryOptions, presetsQueryOptions, refreshQuery } from "./appQueries";
import { getErrorKind, getErrorMessage } from "./error";
import { invokeHost } from "./hostCall";
import type { SkillsShSkill } from "./tauri";

export type InstallStatus = "cancelled" | "done" | "failed" | "queued" | "running";

export interface InstallQueueItem {
  /** `source/skill_id`, also the backend's cancel key. */
  key: string;
  skill: SkillsShSkill;
  hostId: null | string;
  status: InstallStatus;
  error?: string;
}

const TOAST_ID = "install-queue";

export const installKey = (skill: SkillsShSkill) => `${skill.source}/${skill.skill_id}`;

const skillName = (skill: SkillsShSkill) => skill.name || skill.skill_id;

const isActive = (item: InstallQueueItem) => item.status === "queued" || item.status === "running";

/**
 * skills.sh installs, run one at a time on the host each was queued for. The
 * queue lives above the router, so installs keep going while the user browses
 * other pages. Finished items stay until the queue drains, so the progress
 * toast can count "2/5" and the summary can offer a retry of the failures.
 */
export function createInstallQueue(queryClient: QueryClient) {
  const listeners = new Set<() => void>();
  let items: InstallQueueItem[] = [];

  const set = (next: InstallQueueItem[]) => {
    items = next;

    for (const listener of listeners) listener();

    const current = items.find((item) => item.status === "running");

    if (current) {
      const finished = items.filter((item) => !isActive(item)).length;

      toast.loading(
        i18n.t("install.queue.progress", {
          current: finished + 1,
          total: items.length,
          name: skillName(current.skill),
        }),
        // Clear what a previous summary left on the shared toast.
        {
          id: TOAST_ID,
          description: undefined,
          action: undefined,
          closeButton: false,
          duration: undefined,
        },
      );
    }
  };

  const patch = (key: string, change: Partial<InstallQueueItem>) =>
    set(items.map((item) => (item.key === key ? { ...item, ...change } : item)));

  const refreshLibrary = async (hostId: null | string) => {
    try {
      await Promise.all([
        refreshQuery(queryClient, presetsQueryOptions(hostId)),
        refreshQuery(queryClient, managedSkillsQueryOptions(hostId)),
      ]);
    } catch (error) {
      console.warn("Post-install refresh failed:", error);
      toast.error(i18n.t("install.market.refreshError"));
    }
  };

  const finish = () => {
    const total = items.length;
    const installed = items.filter((item) => item.status === "done");
    const failed = items.filter((item) => item.status === "failed");
    set([]);

    if (failed.length > 0) {
      toast.error(i18n.t("install.market.batchSummary", { installed: installed.length, total }), {
        id: TOAST_ID,
        description: failed
          .map((item) =>
            i18n.t("install.market.batchFailureDetail", {
              name: skillName(item.skill),
              error: item.error,
            }),
          )
          .join("\n"),
        action: {
          label: i18n.t("install.queue.retry", { count: failed.length }),
          onClick: () => {
            for (const item of failed) enqueue([item.skill], item.hostId);
          },
        },
        duration: Infinity,
        closeButton: true,
      });
    } else if (installed.length === total) {
      toast.success(
        i18n.t("install.queue.installed", { count: total, name: skillName(installed[0].skill) }),
        {
          id: TOAST_ID,
        },
      );
    } else if (installed.length === 0) {
      toast.info(i18n.t("install.toast.cancelled"), { id: TOAST_ID });
    } else {
      toast.info(i18n.t("install.market.batchSummary", { installed: installed.length, total }), {
        id: TOAST_ID,
      });
    }
  };

  const pump = async () => {
    if (items.some((item) => item.status === "running")) return;
    const next = items.find((item) => item.status === "queued");

    if (!next) {
      if (items.length > 0) finish();

      return;
    }

    patch(next.key, { status: "running" });

    try {
      await invokeHost<void>(next.hostId, "install_from_skillssh", {
        source: next.skill.source,
        skillId: next.skill.skill_id,
      });
      await refreshLibrary(next.hostId);
      patch(next.key, { status: "done" });
    } catch (error) {
      patch(
        next.key,
        getErrorKind(error) === "cancelled"
          ? { status: "cancelled" }
          : { status: "failed", error: getErrorMessage(error, i18n.t("common.error")) },
      );
    }

    void pump();
  };

  const enqueue = (skills: SkillsShSkill[], hostId: null | string) => {
    let next = items;

    for (const skill of skills) {
      const key = installKey(skill);
      const existing = next.find((item) => item.key === key);

      if (existing && isActive(existing)) continue;
      const item: InstallQueueItem = { key, skill, hostId, status: "queued" };
      next = existing ? next.map((other) => (other === existing ? item : other)) : [...next, item];
    }

    if (next === items) return;
    set(next);
    void pump();
  };

  return {
    subscribe: (listener: () => void) => {
      listeners.add(listener);

      return () => listeners.delete(listener);
    },
    getItems: () => items,
    enqueue,
    /** Drops a queued item before it starts. */
    remove: (key: string) =>
      set(items.filter((item) => item.key !== key || item.status !== "queued")),
    cancel: (key: string) => {
      const item = items.find((other) => other.key === key && other.status === "running");

      if (!item) return;
      invokeHost<boolean>(item.hostId, "cancel_install", { key }).catch(() => {
        // Ignore race: the install may finish before the cancel arrives.
      });
    },
    /** A host switch drops what is still queued for other hosts; a running install finishes. */
    keepHost: (hostId: null | string) => {
      if (items.some((item) => item.status === "queued" && item.hostId !== hostId)) {
        set(items.filter((item) => item.status !== "queued" || item.hostId === hostId));
      }
    },
  };
}

export type InstallQueue = ReturnType<typeof createInstallQueue>;
