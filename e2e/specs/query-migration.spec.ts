import type { RemoteHost } from "../../src/lib/tauri";
import { expect, test } from "../fixtures";
import { agentTarget, project, projectSkill, skill } from "../fake-backend/state";
import type { Page } from "@playwright/test";

const skillHeadings = (page: Page) => page.getByRole("heading", { level: 3 });

async function openDeleteConfirm(page: Page, name: string) {
  const heading = page.getByRole("heading", { name, exact: true, level: 3 });
  await heading.locator("xpath=../..").getByRole("button", { name: "More actions" }).click({ force: true });
  await page.getByRole("button", { name: "Delete", exact: true }).click({ force: true });
  await page.getByRole("button", { name: "Delete", exact: true }).last().click();
}

const host: RemoteHost = {
  id: "host-1",
  name: "Remote machine",
  ssh_target: "e2e@remote",
  cli_path: null,
  created_at: 1_700_000_000,
};

test("confirmed managed deletion stays visible as deleted while its read is held", async ({ page, backend }) => {
  await backend.seed({ skills: [skill("s1", "alpha"), skill("s2", "beta")] });
  await page.goto("/my-skills");
  await expect(skillHeadings(page)).toHaveText(["alpha", "beta"]);

  await backend.hold("get_managed_skills");
  await openDeleteConfirm(page, "alpha");

  await expect(skillHeadings(page)).toHaveText(["beta"]);
  await expect(page.getByRole("heading", { name: /^Library/, level: 1 })).toContainText("1");
  await expect(page.getByText(/loading/i)).toHaveCount(0);

  await backend.release("get_managed_skills");
  await expect(skillHeadings(page)).toHaveText(["beta"]);
  expect(await backend.calls("delete_managed_skill")).toEqual([{ skillId: "s1" }]);
});

test("a rejected managed deletion keeps the row and reports the failure", async ({ page, backend }) => {
  await backend.seed({ skills: [skill("s1", "alpha")] });
  await page.goto("/my-skills");
  await expect(skillHeadings(page)).toHaveText(["alpha"]);
  await backend.failNext("delete_managed_skill", "fake failure");

  await openDeleteConfirm(page, "alpha");

  await expect(skillHeadings(page)).toHaveText(["alpha"]);
  await expect(page.getByText("fake failure")).toBeVisible();
});

test("fulfilled batch deletion removes successful rows and keeps reported failures", async ({ page, backend }) => {
  await backend.seed({
    skills: [skill("s1", "alpha"), skill("s2", "beta"), skill("s3", "gamma")],
    deleteFailedIds: ["s2"],
  });
  await page.goto("/my-skills");
  await expect(skillHeadings(page)).toHaveText(["alpha", "beta", "gamma"]);

  await page.getByRole("button", { name: "Select", exact: true }).click();
  await page.getByRole("button", { name: "Select All", exact: true }).click();
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("button", { name: "Delete 3", exact: true }).click();
  await page.getByRole("button", { name: "Delete", exact: true }).last().click();

  await expect(skillHeadings(page)).toHaveText(["beta"]);
  await expect(page.getByText("1 skills failed to delete")).toBeVisible();
  expect(await backend.calls("delete_managed_skills")).toEqual([{ skillIds: ["s1", "s2", "s3"] }]);
});

test("a rejected batch keeps rows until a held revalidation reports partial writes", async ({ page, backend }) => {
  await backend.seed({
    skills: [skill("s1", "alpha"), skill("s2", "beta")],
    rejectBatchDeleteAfterPartialWrite: true,
  });
  await page.goto("/my-skills");
  await expect(skillHeadings(page)).toHaveText(["alpha", "beta"]);

  await page.getByRole("button", { name: "Select", exact: true }).click();
  await page.getByRole("button", { name: "Select All", exact: true }).click();
  await page.getByRole("button", { name: "More actions" }).click();
  await page.getByRole("button", { name: "Delete 2", exact: true }).click();
  await backend.hold("get_managed_skills");
  await page.getByRole("button", { name: "Delete", exact: true }).last().click();

  await expect(page.getByText("fake batch delete failed after partial write")).toBeVisible();
  await expect(skillHeadings(page)).toHaveText(["alpha", "beta"]);
  await backend.release("get_managed_skills");
  await expect(page.getByRole("heading", { name: "No skills to display", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "alpha", exact: true, level: 3 })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "beta", exact: true, level: 3 })).toHaveCount(0);
});

test("project whole-skill deletion keeps the variant whose delete failed", async ({ page, backend }) => {
  await backend.seed({
    projects: [project("p1", "webapp", 0)],
    projectAgentTargets: { p1: [agentTarget("claude_code", "Claude Code"), agentTarget("codex", "Codex")] },
    projectSkills: {
      p1: [
        projectSkill("deploy", "claude_code", { in_center: true, sync_status: "project_newer" }),
        projectSkill("deploy", "codex", { in_center: true, sync_status: "project_newer" }),
      ],
    },
  });
  await page.goto("/project/p1");
  await expect(page.getByRole("heading", { name: "deploy", exact: true, level: 3 })).toBeVisible();
  await backend.failNext("delete_project_skill", "first variant failed");

  await page.getByRole("button", { name: "Delete skill", exact: true }).click();
  await page.getByRole("button", { name: "Delete", exact: true }).last().click();

  await expect(page.getByRole("heading", { name: "deploy", exact: true, level: 3 })).toBeVisible();
  const deletes = await backend.calls("delete_project_skill");
  expect(deletes).toHaveLength(2);
  expect(deletes).toContainEqual({ projectId: "p1", skillRelativePath: "deploy", agent: "claude_code", wholeSkill: true });
  expect(deletes).toContainEqual({ projectId: "p1", skillRelativePath: "deploy", agent: "codex", wholeSkill: true });
});

test("a failed vendored delete keeps its aliases while a successful project delete is held for refresh", async ({ page, backend }) => {
  await backend.seed({
    projects: [project("p1", "webapp", 0)],
    projectAgentTargets: { p1: [agentTarget("cline", "Cline"), agentTarget("claude_code", "Claude Code"), agentTarget("codex", "Codex")] },
    projectSkills: {
      p1: [
        projectSkill("a-vendored", "cline", {
          relative_path: "a-vendored",
          path: "/home/e2e/code/webapp/.agents/skills/a-vendored",
          vendored: true,
        }),
        projectSkill("a-vendored", "claude_code", {
          relative_path: "a-vendored",
          path: "/home/e2e/code/webapp/.claude/skills/a-vendored",
          alias_of: "a-vendored",
        }),
        projectSkill("z-independent", "codex", { relative_path: "z-independent" }),
      ],
    },
  });
  await page.goto("/project/p1");
  await expect(page.getByRole("heading", { level: 3 })).toHaveText(["a-vendored", "z-independent"]);
  await page.getByRole("button", { name: "Select", exact: true }).click();
  await page.getByRole("button", { name: "Select All", exact: true }).click();
  await page.getByRole("button", { name: "More actions", exact: true }).click();
  await page.getByRole("button", { name: "Delete 2", exact: true }).click();
  await backend.failNext("delete_project_skill", "vendored delete failed");
  await backend.hold("get_project_skills");
  await page.getByRole("button", { name: "Delete", exact: true }).last().click();

  await expect(page.getByRole("heading", { level: 3 })).toHaveText(["a-vendored"]);
  await expect(page.getByText("1 skills failed to delete")).toBeVisible();
  await expect(page.locator('[title="2/2"]')).toBeVisible();
  await backend.release("get_project_skills");
  await expect(page.getByRole("heading", { level: 3 })).toHaveText(["a-vendored"]);
  expect(await backend.calls("delete_project_skill")).toHaveLength(2);
});

test("old host read responses cannot replace the selected host's colliding skill", async ({ page, backend }) => {
  await backend.seed({
    remoteHosts: [host],
    skills: [skill("same-id", "Local copy")],
    remoteStates: { [host.id]: { skills: [skill("same-id", "Remote copy")] } },
  });
  await page.goto("/my-skills");
  await expect(skillHeadings(page)).toHaveText(["Local copy"]);

  const initialReads = (await backend.calls("get_managed_skills")).length;
  await backend.holdResponse("get_managed_skills");
  await backend.emit("app-files-changed", {});
  await expect.poll(async () => (await backend.calls("get_managed_skills")).length).toBeGreaterThan(initialReads);

  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Remote machine/ }).click();
  await expect(skillHeadings(page)).toHaveText(["Remote copy"]);
  expect(await backend.calls("remote_invoke")).toContainEqual({
    hostId: host.id,
    command: "get_managed_skills",
    args: {},
  });

  await backend.releaseResponse("get_managed_skills");
  await expect(skillHeadings(page)).toHaveText(["Remote copy"]);
});

test("a deletion finishing after a host switch only changes its originating host", async ({ page, backend }) => {
  await backend.seed({
    remoteHosts: [host],
    skills: [skill("same-id", "Local copy")],
    remoteStates: { [host.id]: { skills: [skill("same-id", "Remote copy")] } },
  });
  await page.goto("/my-skills");
  await expect(skillHeadings(page)).toHaveText(["Local copy"]);
  await backend.hold("delete_managed_skill");
  const heading = page.getByRole("heading", { name: "Local copy", exact: true, level: 3 });
  await heading.locator("xpath=../..").getByRole("button", { name: "More actions" }).click({ force: true });
  await page.getByRole("button", { name: "Delete", exact: true }).click({ force: true });
  await page.getByRole("button", { name: "Delete", exact: true }).last().click();
  await expect.poll(async () => (await backend.calls("delete_managed_skill")).length).toBe(1);

  await page.getByRole("button", { name: "Cancel", exact: true }).last().click();
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Remote machine/ }).click();
  await expect(skillHeadings(page)).toHaveText(["Remote copy"]);

  await backend.release("delete_managed_skill");
  await expect(skillHeadings(page)).toHaveText(["Remote copy"]);
  await page.getByRole("button", { name: "Remote machine", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Local/ }).click();
  await expect(page.getByRole("heading", { name: "Local copy", exact: true, level: 3 })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "No skills to display", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Remote machine/ }).click();
  await expect(skillHeadings(page)).toHaveText(["Remote copy"]);
  expect(await backend.calls("remote_invoke")).not.toContainEqual(expect.objectContaining({ command: "delete_managed_skill" }));
});

test("a failed background read leaves the loaded library visible", async ({ page, backend }) => {
  await backend.seed({ skills: [skill("s1", "alpha"), skill("s2", "beta")] });
  await page.goto("/my-skills");
  await expect(skillHeadings(page)).toHaveText(["alpha", "beta"]);
  await backend.failNext("get_managed_skills", "background refresh failed");

  await backend.emit("app-files-changed", {});

  await expect(page.getByText("Some data could not be refreshed")).toBeVisible();
  await expect(skillHeadings(page)).toHaveText(["alpha", "beta"]);
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByText("Some data could not be refreshed")).toHaveCount(0);
  await expect(skillHeadings(page)).toHaveText(["alpha", "beta"]);
});
