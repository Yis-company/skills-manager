import { useState } from "react";

import { useApp } from "../context/AppContext";
import { getErrorMessage } from "../lib/error";
import * as api from "../lib/tauri";

export const STEPS = ["connect", "install", "access", "preview", "join", "apply"] as const;

type StepKey = (typeof STEPS)[number];

export type StepStatus = "blocked" | "done" | "running" | "todo" | "waiting";

export interface AddHostRun {
  name: string;
  target: string;
  hostId: null | string;
  steps: Record<StepKey, { status: StepStatus; note?: string }>;
  paste: api.HostSyncPasteKey | null;
  preview: api.JoinPreview | null;
  done: boolean;
}

/**
 * Add a host and set it up as a live log (SID-106): connect, install the
 * matching CLI, give it access to Sync, show what joining changes, join, and
 * apply the active preset. A blocked step stops the log with its reason.
 */
export function useAddHost(onJoined: (hostId: string) => void) {
  const { refreshRemoteHosts } = useApp();
  const [run, setRun] = useState<AddHostRun | null>(null);

  const step = (key: StepKey, status: StepStatus, note?: string) =>
    setRun((r) => r && { ...r, steps: { ...r.steps, [key]: { status, note } } });

  /** Run one step; a failure blocks the log there with its reason. */
  const attempt = async <T>(key: StepKey, work: () => Promise<T>): Promise<T | undefined> => {
    step(key, "running");

    try {
      return await work();
    } catch (error) {
      step(key, "blocked", getErrorMessage(error, "Failed"));

      return undefined;
    }
  };

  const access = async (hostId: string, pasted: boolean) => {
    const prepared = await attempt("access", () => api.hostSyncPrepare(hostId, pasted));

    if (!prepared) return;

    if (prepared.paste) {
      setRun((r) => r && { ...r, paste: prepared.paste });
      step("access", "waiting", prepared.paste.reason);

      return;
    }

    setRun((r) => r && { ...r, paste: null });
    step("access", "done", prepared.url ?? undefined);
    const preview = await attempt("preview", () => api.hostSyncPreview(hostId));

    if (!preview) return;
    setRun((r) => r && { ...r, preview });
    step("preview", "waiting");
  };

  const start = async (name: string, target: string) => {
    setRun({
      name,
      target,
      hostId: null,
      steps: {
        connect: { status: "todo" },
        install: { status: "todo" },
        access: { status: "todo" },
        preview: { status: "todo" },
        join: { status: "todo" },
        apply: { status: "todo" },
      },
      paste: null,
      preview: null,
      done: false,
    });

    const host = await attempt("connect", async () => {
      const added = await api.remoteHostAdd(name, target);
      await refreshRemoteHosts();

      return added;
    });

    if (!host) return;
    setRun((r) => r && { ...r, hostId: host.id });
    // A host with no CLI or another version gets the matching one, no click
    // (SID-101); install says why it can't (a newer CLI, an older desktop app).
    const probe = await api.remoteHostProbe(host.id).catch(() => null);
    step("connect", "done", probe ? `${target} · CLI ${probe.version}` : target);

    if (probe?.compatible) {
      step("install", "done", probe.version);
    } else if (!(await attempt("install", () => api.remoteHostInstallCli(host.id)))) {
      return;
    } else {
      step("install", "done");
    }

    await access(host.id, false);
  };

  const pasted = () => run?.hostId && access(run.hostId, true);

  const join = async () => {
    const hostId = run?.hostId;

    if (!hostId) return;
    step("preview", "done");
    const joined = await attempt("join", () => api.hostSyncJoin(hostId));

    if (!joined) return;
    step("join", "done", joined.timer.warning ?? undefined);
    step("apply", "done");
    setRun((r) => r && { ...r, done: true });
    onJoined(hostId);
  };

  return { run, start, pasted, join, reset: () => setRun(null) };
}
