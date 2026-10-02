import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, vi } from "vitest";

/**
 * Answers Tauri commands in the calling test file through Tauri's own IPC
 * mock, so the real `invoke` and plugin wrappers run. Returns the handler to
 * stub answers on and assert calls against.
 */
export function mockTauriIpc() {
  const handler = vi.fn<Parameters<typeof mockIPC>[0]>();

  beforeEach(() => {
    // Unit tests run in Node, which has no `window` for Tauri's internals.
    vi.stubGlobal("window", {});
    mockIPC(handler);
  });

  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
    handler.mockReset();
  });

  return handler;
}
