import type { Page } from "@playwright/test";

import type { OperationSnapshot } from "../../src/lib/operations";
import type { SkillsShSkill } from "../../src/lib/tauri";
import { expect, type FakeBackend, test } from "../fixtures";

// F2 + F5: the skills.sh market: debounced search, paging and the source
// filter's overflow menu.

const SOURCES = Array.from({ length: 12 }, (_, i) => `contributor-number-${i + 1}/agent-skills`);

const pad = (n: number) => String(n).padStart(2, "0");

// 30 skills: more than one page of 24. Every third one is a "pdf" skill.
const market: SkillsShSkill[] = Array.from({ length: 30 }, (_, i) => {
  const name = i % 3 === 0 ? `pdf-tool-${pad(i + 1)}` : `skill-${pad(i + 1)}`;

  return {
    id: `m${i + 1}`,
    skill_id: name,
    name,
    source: SOURCES[i % SOURCES.length],
    installs: 1000 - i,
  };
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

const card = (page: Page, name: string) =>
  page
    .locator(".app-panel")
    .filter({ has: page.getByRole("heading", { level: 3, name, exact: true }) });

const submits = async (backend: FakeBackend) =>
  (await backend.calls("operation_request")).flatMap((call) =>
    call.request.action === "submit_install" ? [call.request] : [],
  );

const marketOperation = (
  id: string,
  phases: OperationSnapshot["items"][number]["phase"][],
): OperationSnapshot => ({
  operation_id: id,
  attempt_id: "first",
  host_id: null,
  project_id: null,
  project_path: null,
  work: "installation",
  destination: { kind: "library", path: "/library" },
  revision: 3,
  settled: true,
  cancel_requested: false,
  items: phases.map((phase, i) => ({
    id: `item-${i}`,
    attempt_id: "first",
    relative_path: `${market[i].source}/${market[i].skill_id}`,
    agent: null,
    source: { kind: "marketplace", source: market[i].source, skill_id: market[i].skill_id },
    phase,
    message: phase === "failed" ? "Repository could not be reached" : null,
  })),
});

test("selected market batch shows partial results and retries only its failure", async ({
  page,
  backend,
}) => {
  await page.goto("/install");
  await backend.patch({ holdOperations: true });
  await page.getByRole("button", { name: "Select skills" }).click();

  for (const name of ["pdf-tool-01", "skill-02", "skill-03"])
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  await page.getByRole("button", { name: "Install selected (3)" }).click();
  await expect.poll(async () => (await submits(backend)).length).toBe(1);
  const submission = (await submits(backend))[0];
  expect(submission.sources).toEqual(
    market
      .slice(0, 3)
      .map((skill) => ({ kind: "marketplace", source: skill.source, skill_id: skill.skill_id })),
  );
  const partial = marketOperation(submission.operation_id, ["succeeded", "failed", "needs_review"]);
  await backend.patch({ operations: [partial] });
  const progress = page.getByRole("region", { name: "Installation progress" });
  await expect(progress).toContainText("Repository could not be reached");
  await progress.getByRole("button", { name: "Retry failed (1)" }).click();
  await expect(progress).toContainText("Queued");
  await expect(progress).toContainText("Installed");
  await expect(progress).toContainText("Needs review");
  expect(await backend.calls("install_from_skillssh")).toEqual([]);
});

test("independent marketplace submissions remain accepted while work is active", async ({
  page,
  backend,
}) => {
  await page.goto("/install");
  await backend.patch({ holdOperations: true });
  await card(page, "pdf-tool-01").getByRole("button", { name: "Install", exact: true }).click();
  await card(page, "skill-02").getByRole("button", { name: "Install", exact: true }).click();
  await expect.poll(async () => (await submits(backend)).length).toBe(2);
  await expect(card(page, "pdf-tool-01").getByText("Queued", { exact: true })).toHaveCount(2);
  await expect(card(page, "skill-02").getByText("Queued", { exact: true })).toHaveCount(2);
});

test("removing a queued marketplace operation leaves independent work active", async ({
  page,
  backend,
}) => {
  await page.goto("/install");
  await backend.patch({ holdOperations: true });
  await card(page, "pdf-tool-01").getByRole("button", { name: "Install", exact: true }).click();
  await card(page, "skill-02").getByRole("button", { name: "Install", exact: true }).click();
  await page.getByRole("button", { name: "Remove skill-02 from the queue" }).click();
  await expect(
    card(page, "skill-02").getByRole("button", { name: "Install", exact: true }),
  ).toBeVisible();
  await expect(
    card(page, "pdf-tool-01").getByRole("button", { name: "Remove pdf-tool-01 from the queue" }),
  ).toBeVisible();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter((a) => a.request.action === "cancel")
          .length,
    )
    .toBe(1);
});

test("running marketplace cancellation waits for a settled snapshot", async ({ page, backend }) => {
  await page.goto("/install");
  const operation = { ...marketOperation("running", ["applying"]), settled: false };
  await backend.patch({ operations: [operation] });
  await card(page, "pdf-tool-01").getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Stopping",
  );
  await backend.patch({
    operations: [
      {
        ...operation,
        revision: 5,
        settled: true,
        cancel_requested: true,
        items: [{ ...operation.items[0], phase: "canceled" }],
      },
    ],
  });
  await expect(card(page, "pdf-tool-01").getByText("Canceled", { exact: true })).toBeVisible();
});

test("marketplace operations remain visible after visiting the library", async ({
  page,
  backend,
}) => {
  await page.goto("/install");
  await backend.patch({ holdOperations: true });
  await card(page, "pdf-tool-01").getByRole("button", { name: "Install", exact: true }).click();
  await card(page, "skill-02").getByRole("button", { name: "Install", exact: true }).click();
  await page.getByRole("link", { name: "Library", exact: true }).click();
  await page.getByRole("link", { name: /Install Skills.*2 in progress/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toHaveCount(2);
  expect(await backend.calls("install_from_skillssh")).toEqual([]);
});

test("market selection is cleared when moving to another result page", async ({ page }) => {
  await page.goto("/install");
  await page.getByRole("button", { name: "Select skills" }).click();
  await page.getByRole("button", { name: "Select all on this page" }).click();
  await expect(page.getByRole("checkbox")).toHaveCount(24);
  await expect
    .poll(() =>
      page
        .getByRole("checkbox")
        .evaluateAll((inputs) =>
          inputs.every((input) => input instanceof HTMLInputElement && input.checked),
        ),
    )
    .toBe(true);
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

const remoteHost = {
  id: "host-1",
  name: "Remote machine",
  ssh_target: "e2e@remote",
  cli_path: null,
  created_at: 1_700_000_000,
};

const remoteCalls = async (backend: FakeBackend, command: string) =>
  (await backend.calls("remote_invoke")).filter((call) => call.command === command);

/** The existing cancel-before-switch transition remains until SKI-22. */
async function installTwoOnTheHost(page: Page, backend: FakeBackend) {
  await page.goto("/install");
  await backend.patch({
    remoteHosts: [remoteHost],
    remoteStates: { [remoteHost.id]: { holdOperations: true } },
  });
  await page.reload();
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Remote machine/ }).click();
  await expect(page.getByRole("button", { name: "Remote machine", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Select skills" }).click();

  for (const name of ["pdf-tool-01", "skill-02"])
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  await page.getByRole("button", { name: "Install selected (2)" }).click();
  await expect
    .poll(
      async () =>
        (await remoteCalls(backend, "operation_request")).filter((call) =>
          JSON.stringify(call.args).includes('"submit_install"'),
        ).length,
    )
    .toBe(1);
  await page.getByRole("button", { name: "Remote machine", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Local/ }).click();
  await expect(page.getByText("2 skill installs haven’t finished.")).toBeVisible();
}

test("switching hosts cancels unfinished library work through its original host", async ({
  page,
  backend,
}) => {
  await installTwoOnTheHost(page, backend);
  await page.getByRole("button", { name: "Cancel them" }).click();
  await expect(page.getByRole("button", { name: "Local", exact: true })).toBeVisible();

  const cancels = (await remoteCalls(backend, "operation_request")).filter((call) =>
    JSON.stringify(call.args).includes('"cancel"'),
  );

  expect(cancels).toHaveLength(1);
  expect(cancels[0].hostId).toBe(remoteHost.id);
  expect(await submits(backend)).toEqual([]);
});

test("waiting for unfinished library work keeps the current host", async ({ page, backend }) => {
  await installTwoOnTheHost(page, backend);
  await page.getByRole("button", { name: "Wait for them" }).click();
  await expect(page.getByRole("button", { name: "Remote machine", exact: true })).toBeVisible();
  expect(
    (await remoteCalls(backend, "operation_request")).filter((call) =>
      JSON.stringify(call.args).includes('"cancel"'),
    ),
  ).toEqual([]);
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

test("unknown marketplace outcomes cannot be submitted again as an ordinary install", async ({
  page,
  backend,
}) => {
  await page.goto("/install");
  await backend.patch({ operations: [marketOperation("unknown", ["outcome_unknown"])] });
  await expect(
    card(page, "pdf-tool-01").getByText("Outcome unknown", { exact: true }),
  ).toBeVisible();
  await expect(
    card(page, "pdf-tool-01").getByRole("button", { name: "Install", exact: true }),
  ).toBeDisabled();
  expect(await submits(backend)).toEqual([]);
});
