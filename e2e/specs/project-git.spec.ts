import type { ProjectGitStatus } from "../../src/lib/projectGit";
import { project } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const status: ProjectGitStatus = {
  review_id: "review-1",
  root: "/work/app",
  branch: "skill-update",
  head: "head-1",
  blocked_reason: null,
  upstream_remote: null,
  upstream_branch: null,
  remotes: [{ name: "origin", url: "git@github.com:example/project.git" }],
  files: [
    {
      path: ".agents/skills/tool/SKILL.md",
      original_path: null,
      status: " M",
      skill_related: true,
      blocked_reason: null,
      diff: "-Old instruction\n+Updated instruction",
      truncated: false,
    },
    {
      path: "src/app.ts",
      original_path: null,
      status: "M ",
      skill_related: false,
      blocked_reason: null,
      diff: "-Unrelated\n+Staged work",
      truncated: false,
    },
    {
      path: ".agents/skills/partial/SKILL.md",
      original_path: null,
      status: "MM",
      skill_related: true,
      blocked_reason: "Partially staged file: commit it outside this dialog.",
      diff: "partial changes",
      truncated: false,
    },
  ],
};

const git = {
  status,
  push: {
    review_id: "push-1",
    remote: "origin",
    branch: "skill-update",
    url: "git@github.com:example/project.git",
    commits: [
      { id: "pre-existing", summary: "Earlier unpushed commit" },
      { id: "new-commit", summary: "Update skill instructions" },
    ],
  },
  pr: {
    review_id: "pr-1",
    repository: "example/project",
    head: "skill-update",
    base: "main",
    bases: ["main", "develop"],
    existing_url: null,
  },
};

const seed = { projects: [project("p1", "webapp", 0)], projectGit: { p1: git } };

test("reviews selected files and outgoing commits before explicit publication actions", async ({
  page,
  backend,
}) => {
  await backend.seed(seed);
  await page.goto("/project/p1");
  await page.getByRole("button", { name: "Git", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("/work/app", { exact: true })).toBeVisible();
  await expect(
    dialog.getByRole("checkbox", { name: ".agents/skills/tool/SKILL.md", exact: true }),
  ).toBeChecked();
  await expect(dialog.getByRole("checkbox", { name: "src/app.ts", exact: true })).not.toBeChecked();
  await expect(
    dialog.getByRole("checkbox", { name: ".agents/skills/partial/SKILL.md", exact: true }),
  ).toBeDisabled();
  await dialog.getByLabel("Commit message", { exact: true }).fill("Update skill instructions");
  await dialog.getByRole("button", { name: "Commit", exact: true }).click();
  await expect
    .poll(async () =>
      (await backend.calls("project_git_request")).filter(
        (call) => call.request.action === "commit",
      ),
    )
    .toEqual([
      {
        projectId: "p1",
        request: {
          action: "commit",
          review_id: "review-1",
          paths: [".agents/skills/tool/SKILL.md"],
          message: "Update skill instructions",
        },
      },
    ]);
  await expect(
    dialog.getByRole("checkbox", { name: ".agents/skills/tool/SKILL.md", exact: true }),
  ).toHaveCount(0);

  await dialog.getByRole("button", { name: "Review push", exact: true }).click();
  await expect(dialog.getByText("Earlier unpushed commit", { exact: true })).toBeVisible();
  await expect(
    dialog.getByRole("list").getByText("Update skill instructions", { exact: true }),
  ).toBeVisible();
  expect(await backend.calls("project_git_request")).not.toContainEqual(
    expect.objectContaining({ request: { action: "push", review_id: "push-1" } }),
  );
  await dialog.getByRole("button", { name: "Push", exact: true }).click();
  await expect
    .poll(async () => await backend.calls("project_git_request"))
    .toContainEqual({ projectId: "p1", request: { action: "push", review_id: "push-1" } });

  await dialog.getByRole("button", { name: "Review pull request", exact: true }).click();
  await dialog.getByLabel("PR title", { exact: true }).fill("Update skills");
  await dialog.getByRole("button", { name: "Create PR", exact: true }).click();
  await expect
    .poll(async () => await backend.calls("project_git_request"))
    .toContainEqual({
      projectId: "p1",
      request: { action: "create_pr", review_id: "pr-1", title: "Update skills" },
    });
  await expect(dialog.getByRole("button", { name: "Open PR", exact: true })).toBeVisible();
});

test("keeps a failed commit review actionable and never pushes automatically", async ({
  page,
  backend,
}) => {
  await backend.seed(seed);
  await page.goto("/project/p1");
  await page.getByRole("button", { name: "Git", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Commit message", { exact: true }).fill("Update skills");
  await backend.failNext(
    "project_git_request",
    "Files changed since review. Refresh and review again.",
  );
  await dialog.getByRole("button", { name: "Commit", exact: true }).click();
  await expect(
    dialog.getByText("Files changed since review. Refresh and review again.", { exact: true }),
  ).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Commit", exact: true })).toBeEnabled();
  const calls = await backend.calls("project_git_request");
  expect(calls.filter((call) => call.request.action === "commit")).toHaveLength(1);
  expect(calls.some((call) => call.request.action === "push")).toBe(false);
});

test("does not offer project Git for a linked skills folder", async ({ page, backend }) => {
  await backend.seed({ projects: [project("p1", "linked", 0, { workspace_type: "linked" })] });
  await page.goto("/project/p1");
  await expect(page.getByRole("heading", { level: 1, name: /^linked\b/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Git", exact: true })).toHaveCount(0);
});

test("a delayed local review cannot replace a remote project's review", async ({
  page,
  backend,
}) => {
  const host = {
    id: "host-1",
    name: "Remote machine",
    ssh_target: "e2e@remote",
    cli_path: null,
    created_at: 1_700_000_000,
  };

  await backend.seed({
    ...seed,
    remoteHosts: [host],
    remoteStates: {
      [host.id]: {
        projects: [project("p1", "remote-project", 0)],
        projectGit: {
          p1: { ...git, status: { ...status, root: "/remote/app", review_id: "remote-review" } },
        },
      },
    },
  });
  await page.goto("/project/p1");
  await backend.holdResponse("project_git_request");
  await page.getByRole("button", { name: "Git", exact: true }).click();
  await expect.poll(async () => (await backend.calls("project_git_request")).length).toBe(1);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .first()
    .click();
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /Remote machine/ }).click();
  await expect(page.getByRole("heading", { level: 1, name: /^remote-project\b/ })).toBeVisible();
  await page.getByRole("button", { name: "Git", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("/remote/app", { exact: true })).toBeVisible();
  await backend.releaseResponse("project_git_request");
  await expect(dialog.getByText("/work/app", { exact: true })).toHaveCount(0);
  await dialog.getByLabel("Commit message", { exact: true }).fill("Remote skill update");
  await dialog.getByRole("button", { name: "Commit", exact: true }).click();
  await expect
    .poll(async () => await backend.calls("remote_invoke"))
    .toContainEqual({
      hostId: host.id,
      command: "project_git_request",
      args: {
        projectId: "p1",
        request: {
          action: "commit",
          review_id: "remote-review",
          paths: [".agents/skills/tool/SKILL.md"],
          message: "Remote skill update",
        },
      },
    });
});

test("reports a successful commit separately from a failed status refresh", async ({
  page,
  backend,
}) => {
  await backend.seed(seed);
  await page.goto("/project/p1");
  await page.getByRole("button", { name: "Git", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Commit message", { exact: true }).fill("Update skills");
  await backend.holdResponse("project_git_request");
  await dialog.getByRole("button", { name: "Commit", exact: true }).click();
  await expect.poll(async () => (await backend.calls("project_git_request")).length).toBe(2);
  await backend.failNext("project_git_request", "Status refresh failed");
  await backend.releaseResponse("project_git_request");
  await expect(dialog.getByText("Commit created.", { exact: true })).toBeVisible();
  await expect(dialog.getByRole("alert")).toHaveText("Status refresh failed");
  await expect(dialog.getByRole("button", { name: "Refresh", exact: true })).toBeEnabled();
  await dialog.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(
    dialog.getByRole("checkbox", { name: ".agents/skills/tool/SKILL.md", exact: true }),
  ).toHaveCount(0);

  const requests = await backend.calls("project_git_request");

  expect(requests.filter((call) => call.request.action === "commit")).toHaveLength(1);
});

test("shows manual refresh errors and retains the review", async ({ page, backend }) => {
  await backend.seed(seed);
  await page.goto("/project/p1");
  await page.getByRole("button", { name: "Git", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("/work/app", { exact: true })).toBeVisible();
  await backend.failNext("project_git_request", "Unable to read the repository");
  await dialog.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(dialog.getByRole("alert")).toHaveText("Unable to read the repository");
  await expect(dialog.getByText("/work/app", { exact: true })).toBeVisible();
});

test("keeps the review within a narrow viewport and returns keyboard focus on close", async ({
  page,
  backend,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await backend.seed(seed);
  await page.goto("/project/p1");
  const open = page.getByRole("button", { name: "Git", exact: true });
  await open.click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("/work/app", { exact: true })).toBeVisible();

  const geometry = await dialog.evaluate((element) => ({
    left: element.getBoundingClientRect().left,
    right: element.getBoundingClientRect().right,
    scroll: element.scrollWidth,
    width: element.clientWidth,
  }));

  expect(geometry.left).toBeGreaterThanOrEqual(0);
  expect(geometry.right).toBeLessThanOrEqual(390);
  expect(geometry.scroll).toBeLessThanOrEqual(geometry.width);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(open).toBeFocused();
});
