import { getCurrentWindow } from "@tauri-apps/api/window";
import { type MouseEventHandler, useCallback } from "react";

/**
 * Returns a mousedown handler that initiates window dragging via Tauri API.
 */
export function useDragWindow(): MouseEventHandler {
  return useCallback((e) => {
    if (e.buttons === 1 && e.detail === 1) {
      getCurrentWindow().startDragging();
    }
  }, []);
}
