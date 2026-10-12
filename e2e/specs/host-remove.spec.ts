import type { RemoteHost } from "../../src/lib/tauri";
import { expect, test } from "../fixtures";

const host: RemoteHost = {
  id: "build-box",
  name: "Build box",
  ssh_target: "dev@build-box",
  cli_path: null,
  created_at: 1,
};

test("removing a host also takes it out of Sync by default", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host] });
  await page.goto("/settings/remote");
  await page.getByTitle("Remove").click();
  await expect(page.getByLabel("Also remove from Sync")).toBeChecked();
  await page.getByRole("button", { name: "Remove", exact: true }).last().click();

  await expect(page.getByText("Build box")).toHaveCount(0);
  expect(await backend.calls("host_sync_leave")).toEqual([{ hostId: "build-box" }]);
  expect(await backend.calls("remote_host_remove")).toEqual([{ hostId: "build-box" }]);
});

test("unchecked, the host is only forgotten and keeps syncing", async ({ page, backend }) => {
  await backend.seed({ remoteHosts: [host] });
  await page.goto("/settings/remote");
  await page.getByTitle("Remove").click();
  await page.getByLabel("Also remove from Sync").uncheck();
  await page.getByRole("button", { name: "Remove", exact: true }).last().click();

  await expect(page.getByText("Build box")).toHaveCount(0);
  expect(await backend.calls("host_sync_leave")).toEqual([]);
  expect(await backend.calls("remote_host_remove")).toEqual([{ hostId: "build-box" }]);
});
