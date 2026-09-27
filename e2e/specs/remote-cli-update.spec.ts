import { expect, test } from "../fixtures";
import type { RemoteHost } from "../../src/lib/tauri";

const host: RemoteHost = {
  id: "build-box",
  name: "Build box",
  ssh_target: "dev@build-box",
  cli_path: "/opt/skills-manager/bin/skills-manager",
  created_at: 1,
};

test("version mismatch offers update review, and cancelling performs no install", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");

  await page.getByRole("button", { name: "Connect" }).click();
  await page.getByRole("button", { name: "Review update" }).first().click();
  await expect(page.getByRole("dialog", { name: "Update remote connection CLI" })).toBeVisible();
  await expect(page.getByRole("dialog").getByText("/opt/skills-manager/bin/skills-manager")).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).last().click();

  expect(await backend.calls("remote_host_install_cli")).toEqual([]);
});

test("Settings probe opens the update flow and installs locally before reconnecting", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Review update" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.getByRole("button", { name: "Install matching version and reconnect" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);

  expect(await backend.calls("remote_host_install_cli")).toEqual([{ hostId: "build-box" }]);
  expect(await backend.calls("remote_host_connect")).toEqual([{ hostId: "build-box" }]);
});

test("a failed reconnect after install can be retried from Local", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Review update" }).click();
  await backend.failNext("remote_host_connect", "SSH temporarily unavailable");
  await page.getByRole("button", { name: "Install matching version and reconnect" }).click();

  await expect(page.getByRole("alert")).toContainText("Installed. Connection failed");
  await page.getByRole("dialog").getByRole("button", { name: "Retry" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(await backend.calls("remote_host_install_cli")).toHaveLength(1);
  expect(await backend.calls("remote_host_connect")).toHaveLength(2);
});

test("ordinary connection failures retain Retry", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host] });
  await page.goto("/settings/remote");
  await backend.failNext("remote_host_connect", "SSH temporarily unavailable");
  await page.getByRole("button", { name: "Connect" }).click();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  expect(await backend.calls("remote_host_install_cli")).toEqual([]);
});

test("update action disables repeated installation while the command is pending", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Review update" }).click();
  await backend.hold("remote_host_install_cli");
  const update = page.getByRole("button", { name: "Install matching version and reconnect" });
  await update.click();
  await expect(page.getByRole("dialog").getByRole("status")).toContainText("Checking host");
  await expect(page.getByRole("dialog").getByRole("button").last()).toBeDisabled();
  expect(await backend.calls("remote_host_install_cli")).toHaveLength(1);
  await backend.emit("remote-cli-install-progress", { host_id: "build-box", stage: "downloading" });
  await expect(page.getByRole("dialog").getByRole("status")).toContainText("Downloading CLI");
  await backend.emit("remote-cli-install-progress", { host_id: "build-box", stage: "installing" });
  await expect(page.getByRole("dialog").getByRole("status")).toContainText("Installing CLI");
  await backend.hold("remote_host_connect");
  await backend.release("remote_host_install_cli");
  await expect(page.getByRole("dialog").getByRole("status")).toContainText("Reconnecting");
  await backend.release("remote_host_connect");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
