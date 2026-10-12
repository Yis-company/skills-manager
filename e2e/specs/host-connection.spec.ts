import type { Page } from "@playwright/test";

import type { RemoteHost } from "../../src/lib/tauri";
import { expect, test } from "../fixtures";

const buildBox: RemoteHost = {
  id: "build-box",
  name: "Build box",
  ssh_target: "dev@build-box",
  cli_path: null,
  created_at: 1,
};

const gpuBox: RemoteHost = {
  id: "gpu-box",
  name: "GPU box",
  ssh_target: "dev@gpu-box",
  cli_path: null,
  created_at: 2,
};

/** The sidebar's host switcher, showing the host the app is on. */
const switcher = (page: Page, label: string) =>
  page.getByRole("button", { name: label, exact: true });

async function pick(page: Page, from: string, host: string) {
  await switcher(page, from).click();
  await page.getByRole("menuitemradio", { name: new RegExp(host) }).click();
}

/** The strip under the title bar while the app is on a host. */
const banner = (page: Page) =>
  page.locator("div[role=status]", {
    has: page.getByRole("button", { name: "Switch to Local" }),
  });

test.beforeEach(async ({ backend }) => {
  await backend.seed({ remoteHosts: [buildBox, gpuBox] });
});

test("a failed switch stays on the host it came from and offers Retry", async ({
  page,
  backend,
}) => {
  await page.goto("/");
  await pick(page, "Local", "Build box");
  await expect(banner(page)).toContainText("Operating on Build box");

  await backend.failNext("remote_host_connect", "Cannot reach dev@gpu-box over ssh");
  await pick(page, "Build box", "GPU box");

  await expect(page.getByText("Couldn’t connect to GPU box")).toBeVisible();
  await expect(switcher(page, "Build box")).toBeVisible();
  await expect(banner(page)).toContainText("Operating on Build box");

  await page.getByRole("button", { name: "Retry" }).click();
  await expect(banner(page)).toContainText("Operating on GPU box");
});

test("Cancel stops a connect in flight and stays where it was", async ({ page, backend }) => {
  await page.goto("/");
  await backend.holdResponse("remote_host_connect");
  await pick(page, "Local", "GPU box");
  await expect(switcher(page, "Connecting to GPU box…")).toBeVisible();

  await switcher(page, "Connecting to GPU box…").click();
  await page.getByRole("menuitem", { name: "Cancel" }).click();
  await expect(switcher(page, "Local")).toBeVisible();

  // The abandoned connect answers late, and changes nothing.
  await backend.releaseResponse("remote_host_connect");
  await expect(switcher(page, "Local")).toBeVisible();
  await expect(banner(page)).toHaveCount(0);
});

test("picking another host while connecting switches there instead", async ({ page, backend }) => {
  await page.goto("/");
  await backend.holdResponse("remote_host_connect");
  await pick(page, "Local", "GPU box");
  await pick(page, "Connecting to GPU box…", "Build box");
  await expect(banner(page)).toContainText("Operating on Build box");

  await backend.releaseResponse("remote_host_connect");
  await expect(banner(page)).toContainText("Operating on Build box");
  expect(await backend.calls("remote_host_connect")).toEqual([
    { hostId: "gpu-box" },
    { hostId: "build-box" },
  ]);
});

test("a dropped link reconnects quietly", async ({ page, backend }) => {
  await page.goto("/");
  await pick(page, "Local", "Build box");
  await expect(banner(page)).toContainText("Operating on Build box");

  await backend.emit("remote-session-state", {
    host_id: "build-box",
    state: "disconnected",
    message: "Connection to Build box was lost",
  });
  await expect(banner(page)).toContainText("Reconnecting to Build box…");
  await expect(banner(page)).toContainText("Operating on Build box");
  await expect(page.getByText("Couldn’t connect to Build box")).toHaveCount(0);
  expect(await backend.calls("remote_host_connect")).toHaveLength(2);
});

test("a request that timed out, and a host that stays unreachable, ends as lost with Reconnect", async ({
  page,
  backend,
}) => {
  await page.clock.install();
  await page.goto("/");
  await pick(page, "Local", "Build box");
  await expect(banner(page)).toContainText("Operating on Build box");

  for (let i = 0; i < 5; i++) {
    await backend.failNext("remote_host_connect", "Cannot reach dev@build-box over ssh");
  }

  // What the backend says when a call heard nothing from the host for 30 s.
  await backend.emit("remote-session-state", {
    host_id: "build-box",
    state: "disconnected",
    message: "Connection to Build box was lost",
  });
  await expect(banner(page)).toContainText("Reconnecting to Build box…");
  await expect(page.getByRole("button", { name: "Reconnect" })).toHaveCount(0);

  // About 30 s of retries, each due within 16 s of the last.
  await expect
    .poll(
      async () => {
        await page.clock.fastForward(16_000);

        return (await backend.calls("remote_host_connect")).length;
      },
      { timeout: 10_000 },
    )
    .toBe(6);
  await expect(banner(page)).toContainText("Connection to Build box was lost.");

  await page.getByRole("button", { name: "Reconnect" }).click();
  await expect(banner(page)).toContainText("Operating on Build box");
});

test("editing the active host reconnects it with the new details", async ({ page, backend }) => {
  await page.goto("/");
  await pick(page, "Local", "Build box");
  await expect(banner(page)).toContainText("dev@build-box");

  await switcher(page, "Build box").click();
  await page.getByRole("menuitem", { name: "Manage hosts…" }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).first().click();
  await page.getByPlaceholder("user@host or an ssh config alias").fill("dev@build-box-2");
  await page.getByRole("button", { name: "Save" }).click();

  await expect(banner(page)).toContainText("dev@build-box-2");
  await expect
    .poll(() => backend.calls("remote_host_connect"))
    .toEqual([{ hostId: "build-box" }, { hostId: "build-box" }]);
});
