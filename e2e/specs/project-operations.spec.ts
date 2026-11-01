import type { OperationSnapshot } from "../../src/lib/operations";
import { agentTarget, project, projectSkill } from "../fake-backend/state";
import { expect, test } from "../fixtures";

test("project progress survives navigation while another project accepts work", async ({
  page,
  backend,
}) => {
  await backend.seed({
    projects: [project("p1", "webapp", 0), project("p2", "api", 1)],
    projectAgentTargets: {
      p1: [agentTarget("claude_code", "Claude Code")],
      p2: [agentTarget("claude_code", "Claude Code")],
    },
    projectSkills: Object.fromEntries(
      ["p1", "p2"].map((id) => [
        id,
        [
          projectSkill("x", "claude_code", {
            in_center: true,
            center_skill_id: "x",
            sync_status: "center_newer",
          }),
        ],
      ]),
    ),
    holdOperations: true,
  });
  await page.goto("/project/p1");
  await page.getByRole("button", { name: /^Update All/ }).click();
  await expect(page.getByRole("region", { name: "Project update progress" })).toContainText(
    "0 of 1",
  );
  await expect(page.getByText("Claude Code: Queued", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Remove x from Claude Code", exact: true }),
  ).toBeDisabled();
  await page.getByRole("button", { name: /api/ }).first().click();
  await expect(page.getByRole("heading", { name: "api", exact: true })).toBeVisible();
  await page.getByRole("button", { name: /^Update All/ }).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter(
          (a) => a.request.action === "submit_project_update",
        ).length,
    )
    .toBe(2);
  await page
    .getByRole("button", { name: /webapp/ })
    .first()
    .click();
  await expect(page.getByRole("region", { name: "Project update progress" })).toContainText(
    "0 of 1",
  );
  await page.getByRole("button", { name: "Cancel update", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("region", { name: "Project update progress" })).toContainText(
    "1 canceled",
  );
  await expect(page.getByRole("region", { name: "Project update progress" })).toBeFocused();
  await page.getByRole("button", { name: /api/ }).first().click();
  await expect(page.getByRole("button", { name: "Cancel update", exact: true })).toBeVisible();
});

test("Retry failed preserves successful, review and unknown outcomes", async ({
  page,
  backend,
}) => {
  const operation: OperationSnapshot = {
    operation_id: "partial",
    attempt_id: "first",
    host_id: null,
    project_id: "p1",
    project_path: "/home/e2e/code/webapp",
    revision: 4,
    settled: true,
    cancel_requested: false,
    items: (["succeeded", "failed", "needs_review", "outcome_unknown"] as const).map(
      (phase, i) => ({
        id: `item-${i}`,
        attempt_id: "first",
        relative_path: `skill-${i}`,
        agent: "claude_code",
        phase,
        message: null,
      }),
    ),
  };

  await backend.seed({ projects: [project("p1", "webapp", 0)], operations: [operation] });
  await page.goto("/project/p1");
  const progress = page.getByRole("region", { name: "Project update progress" });
  await expect(progress).toContainText("1 updated · 1 failed · 1 need review");
  await page.getByRole("button", { name: "Retry failed (1)", exact: true }).click();
  await expect(progress).toContainText("2 updated · 0 failed · 1 need review");
  await expect(progress).toContainText("1 outcome unknown");

  const retries = (await backend.calls("operation_request")).filter(
    (call) => call.request.action === "retry_failed",
  );

  expect(retries).toHaveLength(1);
  expect(await backend.calls("update_project_skill_from_center")).toHaveLength(0);
});

test("refresh failure remains separate from a successful mutation", async ({ page, backend }) => {
  await backend.seed({
    projects: [project("p1", "webapp", 0)],
    projectSkills: {
      p1: [
        projectSkill("x", "claude_code", {
          in_center: true,
          center_skill_id: "x",
          sync_status: "center_newer",
        }),
      ],
    },
    projectAgentTargets: { p1: [agentTarget("claude_code", "Claude Code")] },
    holdOperations: true,
  });
  await page.goto("/project/p1");
  await page.getByRole("button", { name: /^Update All/ }).click();
  await expect(page.getByRole("region", { name: "Project update progress" })).toBeVisible();

  const submission = (await backend.calls("operation_request")).find(
    (call) => call.request.action === "submit_project_update",
  );

  if (!submission || submission.request.action !== "submit_project_update")
    throw new Error("Expected project submission");
  await backend.failNext("get_project_skills", "refresh unavailable");
  await backend.patch({
    operations: [
      {
        operation_id: submission.request.operation_id,
        attempt_id: "attempt-1",
        host_id: null,
        project_id: "p1",
        project_path: "/home/e2e/code/webapp",
        revision: 3,
        settled: true,
        cancel_requested: false,
        items: [
          {
            id: "item",
            attempt_id: "attempt-1",
            relative_path: "x",
            agent: "claude_code",
            phase: "succeeded",
            message: null,
          },
        ],
      },
    ],
  });
  await backend.emit("operation-changed", {
    host_id: null,
    operation_id: submission.request.operation_id,
    revision: 3,
  });
  const progress = page.getByRole("region", { name: "Project update progress" });
  await expect(progress).toContainText("1 updated · 0 failed");
  await expect(progress).toContainText("Changes finished, but project refresh failed.");
  await expect(page.getByRole("button", { name: /Retry failed/ })).toHaveCount(0);
});
