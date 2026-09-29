import { expect, test } from "../fixtures";
import { project, tool } from "../fake-backend/state";
import type { RemoteHost } from "../../src/lib/tauri";

const remoteHost: RemoteHost = {
  id: "resource-host",
  name: "Resource host",
  ssh_target: "dev@resource-host",
  cli_path: "/opt/agents-manager/bin/agents-manager",
  created_at: 1,
};

test.beforeEach(async ({ page, backend }) => {
  await backend.seed({});
  await page.goto("/my-skills");
});

test("resource tabs follow browser history and survive a reload", async ({
  page,
}) => {
  await page.getByRole("button", { name: "Instructions" }).click();
  await expect(page).toHaveURL(/resource=instructions/);
  await expect(
    page.getByRole("heading", { name: "Instructions", level: 2 }),
  ).toBeVisible();

  await page.getByRole("button", { name: "MCPs" }).click();
  await expect(page).toHaveURL(/resource=mcps/);
  await expect(
    page.getByRole("heading", { name: "MCP library" }),
  ).toBeVisible();

  await page.goBack();
  await expect(page).toHaveURL(/resource=instructions/);
  await expect(
    page.getByRole("heading", { name: "Instructions", level: 2 }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Instructions", level: 2 }),
  ).toBeVisible();
});

test("an instruction bundle can be saved, reviewed, applied, and loaded again", async ({
  page,
  backend,
}) => {
  await page.getByRole("button", { name: "Instructions" }).click();
  await page.getByRole("button", { name: "New", exact: true }).click();
  await page.getByLabel("Agent", { exact: true }).selectOption("codex");
  await page.getByRole("button", { name: "New", exact: true }).click();
  await page
    .getByRole("textbox", { name: "Name", exact: true })
    .fill("Repository guide");
  await page
    .getByRole("textbox", { name: "AGENTS.md content" })
    .fill("# Shared guidance\n\nUse the existing project patterns.");
  await page.getByRole("button", { name: "Save bundle" }).click();
  await expect(page.getByText("Bundle saved.")).toBeVisible();
  await page.getByRole("button", { name: "Review updates" }).click();
  await expect(
    page.getByRole("heading", { name: "Review bundle changes" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(
    page.getByText("Applied 1 file(s).", { exact: true }),
  ).toBeVisible();
  const calls = await backend.calls("instructions_request");
  expect(calls).toContainEqual(
    expect.objectContaining({
      request: expect.objectContaining({ action: "apply" }),
    }),
  );

  await page.reload();
  await expect(
    page.getByText("Repository guide", { exact: true }),
  ).toBeVisible();
});

test("an MCP definition uses an explicit preview before apply", async ({
  page,
  backend,
}) => {
  await page.getByRole("button", { name: "MCPs" }).click();
  await page.getByRole("button", { name: "New", exact: true }).click();
  await page
    .getByRole("textbox", { name: "Name", exact: true })
    .fill("Local docs");
  await page.getByRole("textbox", { name: "Command" }).fill("node");
  await page.getByRole("textbox", { name: "Arguments" }).fill("server.js");
  await page.getByRole("button", { name: "Save definition" }).click();
  await expect(page.getByText("Saved.")).toBeVisible();
  await page.getByRole("button", { name: "Preview deployment" }).click();
  await expect(
    page.getByRole("heading", { name: "Review deployment" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Apply reviewed changes" }).click();
  await expect(
    page.getByText("Applied to this workspace.", { exact: true }),
  ).toBeVisible();
  const calls = await backend.calls("mcps_request");
  expect(calls).toContainEqual(
    expect.objectContaining({
      request: expect.objectContaining({ action: "apply" }),
    }),
  );

  await page.reload();
  await expect(page.getByText("Local docs", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Local docs · managed", { exact: true }).first(),
  ).toBeVisible();
});

test("a delayed remote preview cannot appear after switching back to the local host", async ({
  page,
  backend,
}) => {
  const remoteDefinition = {
    id: "remote-mcp",
    name: "Remote docs",
    transport: "stdio" as const,
    server: { command: "node", args: ["remote.js"] },
    revision: "1",
    updatedAt: "2026-09-27T00:00:00Z",
  };
  await backend.patch({
    remoteHosts: [remoteHost],
    tools: [tool("claude_code", "Claude Code")],
    remoteStates: { [remoteHost.id]: { mcpDefinitions: [remoteDefinition] } },
  });
  await page.reload();
  await page.goto("/my-skills?resource=mcps");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Resource host/ }).click();
  await page.getByText("Remote docs", { exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Preview deployment" }),
  ).toBeEnabled();

  await backend.holdResponse("remote_invoke");
  await page.getByRole("button", { name: "Preview deployment" }).click();
  await expect
    .poll(async () =>
      (await backend.calls("remote_invoke")).some((call) => {
        const invoke = call as {
          command?: string;
          args?: { request?: { action?: string } };
        };
        return (
          invoke.command === "mcps_request" &&
          invoke.args?.request?.action === "preview"
        );
      }),
    )
    .toBe(true);
  await page
    .getByRole("button", { name: "Resource host", exact: true })
    .click();
  await page
    .getByRole("menuitemradio", { name: /Local This computer/ })
    .click();
  await backend.releaseResponse("remote_invoke");

  await expect(
    page.getByRole("heading", { name: "MCP library" }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Review deployment" }),
  ).toHaveCount(0);
  await expect(page.getByText("Remote docs", { exact: true })).toHaveCount(0);
});

test("MCP import opens a draft and removal explicitly detaches without changing the native entry", async ({
  page,
  backend,
}) => {
  await backend.patch({
    mcpTargets: {
      "global:claude_code": [
        {
          name: "Imported docs",
          managedId: "",
          status: "unmanaged",
          definition: {
            name: "Imported docs",
            transport: "stdio",
            server: { command: "node", args: ["docs.js"] },
          },
        },
      ],
    },
  });
  await page.goto("/my-skills?resource=mcps");
  await page
    .getByText("Import from this agent or browse the catalog", { exact: true })
    .click();
  await page
    .getByRole("button", { name: "Review import", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "Name", exact: true }),
  ).toHaveValue("Imported docs");
  expect(
    (await backend.calls("mcps_request")).some(
      (call) =>
        (call as { request: { action: string } }).request.action === "save",
    ),
  ).toBe(false);
  await expect(
    page.getByRole("heading", { name: "Review deployment" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Save definition" }).click();
  await expect(page.getByText("Saved.", { exact: true })).toBeVisible();
  await page
    .getByRole("button", { name: "Remove from library", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Remove and detach", exact: true })
    .click();
  await expect(
    page.getByRole("textbox", { name: "Name", exact: true }),
  ).toHaveValue("");
  expect(await backend.calls("mcps_request")).toContainEqual({
    request: expect.objectContaining({ action: "remove", detach: true }),
  });
  await page.reload();
  await expect(
    page.getByText("Imported docs · unmanaged", { exact: true }).first(),
  ).toBeVisible();
});

test("project resources can select every assigned agent and nested edits retain project-relative paths", async ({
  page,
  backend,
}) => {
  await backend.patch({
    projects: [
      project("repo", "Repo", 0, { agent_keys: ["claude_code", "codex"] }),
    ],
    instructionFiles: {
      "docs/AGENTS.md": {
        content: "Nested rules",
        revision: "1",
        managed: false,
        kind: "nested",
      },
    },
  });
  await page.goto("/project/repo?resource=instructions");
  await page.getByLabel("Agent", { exact: true }).selectOption("codex");
  await page.getByRole("button", { name: /docs\/AGENTS.md/ }).click();
  await expect(
    page.getByRole("textbox", { name: "Instruction file content" }),
  ).toHaveValue("Nested rules");
  await page.getByLabel("Include directory", { exact: true }).fill("docs");
  await page
    .getByRole("heading", { name: "Instructions", exact: true })
    .click();
  await page.getByRole("button", { name: /docs\/AGENTS.md/ }).click();
  await page
    .getByRole("textbox", { name: "Instruction file content" })
    .fill("Updated nested rules");
  await page.getByRole("button", { name: "Save file", exact: true }).click();
  await expect(page.getByText("File saved.", { exact: true })).toBeVisible();
  const calls = (await backend.calls("instructions_request")) as {
    request: { action: string; target: unknown; path?: string };
  }[];
  expect(
    calls.findLast((call) => call.request.action === "write")?.request,
  ).toMatchObject({
    target: { agent_key: "codex", project_id: "repo" },
    path: "docs/AGENTS.md",
  });
  expect(
    calls.findLast((call) => call.request.action === "write")?.request.target,
  ).not.toHaveProperty("relative_dir");
});

test("project worktrees get their own tabs and scope file scans", async ({
  page,
  backend,
}) => {
  await backend.patch({
    projects: [project("repo", "Repo", 0, { agent_keys: ["claude_code"] })],
    instructionWorktrees: [
      { name: "repo", path: "/work/repo", branch: "main", is_main: true },
      {
        name: "fix-login",
        path: "/work/repo/.claude/worktrees/fix-login",
        branch: "fix/login",
        is_main: false,
      },
    ],
  });
  await page.goto("/project/repo?resource=instructions");
  const tabs = page.getByRole("tablist", { name: "Worktrees" });
  await expect(tabs.getByRole("tab", { name: "main (main)" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await tabs.getByRole("tab", { name: "fix-login (fix/login)" }).click();
  await expect
    .poll(async () => {
      const calls = (await backend.calls("instructions_request")) as {
        request: { action: string; target?: { worktree?: string } };
      }[];
      return calls.findLast((call) => call.request.action === "scan")?.request
        .target?.worktree;
    })
    .toBe("/work/repo/.claude/worktrees/fix-login");
});
