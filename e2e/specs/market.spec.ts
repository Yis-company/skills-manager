import { expect, test } from "../fixtures";
import type { SkillsShSkill } from "../../src/lib/tauri";
import type { Page } from "@playwright/test";

// F2 + F5: the skills.sh market: debounced search, paging and the source
// filter's overflow menu.

const SOURCES = Array.from({ length: 12 }, (_, i) => `contributor-number-${i + 1}/agent-skills`);
const pad = (n: number) => String(n).padStart(2, "0");
// 30 skills: more than one page of 24. Every third one is a "pdf" skill.
const market: SkillsShSkill[] = Array.from({ length: 30 }, (_, i) => {
  const name = i % 3 === 0 ? `pdf-tool-${pad(i + 1)}` : `skill-${pad(i + 1)}`;
  return { id: `m${i + 1}`, skill_id: name, name, source: SOURCES[i % SOURCES.length], installs: 1000 - i };
});

const skillNames = (page: Page) => page.getByRole("heading", { level: 3 });
const search = (page: Page) => page.getByPlaceholder("Search skills.sh market...");

test.beforeEach(async ({ backend }) => {
  await backend.seed({ market });
});

test("pages through the leaderboard", async ({ page }) => {
  await page.goto("/install");
  await expect(skillNames(page)).toHaveCount(24);
  await expect(skillNames(page).first()).toHaveText("pdf-tool-01");

  await page.getByRole("button", { name: "Next" }).click();
  await expect(skillNames(page)).toHaveCount(6);
  await expect(skillNames(page).first()).toHaveText("pdf-tool-25");

  await page.getByRole("button", { name: "1", exact: true }).click();
  await expect(skillNames(page)).toHaveCount(24);
  await expect(skillNames(page).first()).toHaveText("pdf-tool-01");
});

test("installs selected market skills, continues after one failure, and retries only failures", async ({ page, backend }) => {
  await page.goto("/install");
  await backend.patch({ failedInstallSkillIds: ["skill-02"] });
  await page.getByRole("button", { name: "Select skills" }).click();
  for (const name of ["pdf-tool-01", "skill-02", "skill-03"]) {
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  }
  await page.getByRole("button", { name: "Install selected (3)" }).click();

  await expect.poll(async () => (await backend.calls("install_from_skillssh")).length).toBe(3);
  expect(await backend.calls("install_from_skillssh")).toEqual([
    { source: SOURCES[0], skillId: "pdf-tool-01" },
    { source: SOURCES[1], skillId: "skill-02" },
    { source: SOURCES[2], skillId: "skill-03" },
  ]);
  await expect(page.getByRole("status").filter({ hasText: "Installed 2 of 3 skills" })).toBeVisible();
  await page.getByRole("button", { name: "Retry failed and unstarted (1)" }).click();
  await expect.poll(async () => (await backend.calls("install_from_skillssh")).length).toBe(4);
  expect((await backend.calls("install_from_skillssh")).at(-1)).toEqual({
    source: SOURCES[1], skillId: "skill-02",
  });
  await expect(page.getByRole("checkbox", { name: "Select pdf-tool-01" })).toBeDisabled();
  await expect(page.getByRole("checkbox", { name: "Select skill-03" })).toBeDisabled();
});

test("stopping a market install leaves later skills unstarted and retryable", async ({ page, backend }) => {
  await page.goto("/install");
  await backend.hold("install_from_skillssh");
  await page.getByRole("button", { name: "Select skills" }).click();
  for (const name of ["pdf-tool-01", "skill-02", "skill-03"]) {
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  }
  await page.getByRole("button", { name: "Install selected (3)" }).click();
  await expect.poll(async () => (await backend.calls("install_from_skillssh")).length).toBe(1);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("checkbox", { name: "Select pdf-tool-01" })).toBeChecked();
  await page.getByRole("button", { name: "Stop", exact: true }).click();
  await backend.release("install_from_skillssh");

  await expect(page.getByRole("status").filter({ hasText: "Stopped after 1 of 3 skills" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry failed and unstarted (2)" })).toBeVisible();
  expect(await backend.calls("install_from_skillssh")).toHaveLength(1);
});

test("market selection is cleared when moving to another result page", async ({ page }) => {
  await page.goto("/install");
  await page.getByRole("button", { name: "Select skills" }).click();
  await page.getByRole("button", { name: "Select all on this page" }).click();
  await expect(page.getByRole("checkbox")).toHaveCount(24);
  await expect.poll(() => page.getByRole("checkbox").evaluateAll((inputs) =>
    inputs.every((input) => (input as HTMLInputElement).checked),
  )).toBe(true);
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("Click skills to select")).toBeVisible();
  await expect(page.getByRole("checkbox", { name: "Select pdf-tool-25" })).not.toBeChecked();
});

test("search prunes selected skills that are no longer visible", async ({ page }) => {
  await page.goto("/install");
  await page.getByRole("button", { name: "Select skills" }).click();
  await page.getByRole("checkbox", { name: "Select skill-02" }).check();
  await search(page).fill("pdf");
  await expect(skillNames(page).first()).toHaveText("pdf-tool-01");
  await expect(skillNames(page)).toHaveCount(10);
  await expect(page.getByText("Click skills to select")).toBeVisible();
});

test("a host switch during a delayed batch keeps the install on its original host", async ({ page, backend }) => {
  const host = {
    id: "host-1",
    name: "Remote machine",
    ssh_target: "e2e@remote",
    cli_path: null,
    created_at: 1_700_000_000,
  };
  await page.goto("/install");
  await backend.patch({ remoteHosts: [host], remoteStates: { [host.id]: {} } });
  await page.reload();
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Remote machine/ }).click();
  await page.getByRole("button", { name: "Remote machine", exact: true }).waitFor();
  await expect.poll(async () => (await backend.calls("remote_invoke")).some(
    (call) => (call as { command: string }).command === "get_managed_skills",
  )).toBe(true);

  await backend.hold("remote_invoke");
  await page.getByRole("button", { name: "Select skills" }).click();
  for (const name of ["pdf-tool-01", "skill-02"]) {
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  }
  await page.getByRole("button", { name: "Install selected (2)" }).click();
  await expect.poll(async () => (await backend.calls("remote_invoke")).filter(
    (call) => (call as { command: string }).command === "install_from_skillssh",
  ).length).toBe(1);

  await page.getByRole("button", { name: "Remote machine", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Local/ }).click();
  await backend.release("remote_invoke");
  await expect(page.getByRole("button", { name: "Local", exact: true })).toBeVisible();
  const remoteInstallCalls = (await backend.calls("remote_invoke")).filter(
    (call) => (call as { command: string }).command === "install_from_skillssh",
  );
  expect(remoteInstallCalls).toHaveLength(1);
  expect(remoteInstallCalls[0]).toMatchObject({ hostId: host.id });
  expect(await backend.calls("install_from_skillssh")).toEqual([]);
});

test("searches once the typing stops", async ({ page, backend }) => {
  await page.clock.install();
  await page.goto("/install");
  await expect(skillNames(page)).toHaveCount(24);
  await page.clock.pauseAt(Date.now() + 60_000);

  await search(page).fill("pd");
  await page.clock.runFor(300);
  await search(page).fill("pdf");
  await page.clock.runFor(300);
  expect(await backend.calls("search_skillssh")).toEqual([]);

  // The debounce restarted at "pdf"; let it run out.
  await expect
    .poll(async () => {
      await page.clock.runFor(100);
      return backend.calls("search_skillssh");
    })
    .toEqual([{ query: "pdf", limit: 60 }]);
  await expect(skillNames(page)).toHaveCount(10);
  for (const name of await skillNames(page).allTextContents()) expect(name).toMatch(/^pdf-tool-/);
});

test("filters by a source from the overflow menu, with the mouse", async ({ page }) => {
  await page.goto("/install");
  await page.getByRole("button", { name: /more$/ }).click();
  await page.getByRole("option", { name: "@contributor-number-12/agent-skills" }).click();

  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(skillNames(page)).toHaveText(["skill-12", "skill-24"]);
});

test("filters by a source from the overflow menu, with the keyboard", async ({ page }) => {
  await page.goto("/install");
  await page.getByRole("button", { name: /more$/ }).click();
  await page.keyboard.type("number-11/");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");

  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(skillNames(page)).toHaveText(["skill-11", "skill-23"]);
});
