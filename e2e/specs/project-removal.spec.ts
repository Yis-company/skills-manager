import { agentTarget, preset, project, projectSkill, skill } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const target = agentTarget("claude_code", "Claude Code");

const copy = projectSkill("alpha", "claude_code", {
  in_center: true,
  center_skill_id: "s1",
  sync_status: "in_sync",
});

const seed = {
  skills: [skill("s1", "alpha", { preset_ids: ["preset-1"], source_ref: null })],
  presets: [preset("preset-1", "Writing", 0)],
  projects: [project("p1", "webapp", 0, { workspace_type: "linked" })],
  projectAgentTargets: { p1: [target] },
  projectSkills: { p1: [copy] },
};

test.beforeEach(async ({ backend }) => {
  await backend.seed(seed);
});

test("library removal asks first; Escape preserves the skill detail and copy", async ({
  page,
  backend,
}) => {
  await page.goto("/my-skills");
  await page.getByRole("heading", { name: "alpha", exact: true, level: 3 }).click();
  const detail = page.getByRole("dialog", { name: "alpha", exact: true });
  await detail.getByRole("button", { name: /Added/ }).click();
  const confirmation = page.getByRole("dialog", { name: "Remove workspace skill?" });
  await expect(confirmation).toBeVisible();
  expect(await backend.calls("delete_project_skill")).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(confirmation).toHaveCount(0);
  await expect(detail).toBeVisible();
  expect(await backend.calls("delete_project_skill")).toEqual([]);
  await detail.getByRole("button", { name: /Added/ }).click();
  await confirmation.getByRole("button", { name: "Delete", exact: true }).click();
  await expect
    .poll(() => backend.calls("delete_project_skill"))
    .toEqual([
      { projectId: "p1", skillRelativePath: "alpha", agent: "claude_code", wholeSkill: false },
    ]);
});

test("project removal rechecks an edited copy and allows cancelling or confirming", async ({
  page,
  backend,
}) => {
  await page.goto("/project/p1");
  await page.getByRole("heading", { name: "alpha", exact: true, level: 3 }).click();
  const detail = page.getByRole("dialog", { name: "alpha", exact: true });
  await expect(detail).toBeVisible();
  await backend.patch({ projectSkills: { p1: [{ ...copy, sync_status: "project_newer" }] } });
  await detail.getByRole("button", { name: "Claude Code", exact: true }).click();
  const confirmation = page.getByRole("dialog", { name: "Remove copies with local changes?" });
  await expect(confirmation).toBeVisible();
  expect(await backend.calls("delete_project_skill")).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(confirmation).toHaveCount(0);
  await expect(detail).toBeVisible();
  await detail.getByRole("button", { name: "Claude Code", exact: true }).click();
  await confirmation.getByRole("button", { name: "Delete", exact: true }).click();
  await expect
    .poll(() => backend.calls("delete_project_skill"))
    .toEqual([
      { projectId: "p1", skillRelativePath: "alpha", agent: "claude_code", wholeSkill: false },
    ]);
});

test("preset removal asks when a fresh scan fails and cancels on project navigation", async ({
  page,
  backend,
}) => {
  await page.goto("/my-skills");
  await page
    .getByRole("button", { name: "webapp", exact: true })
    .and(page.locator("button"))
    .click();

  const writing = page.getByRole("button", {
    name: "Remove Writing skills from this project",
    exact: true,
  });

  await expect(writing).toBeEnabled();
  await backend.failNext("get_project_skills", "scan failed");
  await writing.click();
  const confirmation = page.getByRole("dialog", { name: "Remove copies with local changes?" });
  await expect(confirmation).toBeVisible();
  await page.goBack();
  await expect(confirmation).toHaveCount(0);
  expect(await backend.calls("delete_project_skill")).toEqual([]);
});
