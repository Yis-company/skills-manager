import { agentTarget, project, projectSkill } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const linked = { in_center: true, center_skill_id: "s1" };

const seed = {
  projects: [project("p1", "webapp", 0)],
  projectAgentTargets: {
    p1: [agentTarget("claude_code", "Claude Code"), agentTarget("codex", "Codex")],
  },
  projectSkills: {
    p1: [
      projectSkill("mixed", "claude_code", { ...linked, sync_status: "center_newer" }),
      projectSkill("mixed", "codex", { ...linked, sync_status: "project_newer" }),
      projectSkill("hidden", "claude_code", { ...linked, sync_status: "center_newer" }),
      projectSkill("conflict", "claude_code", { ...linked, sync_status: "diverged" }),
      projectSkill("current", "claude_code", { ...linked, sync_status: "in_sync" }),
      projectSkill("local-only", "claude_code"),
    ],
  },
};

test("Update All includes hidden copies and preserves locally changed variants", async ({
  page,
  backend,
}) => {
  await backend.seed(seed);
  await page.goto("/project/p1");
  const update = page.getByRole("button", { name: /^Update All/ });
  await expect(update).toBeEnabled();
  await page.getByRole("textbox").first().fill("mixed");
  await expect(page.getByRole("heading", { level: 3 })).toHaveText(["mixed"]);
  await update.click();
  await expect(page.getByRole("region", { name: "Project update progress" })).toContainText(
    "2 updated · 0 failed · 2 need review",
  );

  const submissions = (await backend.calls("operation_request")).filter(
    (call) => call.request.action === "submit_project_update",
  );

  expect(submissions).toHaveLength(1);
  expect(submissions[0]).toMatchObject({ request: { project_id: "p1", host_id: null } });

  if (submissions[0].request.action !== "submit_project_update")
    throw new Error("Expected project submission");
  expect(submissions[0].request.skill_paths).toBeUndefined();
  expect(await backend.calls("update_project_skill_from_center")).toHaveLength(0);
  await expect(update).toBeDisabled();
});
