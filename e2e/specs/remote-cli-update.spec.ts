import type { RemoteHost } from "../../src/lib/tauri";
import { expect, test } from "../fixtures";

const host: RemoteHost = {
  id: "build-box",
  name: "Build box",
  ssh_target: "dev@build-box",
  cli_path: "/opt/skills-manager/bin/skills-manager",
  created_at: 1,
};

test("opening and cancelling Update CLI while checking makes no changes", async ({
  page,
  backend,
}) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");
  await backend.hold("remote_host_probe");
  await page.getByRole("button", { name: "Update CLI", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("status")).toContainText("Checking host");
  await expect(
    dialog.getByRole("button", { name: "Install matching version and reconnect" }),
  ).toBeDisabled();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).last().click();
  await backend.release("remote_host_probe");
  await expect(dialog).toHaveCount(0);
  expect(await backend.calls("remote_host_install_cli")).toEqual([]);
  expect(await backend.calls("remote_host_connect")).toEqual([]);
});

test("matching CLI is up to date without reinstalling or reconnecting", async ({
  page,
  backend,
}) => {
  await backend.seed({ remoteHosts: [host] });
  await page.goto("/settings/remote");
  await page.getByRole("button", { name: "Update CLI", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("status")).toContainText("CLI is up to date");
  await expect(dialog.getByRole("status")).toContainText("1.40.0");
  await expect(
    dialog.getByRole("button", { name: "Install matching version and reconnect" }),
  ).toHaveCount(0);
  await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
  await expect(dialog).toHaveCount(0);
  expect(await backend.calls("remote_host_install_cli")).toEqual([]);
  expect(await backend.calls("remote_host_connect")).toEqual([]);
});

test("a failed update probe retries checking without installing", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host] });
  await page.goto("/settings/remote");
  await backend.failNext("remote_host_probe", "SSH temporarily unavailable");
  await page.getByRole("button", { name: "Update CLI", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText("SSH temporarily unavailable");
  await dialog.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(dialog.getByRole("status")).toContainText("CLI is up to date");
  expect(await backend.calls("remote_host_probe")).toHaveLength(2);
  expect(await backend.calls("remote_host_install_cli")).toEqual([]);
  expect(await backend.calls("remote_host_connect")).toEqual([]);
});

test("Update CLI remains reachable at the minimum desktop window size", async ({
  page,
  backend,
}) => {
  await page.setViewportSize({ width: 1100, height: 640 });
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");
  const update = page.getByRole("button", { name: "Update CLI", exact: true });
  await expect(update).toBeInViewport();
  await update.click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("button", { name: "Install matching version and reconnect" }),
  ).toBeEnabled();
  await expect(dialog).toBeInViewport({ ratio: 1 });
});

test("version mismatch offers update review, and cancelling performs no install", async ({
  page,
  backend,
}) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");

  await page.getByRole("button", { name: "Connect" }).click();
  await page.getByRole("button", { name: "Review update" }).first().click();
  await expect(page.getByRole("dialog", { name: "Update remote connection CLI" })).toBeVisible();
  await expect(
    page.getByRole("dialog").getByText("/opt/skills-manager/bin/skills-manager"),
  ).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).last().click();

  expect(await backend.calls("remote_host_install_cli")).toEqual([]);
});

test("Settings Update CLI opens without a prior check and installs locally before reconnecting", async ({
  page,
  backend,
}) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");
  expect(await backend.calls("remote_host_probe")).toEqual([]);
  await page.getByRole("button", { name: "Update CLI", exact: true }).click();
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
  await page.getByRole("button", { name: "Update CLI", exact: true }).click();
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

test("update action disables repeated installation while the command is pending", async ({
  page,
  backend,
}) => {
  await backend.seed({ remoteHosts: [host], remoteCliNeedsUpdate: true });
  await page.goto("/settings/remote");
  await page.getByRole("button", { name: "Check" }).click();
  await page.getByRole("button", { name: "Update CLI", exact: true }).click();
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
