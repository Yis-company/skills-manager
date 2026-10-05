/* eslint-disable react-refresh/only-export-components */
import { useQueryClient } from "@tanstack/react-query";
import {
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useState,
  useSyncExternalStore,
} from "react";

import { createInstallQueue, type InstallQueue } from "../lib/installQueue";
import { useApp } from "./AppContext";

const InstallQueueContext = createContext<InstallQueue | null>(null);

export function InstallQueueProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const { activeHostId } = useApp();
  const [queue] = useState(() => createInstallQueue(queryClient));

  useEffect(() => {
    queue.keepHost(activeHostId);
  }, [queue, activeHostId]);

  return <InstallQueueContext.Provider value={queue}>{children}</InstallQueueContext.Provider>;
}

export function useInstallQueue() {
  const queue = useContext(InstallQueueContext);

  if (!queue) throw new Error("useInstallQueue must be used within InstallQueueProvider");
  const items = useSyncExternalStore(queue.subscribe, queue.getItems);

  return { items, enqueue: queue.enqueue, remove: queue.remove, cancel: queue.cancel };
}
