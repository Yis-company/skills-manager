import type { OperationSnapshot } from "../../src/lib/operations";
import { expect, test } from "../fixtures";

const localOperation: OperationSnapshot = {
  operation_id: "local-install",
  attempt_id: "first",
  host_id: null,
  project_id: null,
  project_path: null,
  destination: { kind: "library", path: "/library" },
  revision: 1,
  settled: false,
  cancel_requested: false,
  items: [
    {
      id: "folder",
      attempt_id: "first",
      relative_path: "/sources/writing",
      agent: "library",
      source: { kind: "local", path: "/sources/writing" },
      phase: "waiting",
      message: "Waiting for another change to the library",
    },
  ],
};

test("library installation feedback survives navigation and cancellation keeps focus", async ({
  page,
  backend,
}) => {
  await backend.seed({ operations: [localOperation] });
  await page.goto("/install?tab=local");
  const progress = page.getByRole("region", { name: "Installation progress" });
  await expect(progress).toContainText("Waiting for another change to the library");
  await expect(progress).toContainText("Local library");
  await page.getByRole("link", { name: "Library", exact: true }).click();
  await expect(page.getByRole("link", { name: /Install Skills.*1 in progress/ })).toBeVisible();
  await page.getByRole("link", { name: /Install Skills/ }).click();
  await page.getByRole("button", { name: /Local Install/ }).click();
  await progress.getByRole("button", { name: "Cancel installation" }).focus();
  await page.keyboard.press("Enter");
  await expect(progress).toContainText("Canceled");
  await expect(progress).toBeFocused();
});

test("marketplace submits independent selections to the shared service without a frontend pump", async ({
  page,
  backend,
}) => {
  await backend.seed({
    holdOperations: true,
    market: [
      { id: "one", name: "one", skill_id: "one", source: "fixture/skills", installs: 10 },
      { id: "two", name: "two", skill_id: "two", source: "fixture/skills", installs: 9 },
    ],
  });
  await page.goto("/install");
  await page.getByRole("button", { name: "Install", exact: true }).nth(0).click();
  await page.getByRole("button", { name: "Install", exact: true }).nth(0).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter(
          (a) => a.request.action === "submit_install",
        ).length,
    )
    .toBe(2);
  expect(await backend.calls("install_from_skillssh")).toEqual([]);
  await expect(page.getByRole("region", { name: "Installation progress" })).toHaveCount(2);
});

test("folder and archive controls admit separate library operations", async ({ page, backend }) => {
  await backend.seed({
    holdOperations: true,
    dialogPaths: ["/sources/writing", "/sources/research.skill"],
  });
  await page.goto("/install?tab=local");
  await page.getByRole("button", { name: "Select folder", exact: true }).click();
  await page.getByRole("button", { name: "Select archive or file", exact: true }).click();
  await expect
    .poll(async () =>
      (await backend.calls("operation_request"))
        .filter((a) => a.request.action === "submit_install")
        .map((a) => a.request),
    )
    .toEqual([
      {
        action: "submit_install",
        operation_id: expect.any(String),
        host_id: null,
        sources: [{ kind: "local", path: "/sources/writing" }],
      },
      {
        action: "submit_install",
        operation_id: expect.any(String),
        host_id: null,
        sources: [{ kind: "local", path: "/sources/research.skill" }],
      },
    ]);

  await expect(page.getByRole("region", { name: "Installation progress" })).toHaveCount(2);
});

test("Git selection is an explicit operation confirmation after returning to its source", async ({
  page,
  backend,
}) => {
  const operation: OperationSnapshot = {
    ...localOperation,
    operation_id: "git-source",
    settled: true,
    preview: [
      { rel_path: "skills/one", name: "one", description: null },
      { rel_path: "skills/two", name: "two", description: null },
    ],
    items: [
      {
        ...localOperation.items[0],
        relative_path: "https://fixture.invalid/skills.git",
        source: { kind: "git", url: "https://fixture.invalid/skills.git" },
        phase: "needs_review",
        message: "Select skills to install",
      },
    ],
  };

  await backend.seed({ holdOperations: true, operations: [operation] });
  await page.goto("/install?tab=git");
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Ready to select skills",
  );
  await expect(page.getByRole("region", { name: "Installation progress" })).not.toContainText(
    "your copy was kept",
  );
  await page.getByRole("button", { name: "Review selection" }).click();
  await expect(page.getByPlaceholder("Skill name").first()).toHaveValue("one");
  await page.getByRole("checkbox").nth(1).uncheck();
  await page.getByRole("button", { name: "Import Selected", exact: true }).click();
  await expect
    .poll(async () =>
      (await backend.calls("operation_request"))
        .filter((a) => a.request.action === "confirm_install")
        .map((a) => a.request),
    )
    .toEqual([
      {
        action: "confirm_install",
        operation_id: "git-source",
        attempt_id: expect.any(String),
        items: [{ rel_path: "skills/one", name: "one" }],
      },
    ]);

  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText("Queued");
  await expect(page.getByRole("region", { name: "Installation progress" })).toBeFocused();
});

test("successful installation keeps its outcome when the original library refresh fails", async ({
  page,
  backend,
}) => {
  await backend.seed({ operations: [localOperation] });
  await page.goto("/install?tab=local");
  const progress = page.getByRole("region", { name: "Installation progress" });
  await expect(progress).toContainText("Waiting");
  await backend.failNext("get_managed_skills", "library refresh unavailable");
  await backend.patch({
    operations: [
      {
        ...localOperation,
        settled: true,
        revision: 4,
        items: [{ ...localOperation.items[0], phase: "succeeded", message: null }],
      },
    ],
  });
  await expect(progress).toContainText("Installed");
  await expect(progress).toContainText("The library could not be refreshed");
  await expect(progress.getByRole("button", { name: /Retry failed/ })).toHaveCount(0);
  await progress.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(progress).not.toContainText("The library could not be refreshed");
  expect(
    (await backend.calls("operation_request")).filter(
      (call) => call.request.action === "retry_failed",
    ),
  ).toEqual([]);
});
