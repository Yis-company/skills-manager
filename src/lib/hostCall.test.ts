import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { getActiveHostId, invoke, invokeHost, setActiveHostId, trackHost } from "./hostCall";
import {
  getCentralRepoPendingPath,
  getLocalSettings,
  getSettings,
  remoteHostInstallCli,
} from "./tauri";
import { mockTauriIpc } from "./tauriIpcTesting";

const tauriInvoke = mockTauriIpc();

beforeEach(() => tauriInvoke.mockResolvedValue("ok"));

afterEach(() => setActiveHostId(null));

describe("invoke on this computer", () => {
  it("runs every command locally", async () => {
    await invoke("get_managed_skills");
    await invoke("set_settings", { key: "sync_mode", value: "copy" });
    expect(tauriInvoke).toHaveBeenNthCalledWith(1, "get_managed_skills", {});
    expect(tauriInvoke).toHaveBeenNthCalledWith(2, "set_settings", {
      key: "sync_mode",
      value: "copy",
    });
  });
});

describe("invoke with a remote host active", () => {
  beforeEach(() => setActiveHostId("host-1"));

  it("sends host-scoped commands through remote_invoke with their args", async () => {
    const args = { skillId: "s1", tool: "claude" };
    await expect(invoke("sync_skill_to_tool", args)).resolves.toBe("ok");
    expect(tauriInvoke).toHaveBeenCalledWith("remote_invoke", {
      hostId: "host-1",
      command: "sync_skill_to_tool",
      args,
    });
  });

  it("sends an empty args object when the command takes none", async () => {
    await invoke("get_presets");
    expect(tauriInvoke).toHaveBeenCalledWith("remote_invoke", {
      hostId: "host-1",
      command: "get_presets",
      args: {},
    });
  });

  it("routes settings by key", async () => {
    await invoke("get_settings", { key: "proxy_url" });
    await invoke("get_settings", { key: "theme" });
    expect(tauriInvoke).toHaveBeenNthCalledWith(1, "remote_invoke", {
      hostId: "host-1",
      command: "get_settings",
      args: { key: "proxy_url" },
    });
    expect(tauriInvoke).toHaveBeenNthCalledWith(2, "get_settings", { key: "theme" });
  });

  it("reads the pending library move from the selected host", async () => {
    tauriInvoke.mockResolvedValueOnce("/srv/new-library");
    await expect(getCentralRepoPendingPath()).resolves.toBe("/srv/new-library");
    expect(tauriInvoke).toHaveBeenCalledWith("remote_invoke", {
      hostId: "host-1",
      command: "get_central_repo_pending_path",
      args: {},
    });
  });

  it("keeps local commands local", async () => {
    await invoke("git_backup_status");
    await invoke("remote_host_disconnect");
    expect(tauriInvoke).toHaveBeenNthCalledWith(1, "git_backup_status", {});
    expect(tauriInvoke).toHaveBeenNthCalledWith(2, "remote_host_disconnect", {});
  });

  it("passes remote errors through unchanged", async () => {
    const error = { kind: "network", message: "Connection to box was lost" };
    tauriInvoke.mockRejectedValueOnce(error);
    await expect(invoke("get_managed_skills")).rejects.toBe(error);
  });

  it("reads this computer's proxy for the app updater", async () => {
    await getSettings("proxy_url");
    await getLocalSettings("proxy_url");
    expect(tauriInvoke).toHaveBeenNthCalledWith(1, "remote_invoke", {
      hostId: "host-1",
      command: "get_settings",
      args: { key: "proxy_url" },
    });
    expect(tauriInvoke).toHaveBeenNthCalledWith(2, "get_settings", { key: "proxy_url" });
  });

  it("goes back to local calls after switching to this computer", async () => {
    setActiveHostId(null);
    expect(getActiveHostId()).toBeNull();
    await invoke("get_managed_skills");
    expect(tauriInvoke).toHaveBeenCalledWith("get_managed_skills", {});
  });

  it("installs a remote host CLI on this computer even while a remote host is active", async () => {
    await remoteHostInstallCli("host-2");
    expect(tauriInvoke).toHaveBeenCalledWith("remote_host_install_cli", { hostId: "host-2" });
  });
});

describe("invokeHost", () => {
  it("uses the captured host even after the active host changes", async () => {
    setActiveHostId("host-2");

    await invokeHost("host-1", "get_project_skills", { projectId: "project-1" });

    expect(tauriInvoke).toHaveBeenCalledWith("remote_invoke", {
      hostId: "host-1",
      command: "get_project_skills",
      args: { projectId: "project-1" },
    });
  });

  it("keeps local-only commands on this computer for a remote host", async () => {
    await invokeHost("host-1", "remote_hosts_list");

    expect(tauriInvoke).toHaveBeenCalledWith("remote_hosts_list", {});
  });
});

describe("trackHost", () => {
  it("tells whether an answer still belongs to the machine on screen", () => {
    setActiveHostId("host-1");
    const onHost1 = trackHost();
    expect(onHost1()).toBe(true);

    setActiveHostId("host-2");
    expect(onHost1()).toBe(false);
    const onHost2 = trackHost();

    setActiveHostId(null);
    expect(onHost2()).toBe(false);
    expect(trackHost()()).toBe(true);
  });
});
