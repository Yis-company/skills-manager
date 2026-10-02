import type { ProjectSkill, RemoteHost, SkillTarget } from "../../src/lib/tauri";
import { agentTarget, project, projectSkill, skill, tool } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const skillTarget = (skillId: string, agent: string): SkillTarget => ({
  id: `${skillId}-${agent}`,
  skill_id: skillId,
  tool: agent,
  target_path: `/home/e2e/.${agent}/skills/${skillId}`,
  mode: "link",
  status: "ok",
  synced_at: null,
});

const localSkill = (
  name: string,
  agent: string,
  centerSkillId: null | string = null,
): ProjectSkill =>
  projectSkill(name, agent, {
    agent_display_name: agent === "claude_code" ? "Claude Code" : "Codex",
    center_skill_id: centerSkillId,
    in_center: centerSkillId !== null,
    sync_status: centerSkillId ? "in_sync" : "project_only",
  });

const workspaceTools = [tool("claude_code", "Claude Code"), tool("codex", "Codex")];

test("project bulk removal is visible, scoped to the project, and cancelable", async ({
  page,
  backend,
}) => {
  await backend.seed({
    projects: [project("p1", "webapp", 0)],
    projectAgentTargets: { p1: [agentTarget("claude_code", "Claude Code")] },
    projectSkills: {
      p1: [projectSkill("project-only", "claude_code")],
    },
  });
  await page.goto("/project/p1");
  await expect(page.getByRole("heading", { name: "project-only", level: 3 })).toBeVisible();

  await page.getByRole("button", { name: "Select skills" }).click();
  await page.getByRole("heading", { name: "project-only", level: 3 }).click();
  await page.getByRole("button", { name: "Remove selected (1)" }).click();
  await expect(page.getByText(/project copies across agents/)).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).last().click();
  expect(await backend.calls("delete_project_skill")).toHaveLength(0);
  await expect(page.getByRole("button", { name: "Remove selected (1)" })).toBeVisible();

  await page.getByRole("button", { name: "Remove selected (1)" }).click();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("heading", { name: "project-only", level: 3 })).toHaveCount(0);
  expect(await backend.calls("delete_project_skill")).toEqual([
    { projectId: "p1", skillRelativePath: "project-only", agent: "claude_code", wholeSkill: true },
  ]);
});

test("mixed workspace removal keeps the library and other selected category, then deletes only local files", async ({
  page,
  backend,
}) => {
  const targets = [skillTarget("managed-a", "claude_code"), skillTarget("managed-a", "codex")];
  await backend.seed({
    tools: workspaceTools,
    skills: [skill("managed-a", "Managed A", { targets })],
    globalLocalSkills: {
      claude_code: [
        localSkill("Managed A", "claude_code", "managed-a"),
        localSkill("Loose file", "claude_code"),
      ],
      codex: [localSkill("Managed A", "codex", "managed-a")],
    },
  });
  await page.goto("/global-workspace/claude_code");
  const localHeading = (name: string) => page.getByRole("heading", { name, level: 3 });
  await expect(localHeading("Managed A")).toBeVisible();
  await expect(localHeading("Loose file")).toBeVisible();

  await page.getByRole("button", { name: "Select skills" }).click();
  await localHeading("Managed A").click();
  await localHeading("Loose file").click();
  await page.getByRole("button", { name: "Remove from Claude Code (1)" }).click();
  await expect(page.getByText(/only unlinks them from the global workspace/)).toBeVisible();
  await page.getByRole("button", { name: "Remove", exact: true }).click();

  await expect(localHeading("Managed A")).toHaveCount(0);
  await expect(localHeading("Loose file")).toBeVisible();
  await expect(page.getByRole("button", { name: "Delete local files (1)" })).toBeVisible();
  expect(await backend.calls("unsync_skill_from_tool")).toEqual([
    { skillId: "managed-a", tool: "claude_code" },
  ]);

  await page.goto("/my-skills");
  await expect(page.getByRole("heading", { name: "Managed A", level: 3 })).toBeVisible();
  await page.goto("/global-workspace/codex");
  await expect(localHeading("Managed A")).toBeVisible();

  await page.goto("/global-workspace/claude_code");
  await expect(localHeading("Loose file")).toBeVisible();
  await page.getByRole("button", { name: "Select skills" }).click();
  await localHeading("Loose file").click();
  await expect(page.getByRole("button", { name: "Delete local files (1)" })).toBeVisible();
  await page.getByRole("button", { name: "Delete local files (1)" }).click();
  await expect(page.getByText(/This removes the local files/)).toBeVisible();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(localHeading("Loose file")).toHaveCount(0);
  expect(await backend.calls("delete_global_local_skill")).toEqual([
    { agent: "claude_code", skillRelativePath: "Loose file" },
  ]);
  await page.goto("/my-skills");
  await expect(page.getByRole("heading", { name: "Managed A", level: 3 })).toBeVisible();
});

test("a partial workspace failure keeps only failed skills selected for retry", async ({
  page,
  backend,
}) => {
  await backend.seed({
    tools: workspaceTools,
    skills: [
      skill("managed-fail", "Alpha failure", {
        targets: [skillTarget("managed-fail", "claude_code")],
      }),
      skill("managed-success", "Beta success", {
        targets: [skillTarget("managed-success", "claude_code")],
      }),
    ],
    globalLocalSkills: {
      claude_code: [
        localSkill("Alpha failure", "claude_code", "managed-fail"),
        localSkill("Beta success", "claude_code", "managed-success"),
      ],
    },
  });
  await page.goto("/global-workspace/claude_code");
  const skillHeading = (name: string) => page.getByRole("heading", { name, level: 3 });
  await expect(skillHeading("Alpha failure")).toBeVisible();
  await page.getByRole("button", { name: "Select skills" }).click();
  await skillHeading("Alpha failure").click();
  await skillHeading("Beta success").click();
  await backend.failNext("unsync_skill_from_tool", "network unavailable");
  await page.getByRole("button", { name: "Remove from Claude Code (2)" }).click();
  await page.getByRole("button", { name: "Remove", exact: true }).click();

  await expect(skillHeading("Alpha failure")).toBeVisible();
  await expect(skillHeading("Beta success")).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("Alpha failure: network unavailable");
  await expect(page.getByRole("button", { name: "Remove from Claude Code (1)" })).toBeVisible();
  await page.getByRole("button", { name: "Remove from Claude Code (1)" }).click();
  await page.getByRole("button", { name: "Remove", exact: true }).click();
  await expect(skillHeading("Alpha failure")).toHaveCount(0);
  expect(await backend.calls("unsync_skill_from_tool")).toEqual([
    { skillId: "managed-fail", tool: "claude_code" },
    { skillId: "managed-success", tool: "claude_code" },
    { skillId: "managed-fail", tool: "claude_code" },
  ]);
});

test("a delayed local batch stays on its captured host when the host changes", async ({
  page,
  backend,
}) => {
  const remoteHost: RemoteHost = {
    id: "batch-remote",
    name: "Batch remote",
    ssh_target: "dev@batch-remote",
    cli_path: "/opt/agents-manager/bin/agents-manager",
    created_at: 1,
  };

  await backend.seed({
    tools: workspaceTools,
    globalLocalSkills: { claude_code: [localSkill("Same path", "claude_code")] },
    remoteHosts: [remoteHost],
    remoteStates: {
      [remoteHost.id]: {
        tools: workspaceTools,
        globalLocalSkills: { claude_code: [localSkill("Same path", "claude_code")] },
      },
    },
  });
  await page.goto("/global-workspace/claude_code");
  const skillHeading = page.getByRole("heading", { name: "Same path", level: 3 });
  await expect(skillHeading).toBeVisible();
  await page.getByRole("button", { name: "Select skills" }).click();
  await skillHeading.click();
  await backend.hold("delete_global_local_skill");
  await page.getByRole("button", { name: "Delete local files (1)" }).click();
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("button", { name: "Loading...", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Cancel", exact: true }).last()).toBeDisabled();

  // Exercise the global host switch while the local delete invoke is held.
  await page.getByRole("button", { name: "Local", exact: true }).evaluate((element) => {
    if (element instanceof HTMLElement) element.click();
  });
  await page.getByRole("menuitemradio", { name: /Batch remote/ }).evaluate((element) => {
    if (element instanceof HTMLElement) element.click();
  });
  await expect(page.getByRole("button", { name: "Batch remote", exact: true })).toBeVisible();
  await backend.release("delete_global_local_skill");
  await expect.poll(async () => (await backend.calls("delete_global_local_skill")).length).toBe(1);
  await expect(skillHeading).toBeVisible();
  expect(await backend.calls("remote_invoke")).not.toContainEqual(
    expect.objectContaining({ command: "delete_global_local_skill" }),
  );
});
