/* eslint-disable react-refresh/only-export-components */
import { useQueryClient } from "@tanstack/react-query";
import { createContext, type ReactNode, useContext, useState, useSyncExternalStore } from "react";

import { createInstallQueue, type InstallQueue } from "../lib/installQueue";

const InstallQueueContext = createContext<InstallQueue | null>(null);

export function InstallQueueProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const [queue] = useState(() => createInstallQueue(queryClient));

  return <InstallQueueContext.Provider value={queue}>{children}</InstallQueueContext.Provider>;
}

/** The queue itself, for code that acts on it without showing its items. */
export function useInstallQueueControl() {
  const queue = useContext(InstallQueueContext);

  if (!queue) throw new Error("useInstallQueue must be used within InstallQueueProvider");

  return queue;
}

export function useInstallQueue() {
  const queue = useInstallQueueControl();
  const items = useSyncExternalStore(queue.subscribe, queue.getItems);

  return { items, enqueue: queue.enqueue, remove: queue.remove, cancel: queue.cancel };
}
