import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";

import { useApp } from "../context/AppContext";
import * as api from "../lib/tauri";

/** A host in the Sync screen's list: this computer (`id` null) or an SSH host. */
export interface SyncHost {
  id: null | string;
  name: string;
  /** SSH target; null for this computer. */
  target: null | string;
  state: api.SyncStateKey;
  reason: null | string;
  status: api.SyncStatus | null;
  /** In Sync: this computer once it has a remote; a host once it joined. */
  inSync: boolean;
  keyTitle: null | string;
  timer: boolean;
}

/**
 * Everything the Sync screen shows: this computer's `sync status`, one status
 * call per host in Sync, and the shared history. Refreshes after every
 * automatic round.
 */
export function useSyncScreen() {
  const { remoteHosts } = useApp();
  const [local, setLocal] = useState<api.SyncStatus | null>(null);
  const [hostStates, setHostStates] = useState<api.HostSyncState[]>([]);
  const [history, setHistory] = useState<api.SyncHistoryEntry[]>([]);

  const loadHosts = useCallback(() => {
    // Each host is an SSH round trip; the rest of the screen doesn't wait.
    api
      .hostSyncStatusAll()
      .then(setHostStates)
      .catch(() => setHostStates([]));
  }, []);

  const refresh = useCallback(async () => {
    const [status, entries] = await loadScreen();

    setLocal(status);
    setHistory(entries);
    loadHosts();
  }, [loadHosts]);

  useEffect(() => {
    loadScreen()
      .then(([status, entries]) => {
        setLocal(status);
        setHistory(entries);
      })
      .catch(() => {});
    loadHosts();
    const unlisten = listen("backup-auto-completed", () => void refresh());

    return () => {
      void unlisten.then((stop) => stop()).catch(() => {});
    };
  }, [loadHosts, refresh]);

  const thisHost: SyncHost = {
    id: null,
    name: local?.host ?? "",
    target: null,
    state: local?.state ?? "not_set_up",
    reason: local?.reason ?? null,
    status: local,
    inSync: !!local && local.state !== "not_set_up",
    keyTitle: null,
    timer: false,
  };

  const hosts: SyncHost[] = remoteHosts.map((host) => {
    const known = hostStates.find((state) => state.host_id === host.id);
    const status = known?.status ?? null;

    return {
      id: host.id,
      name: host.name,
      target: host.ssh_target,
      state: known ? (known.error ? "offline" : (status?.state ?? "offline")) : "not_set_up",
      reason: known?.error ?? status?.reason ?? null,
      status,
      inSync: !!known,
      keyTitle: known?.key_title ?? null,
      timer: known?.timer ?? false,
    };
  });

  return { local, hosts: [thisHost, ...hosts], history, refresh };
}

/** This computer's `sync status` and the shared history. */
function loadScreen() {
  return Promise.all([
    api.syncCall<api.SyncStatus>(null, "sync_status").catch(() => null),
    api.syncHistory(50).catch(() => []),
  ]);
}
