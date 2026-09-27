import { expect, test } from "../fixtures";
import { agentTarget, project, projectSkill } from "../fake-backend/state";

const targets = [agentTarget("claude_code", "Claude Code"), agentTarget("codex", "Codex")];
const linked = { in_center: true, center_skill_id: "s1" };
const copies = [
  projectSkill("mixed", "claude_code", { ...linked, sync_status: "center_newer" }),
  projectSkill("mixed", "codex", { ...linked, sync_status: "project_newer" }),
  projectSkill("hidden", "claude_code", { ...linked, sync_status: "center_newer" }),
  projectSkill("conflict", "claude_code", { ...linked, sync_status: "diverged" }),
  projectSkill("current", "claude_code", { ...linked, sync_status: "in_sync" }),
  projectSkill("local-only", "claude_code"),
];
const seed = {
  projects: [project("p1", "webapp", 0)],
  projectAgentTargets: { p1: targets },
  projectSkills: { p1: copies },
};

test("Update All includes hidden copies and preserves locally changed variants", async ({ page, backend }) => {
  await backend.seed(seed);
  await page.goto("/project/p1");
  const update = page.getByRole("button", { name: /^Update All/ });
  await expect(update).toBeEnabled();
  await page.getByRole("textbox").first().fill("mixed");
  await expect(page.getByRole("heading", { level: 3 })).toHaveText(["mixed"]);

  await update.click();

  await expect(page.getByText("Project update complete: 2 updated, 2 skipped for review, 0 failed.")).toBeVisible();
  expect(await backend.calls("update_project_skill_from_center")).toEqual([
    { projectId: "p1", skillRelativePath: "hidden", agent: "claude_code" },
    { projectId: "p1", skillRelativePath: "mixed", agent: "claude_code" },
  ]);
  await expect(update).toBeDisabled();
});

test("Update All continues after a failed copy and disables duplicate submissions", async ({ page, backend }) => {
  await backend.seed({
    ...seed,
    projectSkills: { p1: [
      projectSkill("shared", "claude_code", { ...linked, sync_status: "center_newer" }),
      projectSkill("shared", "codex", { ...linked, sync_status: "center_newer" }),
    ] },
  });
  await page.goto("/project/p1");
  const update = page.getByRole("button", { name: /^Update All/ });
  await expect(update).toBeEnabled();
  await backend.failNext("update_project_skill_from_center", "copy is read-only");
  await backend.hold("update_project_skill_from_center");
  await update.click();
  await expect(update).toBeDisabled();
  await backend.release("update_project_skill_from_center");

  await expect(page.getByText("Project update complete: 1 updated, 0 skipped for review, 1 failed.")).toBeVisible();
  expect(await backend.calls("update_project_skill_from_center")).toEqual([
    { projectId: "p1", skillRelativePath: "shared", agent: "claude_code" },
    { projectId: "p1", skillRelativePath: "shared", agent: "codex" },
  ]);
  await expect(update).toBeEnabled();
});

test("an update sequence stays on its originating host after switching hosts", async ({ page, backend }) => {
  const host = { id: "host-1", name: "Remote machine", ssh_target: "e2e@remote", cli_path: null, created_at: 1_700_000_000 };
  await backend.seed({
    ...seed,
    remoteHosts: [host],
    remoteStates: { [host.id]: { ...seed, projects: [project("p1", "remote-project", 0)] } },
  });
  await page.goto("/project/p1");
  const update = page.getByRole("button", { name: /^Update All/ });
  await expect(update).toBeEnabled();
  await backend.hold("update_project_skill_from_center");
  await update.click();
  await expect.poll(async () => (await backend.calls("update_project_skill_from_center")).length).toBe(1);

  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Remote machine/ }).click();
  await expect(page.getByRole("button", { name: "Remote machine", exact: true })).toBeVisible();
  await backend.release("update_project_skill_from_center");

  await expect.poll(async () => (await backend.calls("update_project_skill_from_center")).length).toBe(2);
  expect(await backend.calls("remote_invoke")).not.toContainEqual(
    expect.objectContaining({ command: "update_project_skill_from_center" }),
  );
});
