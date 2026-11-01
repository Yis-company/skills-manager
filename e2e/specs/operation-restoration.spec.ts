import type { OperationSnapshot } from "../../src/lib/operations";
import { agentTarget, project, projectSkill } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const result = (
  id: string,
  phase: "failed" | "needs_review" | "outcome_unknown" | "succeeded" = "failed",
  host_id: null | string = null,
): OperationSnapshot => ({
  operation_id: id,
  attempt_id: "original",
  host_id,
  project_id: null,
  project_path: null,
  work: "installation",
  destination: { kind: "library", path: "/original/library" },
  sources: [{ kind: "local", path: "/sources/writing" }],
  revision: 5,
  settled: true,
  cancel_requested: false,
  items: [
    {
      id: "writing",
      attempt_id: "original",
      relative_path: "writing",
      agent: null,
      source: { kind: "local", path: "/sources/writing" },
      phase,
      message: null,
    },
    {
      id: "done",
      attempt_id: "original",
      relative_path: "completed sibling",
      agent: null,
      phase: "succeeded",
      message: null,
    },
  ],
});

test("reopening restores a partial result beside its source without success history or replay", async ({
  page,
  backend,
}) => {
  await backend.seed({ operations: [result("partial"), result("success", "succeeded")] });
  await page.goto("/install?tab=local");
  await expect(page.getByRole("region", { name: "Installation progress" })).toHaveCount(2);
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_results_request")).filter(
          (c) => c.request.action === "write",
        ).length,
    )
    .toBeGreaterThan(0);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(sessionStorage.getItem("e2e:fake-backend-state")!).operationResults.results
            .length,
      ),
    )
    .toBe(1);
  await backend.patch({ operations: [] });
  await page.reload();
  await expect
    .poll(async () => (await backend.calls("operation_results_request")).length)
    .toBeGreaterThan(0);
  const restored = page.getByRole("region", { name: "Installation progress" });
  await expect(restored).toHaveCount(1);
  await expect(restored).toContainText("Restored result");
  await expect(restored).toContainText("writing: Update failed");
  await expect(restored).toContainText("completed sibling: Installed");
  expect(
    (await backend.calls("operation_request")).filter(
      (c) => !["read", "list", "reconcile"].includes(c.request.action),
    ),
  ).toEqual([]);
});

test("offline and removed origins retain readable context despite colliding host and project IDs", async ({
  page,
  backend,
}) => {
  const cached = {
    ...result("cached", "outcome_unknown", "build"),
    recovery: "unknown" as const,
    recovery_attempt: "confirm-original",
    project_id: "same",
    project_path: "/original/workspace",
    work: "project_update" as const,
    destination: { kind: "project" as const, id: "same", path: "/original/workspace" },
    origin: {
      host_name: "Build",
      ssh_target: "dev@original",
      host_created_at: 1,
      project_name: "Webapp",
    },
  };

  await backend.seed({
    operationResults: { version: 1, results: [cached] },
    remoteHosts: [
      { id: "build", name: "Build", ssh_target: "dev@replacement", created_at: 2, cli_path: null },
    ],
  });
  await page.goto("/");
  const shortcut = page.getByRole("button", { name: /Build · Webapp.*Outcome unknown/ });
  await expect(shortcut).toBeVisible();
  await shortcut.click();
  const context = page.getByRole("dialog", { name: "Webapp" });
  await expect(context).toContainText("dev@original");
  await expect(context).toContainText("/original/workspace");
  await expect(context).toContainText("writing");
  await expect(context.getByRole("button", { name: "Reconnect and check result" })).toBeDisabled();
  expect(await backend.calls("remote_host_connect")).toEqual([]);
  expect(await backend.calls("remote_invoke")).toEqual([]);
  await page.keyboard.press("Escape");
  await backend.patch({ remoteHosts: [] });
  await page.reload();
  await expect(page.getByRole("button", { name: /Build · Webapp.*Outcome unknown/ })).toBeVisible();
});

test("restored review recreates evidence and waits for fresh selection before installing", async ({
  page,
  backend,
}) => {
  const saved = result("review", "needs_review");
  await backend.seed({ operationResults: { version: 1, results: [saved] } });
  await page.goto("/install?tab=local");
  const region = page.getByRole("region", { name: "Installation progress" });
  await expect(region).toContainText("Restored result");
  await expect(region.getByRole("button", { name: "Review selection" })).toHaveCount(0);
  await backend.hold("operation_request:prepare_review");
  await region.getByRole("button", { name: "Prepare and review again" }).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter(
          (c) => c.request.action === "prepare_review",
        ).length,
    )
    .toBe(1);
  expect(
    (await backend.calls("operation_request")).filter(
      (c) => c.request.action === "submit_install" || c.request.action === "confirm_install",
    ),
  ).toEqual([]);

  const preparation = (await backend.calls("operation_request")).find(
    (c) => c.request.action === "prepare_review",
  )!.request;

  await backend.patch({
    operations: [
      {
        ...saved,
        revision: 6,
        attempt_id: "attempt_id" in preparation ? preparation.attempt_id : "missing",
        preview: [{ rel_path: "fresh-one", name: "fresh writing", description: null }],
      },
    ],
  });
  await backend.release("operation_request:prepare_review");
  await region.getByRole("button", { name: "Review selection" }).click();
  await expect(page.getByRole("dialog").getByRole("textbox")).toHaveValue("fresh writing");
  await page.getByRole("button", { name: "Import Selected" }).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter(
          (c) => c.request.action === "confirm_install",
        ).length,
    )
    .toBe(1);

  const confirmation = (await backend.calls("operation_request")).find(
    (c) => c.request.action === "confirm_install",
  );

  expect(confirmation?.request).toMatchObject({
    action: "confirm_install",
    operation_id: "review",
    items: [{ rel_path: "fresh-one", name: "fresh writing" }],
  });
});

test("save failure retains results in memory and an explicit save retry makes them durable", async ({
  page,
  backend,
}) => {
  await page.goto("/install?tab=local");
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_results_request")).filter(
          (c) => c.request.action === "read",
        ).length,
    )
    .toBeGreaterThan(0);
  await backend.failNext("operation_results_request", "synthetic disk full with SECRET exception");
  await backend.patch({ operations: [result("save-failure")] });
  await backend.emit("operation-changed", { host_id: null });
  await expect(
    page.getByText("Could not save operation results. They remain available in this session."),
  ).toBeVisible();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "writing",
  );
  await expect(page.getByText(/SECRET/)).toHaveCount(0);
  await page.getByRole("button", { name: "Retry saving results" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(sessionStorage.getItem("e2e:fake-backend-state")!).operationResults.results
            .length,
      ),
    )
    .toBe(1);
  await backend.patch({ operations: [] });
  await page.reload();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Restored result",
  );
});

test("closing before admission retains original correlation and reopening never replays the submission", async ({
  page,
  backend,
}) => {
  await backend.seed({
    market: [
      { id: "market", skill_id: "writing", name: "writing", source: "fixture/skills", installs: 1 },
    ],
  });
  await page.goto("/install?tab=market");
  await backend.hold("operation_request:submit_install");
  await page.getByRole("button", { name: "Install", exact: true }).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter(
          (c) => c.request.action === "submit_install",
        ).length,
    )
    .toBe(1);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(sessionStorage.getItem("e2e:fake-backend-state")!).operationResults.results
            .length,
      ),
    )
    .toBe(1);

  const submitted = (await backend.calls("operation_request")).find(
    (c) => c.request.action === "submit_install",
  )!.request;

  await page.reload();
  const region = page.getByRole("region", { name: "Installation progress" });
  await expect(region).toContainText("Outcome unknown");
  await expect(region).toContainText("Restored result");
  await expect(region.getByRole("button", { name: /Retry failed|Cancel/ })).toHaveCount(0);
  expect(
    (await backend.calls("operation_request")).filter(
      (c) => !["list", "read", "reconcile"].includes(c.request.action),
    ),
  ).toEqual([]);
  await region.getByRole("button", { name: "Reconnect and check result" }).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter((c) => c.request.action === "reconcile")
          .length,
    )
    .toBeGreaterThan(0);
  expect(
    (await backend.calls("operation_request")).find((c) => c.request.action === "reconcile")
      ?.request,
  ).toMatchObject({
    operation_id: "operation_id" in submitted ? submitted.operation_id : "missing",
  });
  await expect(region).toContainText("Outcome unknown");
});

test("dismissing a successful summary survives stale polling during this session", async ({
  page,
  backend,
}) => {
  await backend.seed({ operations: [result("successful", "succeeded")] });
  await page.goto("/install?tab=local");
  const region = page.getByRole("region", { name: "Installation progress" });
  await expect(region).toBeVisible();
  await region.getByRole("button", { name: "Dismiss result" }).click();
  await expect(region).toHaveCount(0);
  await backend.emit("operation-changed", { host_id: null });
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter((c) => c.request.action === "list")
          .length,
    )
    .toBeGreaterThan(1);
  await expect(region).toHaveCount(0);
});

for (const [tab, source] of [
  ["market", { kind: "marketplace", source: "fixture/skills", skill_id: "writing" }],
  ["git", { kind: "git", url: "https://fixture.invalid/repo" }],
  ["local", { kind: "local", path: "/sources/folder" }],
  ["local", { kind: "local", path: "/sources/writing.skill" }],
  ["local", { kind: "local_batch", path: "/sources/batch" }],
  ["local", { kind: "local", path: "/sources/discovered", origin: "discovered" }],
] as const) {
  test(`restores ${JSON.stringify(source)} in its original source tab without starting work`, async ({
    page,
    backend,
  }) => {
    const saved = {
      ...result("source", "needs_review"),
      sources: [source],
      items: [{ ...result("source").items[0], source, phase: "needs_review" as const }],
    };

    await backend.seed({ operationResults: { version: 1, results: [saved] } });
    await page.goto(`/install?tab=${tab}`);
    const region = page.getByRole("region", { name: "Installation progress" });
    await expect(region).toContainText("Restored result");
    await expect(region).toContainText("/original/library");
    await expect(region.getByRole("button", { name: "Prepare and review again" })).toBeVisible();
    expect(
      (await backend.calls("operation_request")).filter((c) => c.request.action !== "list"),
    ).toEqual([]);
  });
}

test("restored project detail remains visible without current skill rows and never follows a reused project ID", async ({
  page,
  backend,
}) => {
  const original = project("same", "Webapp", 0, { path: "/original/project" });

  const saved: OperationSnapshot = {
    ...result("project"),
    project_id: "same",
    project_path: original.path,
    work: "project_update",
    destination: { kind: "project", id: "same", path: original.path },
    origin: { host_name: "Local", project_name: "Webapp" },
  };

  await backend.seed({ projects: [original], operationResults: { version: 1, results: [saved] } });
  await page.goto("/project/same");
  const region = page.getByRole("region", { name: "Project update progress" });
  await expect(region).toContainText("writing");
  await expect(region).toContainText("completed sibling");
  await backend.patch({ projects: [{ ...original, path: "/replacement/project" }] });
  await page.reload();
  await expect(region).toHaveCount(0);
  const shortcut = page.getByRole("button", { name: /Local · Webapp.*need attention/ });
  await shortcut.click();
  await expect(page.getByRole("dialog")).toContainText("/original/project");
});

for (const phase of ["outcome_unknown", "queued"] as const) {
  test(`replacement project matrix excludes original ${phase} work and keeps its controls usable`, async ({
    page,
    backend,
  }) => {
    const original: OperationSnapshot = {
      ...result("original-project", "outcome_unknown"),
      project_id: "same",
      project_path: "/original/project",
      work: "project_update",
      destination: { kind: "project", id: "same", path: "/original/project" },
      origin: { host_name: "Local", project_name: "Webapp" },
      settled: phase !== "queued",
      items: [{ ...result("original-project").items[0], agent: "claude_code", phase }],
    };

    await backend.seed({
      projects: [project("same", "Webapp", 0, { path: "/replacement/project" })],
      projectAgentTargets: { same: [agentTarget("claude_code", "Claude Code")] },
      projectSkills: { same: [projectSkill("writing", "claude_code")] },
      ...(phase === "queued"
        ? { operations: [original] }
        : { operationResults: { version: 1, results: [original] } }),
    });
    await page.goto("/project/same");
    const matrix = page.getByRole("region", { name: "On agents", exact: true });
    const control = matrix.getByRole("button", { name: "Remove writing from Claude Code" });

    await expect(control).toBeVisible();
    // Wait for observation/restoration before asserting that the current project excludes it.
    const shortcut = page.getByRole("button", { name: /Local · Webapp/ });

    if (phase === "queued") {
      await page.getByRole("button", { name: "Local", exact: true }).click();
      await expect(page.getByRole("menuitemradio", { name: /Local.*1 in progress/ })).toBeVisible();
      await page.keyboard.press("Escape");
    } else {
      await expect(shortcut).toBeVisible();
    }

    await expect(matrix.getByRole("status")).toHaveCount(0);
    await expect(control).toBeEnabled();
    await expect(page.getByRole("region", { name: "Project update progress" })).toHaveCount(0);
    await expect(page.locator("#sidebar-projects")).not.toContainText(
      phase === "queued" ? "0 of 1" : "Outcome unknown",
    );

    if (phase === "outcome_unknown") {
      await shortcut.click();
      const context = page.getByRole("dialog", { name: "Webapp" });
      await expect(context).toContainText("/original/project");
      await expect(context).toContainText("writing");
      await expect(context).toContainText("Outcome unknown");
    }

    expect(
      (await backend.calls("operation_request")).filter(
        (call) => !["read", "list", "reconcile"].includes(call.request.action),
      ),
    ).toEqual([]);
  });
}

test("reconnection cannot navigate a restored result into a project that reused its ID", async ({
  page,
  backend,
}) => {
  const saved: OperationSnapshot = {
    ...result("original", "failed", "build"),
    project_id: "same",
    project_path: "/original/workspace",
    work: "project_update",
    destination: { kind: "project", id: "same", path: "/original/workspace" },
    origin: {
      host_name: "Build",
      ssh_target: "dev@original",
      host_created_at: 1,
      project_name: "Webapp",
    },
  };

  await backend.seed({
    operationResults: { version: 1, results: [saved] },
    remoteHosts: [
      { id: "build", name: "Build", ssh_target: "dev@original", created_at: 1, cli_path: null },
    ],
    remoteStates: {
      build: {
        operations: [saved],
        projects: [project("same", "Webapp", 0, { path: "/replacement/workspace" })],
      },
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: /Build · Webapp.*need attention/ }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Reconnect and check result" })
    .click();
  await expect
    .poll(
      async () =>
        (await backend.calls("remote_invoke")).filter(
          (c) =>
            c.command === "operation_request" &&
            c.args instanceof Object &&
            "request" in c.args &&
            c.args.request instanceof Object &&
            "action" in c.args.request &&
            c.args.request.action === "reconcile",
        ).length,
    )
    .toBeGreaterThan(0);
  await page.keyboard.press("Escape");
  const shortcut = page.getByRole("button", { name: /Build · workspace.*need attention/ });
  await expect(shortcut).toBeVisible();
  await shortcut.click();
  await expect(page.getByRole("dialog", { name: "Webapp" })).toContainText("/original/workspace");
  await expect(page).not.toHaveURL(/\/project\/same/);
});

test("a save already in flight cannot erase newer failures or revive a resolved success", async ({
  page,
  backend,
}) => {
  await page.goto("/install?tab=local");
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_results_request")).filter(
          (c) => c.request.action === "read",
        ).length,
    )
    .toBeGreaterThan(0);
  await backend.holdResponse("operation_results_request");
  await backend.patch({ operations: [result("old")] });
  await backend.emit("operation-changed", { host_id: null });
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_results_request")).filter(
          (c) => c.request.action === "write",
        ).length,
    )
    .toBe(1);
  await backend.patch({
    operations: [
      { ...result("old", "succeeded"), revision: 6 },
      { ...result("new"), items: [{ ...result("new").items[0], relative_path: "new failure" }] },
    ],
  });
  await backend.emit("operation-changed", { host_id: null });
  await expect(
    page.getByRole("region", { name: "Installation progress" }).filter({ hasText: "new failure" }),
  ).toBeVisible();
  await backend.releaseResponse("operation_results_request");
  await expect
    .poll(() =>
      page.evaluate(() =>
        JSON.parse(sessionStorage.getItem("e2e:fake-backend-state")!).operationResults.results.map(
          (r: { operation_id: string }) => r.operation_id,
        ),
      ),
    )
    .toEqual(["new"]);
  await backend.patch({ operations: [] });
  await page.reload();
  const region = page.getByRole("region", { name: "Installation progress" });
  await expect(region).toHaveCount(1);
  await expect(region).toContainText("new failure");
});

for (const theme of ["light", "dark"]) {
  test(`restored context stays readable and keyboard accessible with enlarged text in ${theme}`, async ({
    page,
    backend,
  }) => {
    const offline: OperationSnapshot = {
      ...result("offline", "outcome_unknown", "removed"),
      recovery: "unknown",
      project_id: "original",
      project_path: "/original/projects/a-long-project-name",
      work: "project_update",
      destination: {
        kind: "project",
        id: "original",
        path: "/original/projects/a-long-project-name",
      },
      origin: {
        host_name: "Original host",
        ssh_target: "dev@original",
        host_created_at: 1,
        project_name: "Webapp",
      },
    };

    await backend.seed({
      settings: { theme, text_size: "xlarge" },
      operationResults: { version: 1, results: [result("local"), offline] },
    });
    await page.setViewportSize({ width: 1000, height: 900 });
    await page.goto("/install?tab=local");
    await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
      "Restored result",
    );
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
      .toBe(true);
    await page.screenshot({
      path: `/private/tmp/ski-24-restored-${theme}.png`,
      animations: "disabled",
    });
    const shortcut = page.getByRole("button", { name: /Original host · Webapp.*Outcome unknown/ });
    await shortcut.focus();
    await page.keyboard.press("Enter");
    const dialog = page.getByRole("dialog", { name: "Webapp" });
    await expect(dialog).toContainText("/original/projects/a-long-project-name");
    await expect(dialog.getByRole("button", { name: "Reconnect and check result" })).toBeDisabled();
    await page.screenshot({
      path: `/private/tmp/ski-24-offline-${theme}.png`,
      animations: "disabled",
    });
    await page.keyboard.press("Escape");
    await expect(shortcut).toBeFocused();
  });
}

test("a failed initial result read keeps current operations observable and can be retried", async ({
  page,
  backend,
}) => {
  await backend.seed({
    operationResultsReadError: true,
    operations: [result("current")],
    operationResults: { version: 1, results: [result("cached")] },
  });
  await page.goto("/install?tab=local");
  await expect(page.getByRole("alert")).toContainText("Could not save operation results");
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "writing",
  );
  await page.getByRole("button", { name: "Retry saving results" }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toHaveCount(2);
  await expect(page.getByRole("alert")).toHaveCount(0);
});
