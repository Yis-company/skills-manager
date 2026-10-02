import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setActiveHostId } from "./hostCall";
import { pickPath, type RemotePicker, setRemotePicker } from "./pickPath";
import { mockTauriIpc } from "./tauriIpcTesting";

const ipc = mockTauriIpc();

afterEach(() => {
  setActiveHostId(null);
  setRemotePicker(null);
});

describe("pickPath on this computer", () => {
  it("opens the native dialog for a folder or a filtered file", async () => {
    ipc.mockResolvedValue("/Users/me/skills");
    await expect(pickPath({ directory: true }, { startPath: "/ignored" })).resolves.toBe(
      "/Users/me/skills",
    );
    await pickPath({ files: ["zip", "skill"], filterName: "Skills" });
    expect(ipc).toHaveBeenNthCalledWith(1, "plugin:dialog|open", {
      options: { directory: true, multiple: false },
    });
    expect(ipc).toHaveBeenNthCalledWith(2, "plugin:dialog|open", {
      options: { multiple: false, filters: [{ name: "Skills", extensions: ["zip", "skill"] }] },
    });
  });

  it("resolves null when the dialog is cancelled", async () => {
    ipc.mockResolvedValue(null);
    await expect(pickPath({ directory: true })).resolves.toBeNull();
  });
});

describe("pickPath with a remote host active", () => {
  beforeEach(() => setActiveHostId("host-1"));

  it("asks the remote browser and never the native dialog", async () => {
    const remote = vi.fn<RemotePicker>().mockResolvedValue("/home/me/project");
    setRemotePicker(remote);
    await expect(pickPath({ directory: true }, { startPath: "/home/me" })).resolves.toBe(
      "/home/me/project",
    );
    expect(remote).toHaveBeenCalledWith({ directory: true }, { startPath: "/home/me" });
    expect(ipc).not.toHaveBeenCalled();
  });

  it("fails rather than falling back to this computer's disk", async () => {
    await expect(pickPath({ directory: true })).rejects.toThrow(
      "The remote folder browser is not mounted",
    );
    expect(ipc).not.toHaveBeenCalled();
  });
});
