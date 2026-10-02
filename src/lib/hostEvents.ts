import { type EventCallback, listen } from "@tauri-apps/api/event";

import { isString } from "../utils";
import { getActiveHostId } from "./hostCall";

/**
 * Whether an event is about the machine the app is showing. Events a host
 * forwards carry its `host_id`; this app's own events carry none.
 */
export function isForActiveHost<T>(payload: T, activeHostId: null | string): boolean {
  const hostId = payload instanceof Object && "host_id" in payload ? payload.host_id : undefined;

  return isString(hostId) ? hostId === activeHostId : activeHostId === null;
}

/** `listen`, minus events from a machine the app isn't showing. */
export function listenOnActiveHost<T>(event: string, handler: EventCallback<T>) {
  return listen<T>(event, (e) => {
    if (isForActiveHost(e.payload, getActiveHostId())) handler(e);
  });
}
