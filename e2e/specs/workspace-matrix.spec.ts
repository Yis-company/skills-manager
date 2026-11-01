import { agentTarget, preset, project, projectSkill, skill, tool } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const tools = [tool("claude_code", "Claude Code"), tool("codex", "Codex")];

const matrixSeed = {
  tools,
  skills: [
    skill("review", "code-review", { preset_ids: ["engineering"] }),
    skill("context", "project-context", { preset_ids: ["writing"] }),
  ],
  presets: [preset("engineering", "Engineering", 0), preset("writing", "Writing", 1)],
  activePresetId: "engineering",
};

test("staged cells do not mutate, survive hidden rows and require review before apply", async ({
  page,
  backend,
}) => {
  await backend.seed(matrixSeed);
  await page.goto("/my-skills");
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  expect(await backend.calls("sync_skill_to_tool")).toHaveLength(0);
  await page.getByRole("textbox", { name: /Search skills/ }).fill("no matching skill");
  await expect(page.getByRole("button", { name: "Review changes", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  const review = page.getByRole("dialog", { name: "Review changes", exact: true });
  await expect(review).toContainText("code-review");
  await expect(review).toContainText("Codex");
  await expect(review).toContainText("Global workspace");
  await review.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await backend.calls("sync_skill_to_tool")).toHaveLength(0);
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(review).toHaveCount(0);
  expect(await backend.calls("sync_skill_to_tool")).toEqual([{ skillId: "review", tool: "codex" }]);
});

test("navigation and preset browsing cannot silently discard pending targets", async ({
  page,
  backend,
}) => {
  await backend.seed(matrixSeed);
  await page.goto("/my-skills");
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  await page.getByRole("tab", { name: "Instructions", exact: true }).click();
  const leave = page.getByRole("dialog", { name: "Leave staged changes?" });
  await expect(leave).toBeVisible();
  await leave.getByRole("button", { name: "Keep editing" }).click();
  await expect(page.getByRole("tab", { name: "Skills", exact: true })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await page.getByLabel("Viewing preset", { exact: true }).selectOption("writing");
  await expect(leave).toBeVisible();
  await leave.getByRole("button", { name: "Discard and continue" }).click();
  await expect(page.getByText("Shared active preset: Engineering", { exact: true })).toBeVisible();
  expect(await backend.calls("apply_preset_to_default")).toHaveLength(0);
});

test("partial failure retains only failed pairs for review and retry", async ({
  page,
  backend,
}) => {
  await backend.seed(matrixSeed);
  await page.goto("/my-skills");
  await page.getByRole("button", { name: "Add code-review to Claude Code", exact: true }).click();
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  await backend.failNext("sync_skill_to_tool", "host offline");
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  let dialog = page.getByRole("dialog", { name: "Review changes", exact: true });
  await dialog.getByRole("checkbox").check();
  await dialog.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(page.getByRole("alert")).toContainText("host offline");
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  dialog = page.getByRole("dialog", { name: "Review changes", exact: true });
  await expect(dialog).toContainText("Claude Code");
  await expect(dialog).not.toContainText("code-review · Codex");
  await dialog.getByRole("checkbox").check();
  await dialog.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(dialog).toHaveCount(0);
  expect(await backend.calls("sync_skill_to_tool")).toEqual([
    { skillId: "review", tool: "claude_code" },
    { skillId: "review", tool: "codex" },
    { skillId: "review", tool: "claude_code" },
  ]);
});

test("a target changed after review prevents the first mutation", async ({ page, backend }) => {
  await backend.seed(matrixSeed);
  await page.goto("/my-skills");
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Review changes", exact: true });
  await dialog.getByRole("checkbox").check();
  await backend.patch({
    skills: [
      skill("review", "code-review", {
        targets: [
          {
            id: "other",
            skill_id: "review",
            tool: "codex",
            target_path: "/other",
            mode: "copy",
            status: "ok",
            synced_at: 1,
          },
        ],
      }),
    ],
  });
  await dialog.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(page.getByRole("alert")).toContainText("changed since review");
  expect(await backend.calls("sync_skill_to_tool")).toHaveLength(0);
});

for (const scope of ["library", "agent", "project"] as const) {
  test(`${scope} heading persists across resource tabs and optional editors`, async ({
    page,
    backend,
  }) => {
    await backend.seed({
      ...matrixSeed,
      projects: [project("repo", "Workspace project", 0)],
      projectAgentTargets: { repo: [agentTarget("claude_code", "Claude Code")] },
      projectSkills: { repo: [projectSkill("local", "claude_code")] },
    });
    await page.goto(
      scope === "library"
        ? "/my-skills"
        : scope === "agent"
          ? "/global-workspace/claude_code"
          : "/project/repo",
    );

    const name =
      scope === "library" ? "Library" : scope === "agent" ? "Claude Code" : "Workspace project";

    for (const resource of ["Instructions", "MCPs", "Skills"]) {
      await page.getByRole("tab", { name: resource, exact: true }).click();
      await expect(page.getByRole("heading", { level: 1, name, exact: true })).toBeVisible();
    }

    await page.getByRole("tab", { name: "MCPs", exact: true }).click();
    await expect(page.getByRole("textbox", { name: "Command", exact: true })).toHaveCount(0);
    await page.getByRole("button", { name: "Manage definitions and content" }).click();
    await expect(page.getByRole("textbox", { name: "Command", exact: true })).toBeVisible();
    await expect(page.getByRole("heading", { level: 1, name, exact: true })).toBeVisible();
  });
}

test("desktop matrix fits minimum width and supports keyboard staging", async ({
  page,
  backend,
}) => {
  await backend.seed(matrixSeed);
  await page.setViewportSize({ width: 1100, height: 640 });
  await page.goto("/my-skills");
  const add = page.getByRole("button", { name: "Add code-review to Codex", exact: true });
  await add.focus();
  await page.keyboard.press("Space");
  await expect(page.getByText("To add", { exact: true })).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
    ),
  ).toBe(false);
  await page.screenshot({ path: "/tmp/agents-manager-matrix-1100.png" });
});

test("project matrix changes the reviewed assignment without switching the shared preset", async ({
  page,
  backend,
}) => {
  await backend.seed({
    ...matrixSeed,
    projects: [project("repo", "Workspace project", 0)],
    projectAgentTargets: {
      repo: [agentTarget("claude_code", "Claude Code"), agentTarget("codex", "Codex")],
    },
    projectSkills: {
      repo: [
        projectSkill("code-review", "claude_code", {
          center_skill_id: "review",
          in_center: true,
          sync_status: "in_sync",
        }),
      ],
    },
  });
  await page.goto("/project/repo");
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  expect(await backend.calls("set_project_skill_agents")).toHaveLength(0);
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  await page
    .getByRole("dialog", { name: "Review changes", exact: true })
    .getByRole("button", { name: "Apply reviewed changes" })
    .click();
  await expect(
    page.getByRole("button", { name: "Remove code-review from Codex", exact: true }),
  ).toBeVisible();
  expect(await backend.calls("set_project_skill_agents")).toEqual([
    { projectId: "repo", skillRelativePath: "code-review", agentKeys: ["claude_code", "codex"] },
  ]);
  expect(await backend.calls("apply_preset_to_default")).toHaveLength(0);
});

test("MCP cells use the named agent preview and leave content secondary", async ({
  page,
  backend,
}) => {
  await backend.seed({
    ...matrixSeed,
    mcpDefinitions: [
      {
        id: "docs",
        name: "Docs",
        transport: "stdio",
        server: { command: "node", args: ["docs.js"] },
        revision: "1",
        updatedAt: "2026-11-01",
      },
    ],
  });
  await page.goto("/my-skills?resource=mcps");
  await page.getByRole("button", { name: "Add Docs to Codex", exact: true }).click();
  expect(
    (await backend.calls("mcps_request")).filter((call) => call.request.action === "apply"),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  const review = page.getByRole("dialog", { name: "Review changes", exact: true });
  await expect(review).toContainText("Codex");
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(
    page.getByRole("button", { name: "Remove Docs from Codex", exact: true }),
  ).toBeVisible();
  expect(
    (await backend.calls("mcps_request")).find((call) => call.request.action === "preview")
      ?.request,
  ).toMatchObject({ target: { agentKey: "codex" } });
  await expect(page.getByRole("textbox", { name: "Command", exact: true })).toHaveCount(0);
});

test("instruction cells review complete files and then reflect managed presence", async ({
  page,
  backend,
}) => {
  await backend.seed({
    ...matrixSeed,
    instructionBundles: [
      {
        id: "rules",
        name: "Review rules",
        files: { "AGENTS.md": "Review observable behavior." },
        revision: "1",
        updated_at: "2026-11-01",
      },
    ],
  });
  await page.goto("/my-skills?resource=instructions");
  await page.getByRole("button", { name: "Add Review rules to Codex", exact: true }).click();
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  const review = page.getByRole("dialog", { name: "Review changes", exact: true });
  await expect(review).toContainText("AGENTS.md");
  expect(
    (await backend.calls("instructions_request")).filter((call) => call.request.action === "apply"),
  ).toHaveLength(0);
  await review.getByRole("checkbox").check();
  await review.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(
    page.getByRole("button", { name: "Remove Review rules from Codex", exact: true }),
  ).toBeVisible();
});

test("unavailable agents and disabled project copies stay visibly present", async ({
  page,
  backend,
}) => {
  await backend.seed({
    ...matrixSeed,
    tools: [tool("claude_code", "Claude Code"), tool("codex", "Codex", { enabled: false })],
    skills: [
      skill("review", "code-review", {
        targets: [
          {
            id: "on-codex",
            skill_id: "review",
            tool: "codex",
            target_path: "/fixture/review",
            mode: "link",
            status: "ok",
            synced_at: 1,
          },
        ],
      }),
    ],
    projects: [project("repo", "Workspace project", 0)],
    projectAgentTargets: { repo: [agentTarget("claude_code", "Claude Code")] },
    projectSkills: { repo: [projectSkill("disabled-copy", "claude_code", { enabled: false })] },
  });
  await page.goto("/my-skills");
  await expect(
    page.getByRole("button", { name: "Remove code-review from Codex", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await page.goto("/project/repo");
  await expect(
    page.getByRole("button", { name: "Remove disabled-copy from Claude Code", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByText("Present · disabled", { exact: true })).toBeVisible();
});

test("inventory failure keeps presence unknown and blocks target changes", async ({
  page,
  backend,
}) => {
  await backend.seed({
    ...matrixSeed,
    mcpDefinitions: [
      {
        id: "docs",
        name: "Docs",
        transport: "stdio",
        server: { command: "node" },
        revision: "1",
        updatedAt: "2026-11-01",
      },
    ],
  });
  await page.goto("/my-skills?resource=mcps");
  await expect(page.getByRole("button", { name: "Add Docs to Codex", exact: true })).toBeEnabled();
  await backend.failNext("mcps_request", "inventory offline");
  await page.getByRole("button", { name: "Refresh resources" }).click();
  await expect(page.getByRole("alert")).toContainText("inventory offline");
  await expect(page.getByRole("button", { name: "Add Docs to Codex", exact: true })).toBeDisabled();
  expect(
    (await backend.calls("mcps_request")).filter((call) => call.request.action === "apply"),
  ).toHaveLength(0);
});

test("host changes review pending work before connecting another host", async ({
  page,
  backend,
}) => {
  await backend.seed({
    ...matrixSeed,
    remoteHosts: [
      {
        id: "host-two",
        name: "Build host",
        ssh_target: "dev@build-host",
        cli_path: "/opt/agents-manager",
        created_at: 1,
      },
    ],
    remoteStates: { "host-two": matrixSeed },
  });
  await page.goto("/my-skills");
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Build host/ }).click();
  const guard = page.getByRole("dialog", { name: "Leave staged changes?" });
  await expect(guard).toBeVisible();
  await guard.getByRole("button", { name: "Keep editing" }).click();
  await expect(page.getByRole("button", { name: "Local", exact: true })).toBeVisible();
  expect(await backend.calls("remote_host_connect")).toHaveLength(0);
  expect(await backend.calls("sync_skill_to_tool")).toHaveLength(0);
});

test("changing target inventory does not reverse the staged intent", async ({ page, backend }) => {
  await backend.seed(matrixSeed);
  await page.goto("/my-skills");
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  await page.getByRole("button", { name: "Review changes", exact: true }).click();
  const review = page.getByRole("dialog", { name: "Review changes", exact: true });
  await review.getByRole("checkbox").check();
  await backend.patch({
    skills: [
      skill("review", "code-review", {
        targets: [
          {
            id: "external",
            skill_id: "review",
            tool: "codex",
            target_path: "/external",
            mode: "copy",
            status: "ok",
            synced_at: 1,
          },
        ],
      }),
    ],
  });
  await review.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(page.getByRole("alert")).toContainText("changed since review");
  await expect(page.getByText("To add", { exact: true })).toBeVisible();
  await expect(page.getByText("To remove", { exact: true })).toHaveCount(0);
  await page
    .getByRole("button", { name: "Undo staged change for code-review on Codex", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Remove code-review from Codex", exact: true }),
  ).toBeVisible();
  expect(await backend.calls("unsync_skill_from_tool")).toHaveLength(0);
});

for (const theme of ["light", "dark"]) {
  test(`primary actions remain readable at minimum width and large text: ${theme}`, async ({
    page,
    backend,
  }) => {
    await backend.seed({ ...matrixSeed, settings: { theme, text_size: "xlarge" } });
    await page.setViewportSize({ width: 1100, height: 640 });
    await page.goto("/my-skills");
    await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
    const review = page.getByRole("button", { name: "Review changes", exact: true });

    const contrast = () =>
      review.evaluate((element) => {
        const style = getComputedStyle(element);

        const luminance = (color: string) => {
          const channels = color
            .match(/[\d.]+/g)!
            .slice(0, 3)
            .map(Number)
            .map((value) => {
              const linear = value / 255;

              return linear <= 0.04045 ? linear / 12.92 : ((linear + 0.055) / 1.055) ** 2.4;
            });

          return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
        };

        const foreground = luminance(style.color),
          background = luminance(style.backgroundColor);

        return (
          (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05)
        );
      });

    expect(await contrast()).toBeGreaterThanOrEqual(4.5);
    await review.hover();
    expect(await contrast()).toBeGreaterThanOrEqual(4.5);
    await page.getByRole("button", { name: "Discard", exact: true }).click();
    await page.screenshot({ path: `/tmp/agents-manager-matrix-${theme}-large.png` });
  });
}
