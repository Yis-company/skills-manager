import type { OperationSnapshot } from "../../src/lib/operations";
import { skill, tool } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const operation = (
  id = "running",
  phase: OperationSnapshot["items"][number]["phase"] = "applying",
): OperationSnapshot => ({
  operation_id: id,
  attempt_id: "first",
  host_id: null,
  project_id: null,
  project_path: null,
  work: "installation",
  destination: { kind: "library", path: "/library" },
  sources: [{ kind: "local", path: `/sources/${id}` }],
  revision: 1,
  settled: ["failed", "needs_review", "outcome_unknown", "succeeded", "canceled"].includes(phase),
  cancel_requested: false,
  items: [{ id, attempt_id: "first", relative_path: id, agent: null, phase, message: null }],
});

test("real quit offers three choices and Keep app open never cancels work", async ({
  page,
  backend,
}) => {
  await backend.seed({ operations: [operation()] });
  await page.goto("/install?tab=local");
  await expect(page.getByRole("region", { name: "Installation progress" })).toBeVisible();
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  const dialog = page.getByRole("dialog", { name: "Quit with unfinished work?" });
  await expect(dialog).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "Cancel unfinished work and quit", exact: true }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "Quit automatically when all work finishes", exact: true }),
  ).toBeVisible();
  await dialog.getByRole("button", { name: "Keep app open", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    (await backend.calls("operation_request")).filter((c) => c.request.action === "cancel"),
  ).toEqual([]);
});

test("automatic quit includes a later pending admission and saves failed review and unknown results before exit", async ({
  page,
  backend,
}) => {
  await backend.seed({
    operations: [operation()],
    holdOperations: true,
    market: [
      { id: "extra", name: "extra", skill_id: "extra", source: "fixture/skills", installs: 10 },
    ],
  });
  await page.goto("/install");
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  await page
    .getByRole("button", { name: "Quit automatically when all work finishes", exact: true })
    .click();
  await expect(page.getByRole("status", { name: "Pending quit" })).toBeVisible();
  await backend.holdResponse("operation_request:submit_install");
  await page.getByRole("button", { name: "Install", exact: true }).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter(
          (c) => c.request.action === "submit_install",
        ).length,
    )
    .toBe(1);

  const call = (await backend.calls("operation_request")).find(
    (c) => c.request.action === "submit_install",
  )!;

  const id = "operation_id" in call.request ? call.request.operation_id : "";
  await backend.patch({
    operations: [{ ...operation("running", "succeeded"), revision: 2 }, operation(id, "queued")],
  });
  await backend.emit("operation-changed", { host_id: null });
  await expect(page.getByRole("status", { name: "Pending quit" })).toContainText("1 unfinished");
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
  await backend.releaseResponse("operation_request:submit_install");
  await backend.holdResponse("operation_results_request");
  await backend.patch({
    operations: [
      { ...operation("running", "succeeded"), revision: 2 },
      { ...operation(id, "failed"), revision: 2 },
      operation("review", "needs_review"),
      operation("unknown", "outcome_unknown"),
    ],
  });
  await backend.emit("operation-changed", { host_id: null });
  await expect(page.getByText("Saving results before closing…", { exact: true })).toBeVisible();
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
  await backend.releaseResponse("operation_results_request");
  await expect
    .poll(
      async () =>
        (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish")
          .length,
    )
    .toBe(1);

  const saved = await page.evaluate(
    () => JSON.parse(sessionStorage.getItem("e2e:fake-backend-state")!).operationResults.results,
  );

  expect(saved.map((o: OperationSnapshot) => o.items[0].phase).sort()).toEqual([
    "failed",
    "needs_review",
    "outcome_unknown",
  ]);
});

test("cancel and quit waits for safe stopping and retains successes", async ({ page, backend }) => {
  const active = operation();
  active.items.push({ ...operation("completed", "succeeded").items[0] });
  await backend.seed({ operations: [active], shutdownReady: false });
  await page.goto("/install?tab=local");
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  await page.getByRole("button", { name: "Cancel unfinished work and quit", exact: true }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Stopping safely",
  );
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);

  const canceled = {
    ...active,
    revision: 3,
    settled: true,
    cancel_requested: true,
    items: [
      { ...active.items[0], phase: "canceled" as const },
      active.items[1],
      operation("review", "needs_review").items[0],
    ],
  };

  await backend.patch({ operations: [canceled], shutdownReady: true });
  await backend.emit("operation-changed", { host_id: null });
  await expect
    .poll(
      async () =>
        (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish")
          .length,
    )
    .toBe(1);

  const saved = await page.evaluate(
    () => JSON.parse(sessionStorage.getItem("e2e:fake-backend-state")!).operationResults.results,
  );

  expect(saved[0].items.map((i: OperationSnapshot["items"][number]) => i.phase)).toEqual([
    "canceled",
    "succeeded",
    "needs_review",
  ]);
});

test("settled unresolved outcomes do not prompt; a save failure keeps restart recoverable", async ({
  page,
  backend,
}) => {
  await backend.seed({
    operations: [
      operation("failed", "failed"),
      operation("review", "needs_review"),
      operation("unknown", "outcome_unknown"),
    ],
    operationResultsWriteError: true,
    shutdown: { id: 1, target: "restart", mode: null },
  });
  await page.goto("/install?tab=local");
  await expect(
    page.getByRole("alert").filter({ hasText: "Could not save results. The app is still open." }),
  ).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Quit with unfinished work?" })).toHaveCount(0);
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
  await backend.patch({ operationResultsWriteError: false });
  await page.getByRole("button", { name: "Retry saving and restart", exact: true }).click();
  await expect
    .poll(
      async () =>
        (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish")
          .length,
    )
    .toBe(1);

  const writes = (await backend.calls("operation_results_request")).filter(
    (c) => c.request.action === "write",
  );

  expect(writes.length).toBeGreaterThan(1);
});

test.describe("Windows update exit adapter", () => {
  test.use({
    userAgent:
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/140.0 Safari/537.36",
  });
  test("the installer requests coordinated shutdown before invoking its process-exit installer", async ({
    page,
    backend,
  }) => {
    await backend.seed({
      operations: [operation()],
      appUpdate: {
        has_update: true,
        current_version: "1.40.0",
        latest_version: "1.50.0",
        release_url: "https://example.invalid/release",
      },
    });
    await page.goto("/settings/about");
    await page.getByRole("button", { name: "Install Update", exact: true }).click();
    await expect.poll(async () => (await backend.calls("install_app_update")).length).toBe(1);
    await expect(page.getByRole("dialog", { name: "Quit with unfinished work?" })).toBeVisible();
    await page.getByRole("button", { name: "Keep app open", exact: true }).click();
    expect(
      (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
    ).toHaveLength(0);
  });
});

test("Keep app open during final saving revokes exit and protects keyboard focus", async ({
  page,
  backend,
}) => {
  await backend.seed({ operations: [operation()] });
  await page.goto("/install?tab=local");
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  await page
    .getByRole("button", { name: "Quit automatically when all work finishes", exact: true })
    .click();
  await backend.holdResponse("operation_results_request");
  await backend.patch({ operations: [{ ...operation("running", "needs_review"), revision: 2 }] });
  await backend.emit("operation-changed", { host_id: null });
  const saving = page.getByRole("dialog", { name: "Saving results before closing…" });
  await expect(saving).toBeVisible();
  await saving.getByRole("button", { name: "Keep app open", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(saving).toHaveCount(0);
  await backend.releaseResponse("operation_results_request");
  await expect
    .poll(
      async () =>
        (await backend.calls("shutdown_request")).filter((c) => c.request.action === "keep_open")
          .length,
    )
    .toBe(1);
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
});

test("close to tray keeps work running and native restart preserves staged workspace checks", async ({
  page,
  backend,
}) => {
  await backend.seed({
    operations: [operation()],
    settings: { close_action: "hide" },
    tools: [tool("claude_code", "Claude Code"), tool("codex", "Codex")],
    skills: [skill("review", "code-review")],
  });
  await page.goto("/my-skills");
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  await backend.emit("window-close-requested", null);
  await expect.poll(async () => (await backend.calls("hide_to_tray")).length).toBe(1);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await backend.emit("shutdown-requested", { id: 1, target: "restart", mode: null });
  const leave = page.getByRole("dialog", { name: "Leave staged changes?" });
  await expect(leave).toBeVisible();
  await leave.getByRole("button", { name: "Keep editing", exact: true }).click();
  await expect(page.getByRole("button", { name: "Review changes", exact: true })).toBeVisible();
  expect(
    (await backend.calls("shutdown_request")).filter((c) =>
      ["choose", "finish"].includes(c.request.action),
    ),
  ).toHaveLength(0);
});

const buildHost = {
  id: "build",
  name: "Build",
  ssh_target: "dev@build",
  created_at: 1,
  cli_path: null,
};

test("auto-quit lets original-host recovery finish without changing the viewed host", async ({
  page,
  backend,
}) => {
  const remote = { ...operation(), host_id: "build" };
  await backend.seed({
    remoteHosts: [buildHost],
    retainedHosts: ["build"],
    remoteStates: { build: { operations: [remote] } },
  });
  await page.goto("/install?tab=local");
  await expect(page.getByRole("button", { name: /Build · Install Skills/ })).toBeVisible();
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  await page
    .getByRole("button", { name: "Quit automatically when all work finishes", exact: true })
    .click();
  await backend.hold("remote_host_connect");
  await backend.emit("remote-session-state", { host_id: "build", state: "disconnected" });
  await expect(
    page.getByRole("button", { name: /Build · Install Skills.*checking result/ }),
  ).toBeVisible();
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
  await backend.patch({
    remoteStates: {
      build: {
        operations: [
          {
            ...remote,
            revision: 2,
            settled: true,
            items: [{ ...remote.items[0], phase: "succeeded" }],
          },
        ],
      },
    },
  });
  await backend.release("remote_host_connect");
  await expect
    .poll(
      async () =>
        (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish")
          .length,
    )
    .toBe(1);
  const connects = await backend.calls("remote_host_connect");
  expect(connects.every((c) => c.hostId === "build" && c.select === false)).toBe(true);
  // SAFETY: operation_request is recorded with its public request envelope.
  expect(
    (await backend.calls("remote_invoke")).filter(
      (c) =>
        c.command === "operation_request" &&
        !["list", "read", "reconcile"].includes(
          (c.args as { request: { action: string } }).request.action,
        ),
    ),
  ).toEqual([]);
});

test("bounded checking becomes saved unknown and permits automatic quit without cancellation or replay", async ({
  page,
  backend,
}) => {
  const remote = { ...operation(), host_id: "build" };
  await backend.seed({
    remoteHosts: [buildHost],
    retainedHosts: ["build"],
    remoteStates: { build: { operations: [remote] } },
  });
  await page.goto("/install?tab=local");
  await expect(page.getByRole("button", { name: /Build · Install Skills/ })).toBeVisible();
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  await page
    .getByRole("button", { name: "Quit automatically when all work finishes", exact: true })
    .click();
  await page.clock.install();
  await backend.hold("remote_host_connect");
  await backend.emit("remote-session-state", { host_id: "build", state: "disconnected" });
  await expect(
    page.getByRole("button", { name: /Build · Install Skills.*checking result/ }),
  ).toBeVisible();
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
  await backend.patch({ remoteStates: { build: { operations: [] } } });
  await page.clock.fastForward(12500);
  await expect(
    page.getByRole("button", { name: /Build · Install Skills.*Outcome unknown/ }),
  ).toBeVisible();
  await page.clock.fastForward(500);
  await expect
    .poll(
      async () =>
        (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish")
          .length,
    )
    .toBe(1);

  const saved = await page.evaluate(
    () => JSON.parse(sessionStorage.getItem("e2e:fake-backend-state")!).operationResults.results,
  );

  expect(saved[0].host_id).toBe("build");
  expect(saved[0].items[0].phase).toBe("outcome_unknown");
});

for (const theme of ["light", "dark"])
  test(`quit choices remain readable and keyboard accessible in ${theme}`, async ({
    page,
    backend,
  }) => {
    await backend.seed({ operations: [operation()], settings: { theme, text_size: "xlarge" } });
    await page.setViewportSize({ width: 1000, height: 900 });
    await page.goto("/install?tab=local");
    await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
    const dialog = page.getByRole("dialog", { name: "Quit with unfinished work?" });
    await expect(dialog).toBeVisible();
    expect(await dialog.evaluate((e) => e.scrollWidth <= e.clientWidth)).toBe(true);
    await page.screenshot({
      path: `/private/tmp/ski-25-choices-${theme}.png`,
      animations: "disabled",
    });
    await dialog
      .getByRole("button", { name: "Quit automatically when all work finishes", exact: true })
      .focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("status", { name: "Pending quit" })).toBeVisible();
    await page.screenshot({
      path: `/private/tmp/ski-25-pending-${theme}.png`,
      animations: "disabled",
    });
    await page.getByRole("button", { name: "Keep app open", exact: true }).focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("status", { name: "Pending quit" })).toHaveCount(0);
  });

test("pending intent before its RPC closes the automatic-quit admission gap", async ({
  page,
  backend,
}) => {
  await backend.seed({
    operations: [operation()],
    holdOperations: true,
    market: [
      { id: "extra", name: "extra", skill_id: "extra", source: "fixture/skills", installs: 10 },
    ],
  });
  await page.goto("/install");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          JSON.parse(sessionStorage.getItem("e2e:fake-backend-state") ?? "null")?.operationResults
            .results.length ?? 0,
      ),
    )
    .toBe(1);
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  await page
    .getByRole("button", { name: "Quit automatically when all work finishes", exact: true })
    .click();
  await backend.holdResponse("operation_results_request");
  await page.getByRole("button", { name: "Install", exact: true }).click();
  await expect
    .poll(async () =>
      (await backend.calls("operation_results_request")).some(
        (c) => c.request.action === "write" && c.request.results.some((o) => o.admission),
      ),
    )
    .toBe(true);
  await backend.patch({ operations: [{ ...operation("running", "succeeded"), revision: 2 }] });
  await backend.emit("operation-changed", { host_id: null });
  await expect(page.getByRole("status", { name: "Pending quit" })).toContainText("1 unfinished");
  expect(
    (await backend.calls("operation_request")).filter((c) => c.request.action === "submit_install"),
  ).toHaveLength(0);
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
  await backend.releaseResponse("operation_results_request");
  await expect
    .poll(
      async () =>
        (await backend.calls("operation_request")).filter(
          (c) => c.request.action === "submit_install",
        ).length,
    )
    .toBe(1);
  await page.getByRole("button", { name: "Keep app open", exact: true }).click();
});

test("staged work added while draining is checked again before final exit", async ({
  page,
  backend,
}) => {
  await backend.seed({
    operations: [operation()],
    tools: [tool("claude_code", "Claude Code"), tool("codex", "Codex")],
    skills: [skill("review", "code-review")],
  });
  await page.goto("/my-skills");
  await backend.emit("shutdown-requested", { id: 1, target: "quit", mode: null });
  await page
    .getByRole("button", { name: "Quit automatically when all work finishes", exact: true })
    .click();
  await page.getByRole("button", { name: "Add code-review to Codex", exact: true }).click();
  await backend.patch({ operations: [{ ...operation("running", "succeeded"), revision: 2 }] });
  await backend.emit("operation-changed", { host_id: null });
  const leave = page.getByRole("dialog", { name: "Leave staged changes?" });
  await expect(leave).toBeVisible();
  await leave.getByRole("button", { name: "Keep editing", exact: true }).click();
  await expect(page.getByRole("button", { name: "Review changes", exact: true })).toBeVisible();
  await expect(page.getByRole("status", { name: "Pending quit" })).toHaveCount(0);
  expect(
    (await backend.calls("shutdown_request")).filter((c) => c.request.action === "finish"),
  ).toHaveLength(0);
});
