import type { InvokeArgs } from "@tauri-apps/api/core";

import type { InstallSource, OperationSnapshot } from "../../src/lib/operations";
import { agentTarget, project, projectSkill } from "../fake-backend/state";
import { expect, test } from "../fixtures";

function requestAction(args: InvokeArgs | undefined) {
  return args instanceof Object &&
    "request" in args &&
    args.request instanceof Object &&
    "action" in args.request
    ? args.request.action
    : undefined;
}

const hosts = ["build", "gpu"].map((id) => ({
  id,
  name: `${id} host`,
  ssh_target: `dev@${id}`,
  cli_path: null,
  created_at: 1,
}));

const installation = (host_id: null | string): OperationSnapshot => ({
  operation_id: "colliding",
  attempt_id: "first",
  host_id,
  project_id: null,
  project_path: null,
  work: "installation",
  destination: { kind: "library", path: "/library" },
  revision: 1,
  settled: false,
  cancel_requested: false,
  sources: [{ kind: "local", path: "/sources/demo" }],
  items: [
    {
      id: "item",
      attempt_id: "first",
      agent: null,
      relative_path: "demo",
      source: { kind: "local", path: "/sources/demo" },
      phase: "queued",
      message: null,
    },
  ],
});

test("switching hosts keeps unfinished installations without asking to cancel", async ({
  page,
  backend,
}) => {
  await backend.seed({ remoteHosts: hosts, operations: [installation(null)] });
  await page.goto("/install?tab=local");
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Local library",
  );
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("button", { name: "build host", exact: true })).toBeVisible();
  expect(
    (await backend.calls("operation_request")).filter((c) => c.request.action === "cancel"),
  ).toEqual([]);
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /^Local/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText("Queued");
});

test("off-screen changes reveal their original host and library despite colliding identities", async ({
  page,
  backend,
}) => {
  const local = installation(null);
  const build = installation("build");
  await backend.seed({
    remoteHosts: hosts,
    operations: [local],
    remoteStates: { build: { operations: [build] } },
  });
  await page.goto("/install?tab=local");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "build host library",
  );
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /gpu host/ }).click();
  await expect(page.getByRole("button", { name: "gpu host", exact: true })).toBeVisible();
  await backend.patch({
    operations: [
      {
        ...local,
        revision: 5,
        settled: true,
        items: [{ ...local.items[0], phase: "failed", message: "Local source unavailable" }],
      },
    ],
  });
  await backend.emit("operation-changed", {
    host_id: null,
    operation_id: "colliding",
    revision: 5,
  });
  const shortcut = page.getByRole("button", { name: /Local · Install Skills.*1 need attention/ });
  await expect(shortcut).toBeVisible();
  await expect(
    page.getByRole("button", { name: /build host · Install Skills.*1 in progress/ }),
  ).toBeVisible();
  await shortcut.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Local source unavailable",
  );
  await page.getByRole("button", { name: "Retry failed (1)" }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Installed",
  );
  await expect(
    page.getByRole("button", { name: /build host · Install Skills.*1 in progress/ }),
  ).toBeVisible();
});

test("Git confirmation releases the view while its admission reply stays on the originating host", async ({
  page,
  backend,
}) => {
  const preview: OperationSnapshot = {
    ...installation("build"),
    settled: true,
    sources: [{ kind: "git", url: "https://fixture.invalid/repo" }],
    preview: [
      { rel_path: "one", name: "one", description: null },
      { rel_path: "two", name: "two", description: null },
    ],
    items: [
      {
        ...installation("build").items[0],
        source: { kind: "git", url: "https://fixture.invalid/repo" },
        phase: "needs_review",
      },
    ],
  };

  await backend.seed({
    remoteHosts: hosts,
    remoteStates: { build: { operations: [preview], holdOperations: false } },
  });
  await page.goto("/install?tab=git");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await page.getByRole("button", { name: "Review selection" }).click();
  await page.getByRole("checkbox").nth(1).uncheck();
  await page.getByPlaceholder("Skill name").first().fill("chosen-name");
  await backend.holdResponse("build:operation_request:confirm_install");
  await page.getByRole("button", { name: "Import Selected", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /gpu host/ }).click();
  await expect(page.getByRole("button", { name: "gpu host", exact: true })).toBeVisible();
  await backend.releaseResponse("build:operation_request:confirm_install");
  await expect.poll(() => backend.calls("remote_host_release")).toContainEqual({ hostId: "build" });
  await page.getByRole("button", { name: "gpu host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "one: Installed",
  );

  const confirms = (await backend.calls("remote_invoke")).filter(
    (c) => c.command === "operation_request" && requestAction(c.args) === "confirm_install",
  );

  expect(confirms).toEqual([
    {
      hostId: "build",
      command: "operation_request",
      args: {
        request: {
          action: "confirm_install",
          operation_id: "colliding",
          attempt_id: expect.any(String),
          items: [{ rel_path: "one", name: "chosen-name" }],
        },
      },
    },
  ]);
});

test("a Git review stays attached to its host when another host has the same operation ID", async ({
  page,
  backend,
}) => {
  const preview = (host: string): OperationSnapshot => ({
    ...installation(host),
    settled: true,
    sources: [{ kind: "git", url: `https://fixture.invalid/${host}` }],
    preview: [{ rel_path: host, name: host, description: null }],
    items: [
      {
        ...installation(host).items[0],
        source: { kind: "git", url: `https://fixture.invalid/${host}` },
        phase: "needs_review",
      },
    ],
  });

  await backend.seed({
    remoteHosts: hosts,
    remoteStates: {
      build: { operations: [preview("build")] },
      gpu: { operations: [preview("gpu")] },
    },
  });
  await page.goto("/install?tab=git");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await page.getByRole("button", { name: "Review selection" }).click();
  await page.getByPlaceholder("Skill name").fill("chosen-build");
  // A context change can also arrive while a modal is open. Dispatch the public
  // host controls without making the test depend on the modal's pointer overlay.
  await page.getByRole("button", { name: "build host", exact: true }).dispatchEvent("click");
  await page.getByRole("menuitemradio", { name: /gpu host/ }).dispatchEvent("click");
  await expect(page.getByRole("button", { name: "gpu host", exact: true })).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "gpu host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await page.getByRole("button", { name: "Review selection" }).click();
  await expect(page.getByPlaceholder("Skill name")).toHaveValue("build");
  await page.getByPlaceholder("Skill name").fill("chosen-build");
  await page.getByRole("button", { name: "Import Selected", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);

  const confirms = (await backend.calls("remote_invoke")).filter(
    (c) => c.command === "operation_request" && requestAction(c.args) === "confirm_install",
  );

  expect(confirms).toEqual([
    {
      hostId: "build",
      command: "operation_request",
      args: {
        request: {
          action: "confirm_install",
          operation_id: "colliding",
          attempt_id: expect.any(String),
          items: [{ rel_path: "build", name: "chosen-build" }],
        },
      },
    },
  ]);
});

test("a disconnected off-screen host leaves another host's controls usable", async ({
  page,
  backend,
}) => {
  await backend.seed({
    remoteHosts: hosts,
    remoteStates: {
      build: { operations: [installation("build")] },
      gpu: { operations: [installation("gpu")] },
    },
  });
  await page.goto("/install?tab=local");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toBeVisible();
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /gpu host/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "gpu host library",
  );
  await backend.failNext("build:operation_request:list", "Connection to build host was lost");
  await backend.emit("operation-changed", { host_id: "build" });
  await expect(
    page.getByRole("button", { name: /build host · Install Skills.*Connection unavailable/ }),
  ).toBeVisible();
  await backend.hold("build:operation_request:list");
  await page.getByRole("button", { name: "Cancel installation" }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Canceled",
  );
  await page.getByRole("button", { name: "gpu host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /^Local/ }).click();
  await expect(page.getByRole("button", { name: "Local", exact: true })).toBeVisible();
  await backend.release("build:operation_request:list");
});

test("a remounted observer discovers existing app-session work without switching views", async ({
  page,
  backend,
}) => {
  await backend.seed({
    remoteHosts: hosts,
    retainedHosts: ["build"],
    remoteStates: { build: { operations: [installation("build")] } },
  });
  await page.goto("/my-skills");
  await expect(page.getByRole("button", { name: "Local", exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: /build host · Install Skills.*1 in progress/ }),
  ).toBeVisible();
  expect(await backend.calls("remote_host_connect")).toEqual([]);
});

test("project updates and every installation source stay contextual on Local and two hosts", async ({
  page,
  backend,
}) => {
  const sources: InstallSource[] = [
    { kind: "marketplace", source: "fixture/skills", skill_id: "demo" },
    { kind: "git", url: "https://fixture.invalid/repo" },
    { kind: "local", path: "/sources/folder" },
    { kind: "local", path: "/sources/archive.skill" },
    { kind: "local_batch", path: "/sources/batch" },
    { kind: "local", path: "/sources/discovered", origin: "discovered" },
  ];

  const seed = (host: null | string) => ({
    holdOperations: true,
    projects: [project("shared-project", `${host ?? "Local"} app`, 0)],
    projectSkills: {
      "shared-project": [
        projectSkill("demo", "claude_code", {
          in_center: true,
          center_skill_id: "demo",
          sync_status: "center_newer",
        }),
      ],
    },
    projectAgentTargets: { "shared-project": [agentTarget("claude_code", "Claude Code")] },
    operations: sources.map((source, i) => ({
      ...installation(host),
      operation_id: `source-${i}`,
      sources: [source],
      items: [
        {
          ...installation(host).items[0],
          source,
          relative_path: `source-${i}`,
          phase: source.kind === "git" ? ("preparing" as const) : ("queued" as const),
        },
      ],
    })),
  });

  await backend.seed({
    ...seed(null),
    remoteHosts: hosts,
    remoteStates: { build: seed("build"), gpu: seed("gpu") },
  });
  await page.goto("/project/shared-project");

  for (const [index, label] of ["Local", "build host", "gpu host"].entries()) {
    if (index > 0) {
      await page
        .getByRole("button", { name: index === 1 ? "Local" : "build host", exact: true })
        .click();
      await page.getByRole("menuitemradio", { name: new RegExp(label) }).click();
    }

    await expect(
      page.getByRole("heading", {
        name: `${index === 0 ? "Local" : index === 1 ? "build" : "gpu"} app`,
        exact: true,
      }),
    ).toBeVisible();

    if (index === 0) await backend.holdResponse("operation_request:submit_project_update");
    await page.getByRole("button", { name: /^Update All/ }).click();
    await expect(page.getByRole("region", { name: "Project update progress" })).toContainText(
      "0 of 1",
    );
    await page.getByRole("link", { name: /Install Skills.*6 in progress/ }).click();
    await page.getByRole("button", { name: /Git Install/ }).click();
    await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
      `${label} library`,
    );
    await page.getByRole("button", { name: /Local Install/ }).click();
    await expect(page.getByRole("region", { name: "Installation progress" })).toHaveCount(4);
    await page
      .getByRole("button", {
        name: new RegExp(`${index === 0 ? "Local" : index === 1 ? "build" : "gpu"} app`),
      })
      .first()
      .click();
  }

  await backend.releaseResponse("operation_request:submit_project_update");
  await page.getByRole("button", { name: "Cancel update", exact: true }).click();
  await expect(page.getByRole("region", { name: "Project update progress" })).toContainText(
    "1 canceled",
  );
  // Use the host menu to preserve the project route while changing only its owner.
  await page.getByRole("button", { name: "gpu host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("button", { name: "Cancel update", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /^Local/ }).click();
  await expect(page.getByRole("button", { name: "Cancel update", exact: true })).toBeVisible();

  const submissions = (await backend.calls("operation_request")).filter(
    (c) => c.request.action === "submit_project_update",
  );

  expect(submissions).toHaveLength(1);
  expect(
    (await backend.calls("remote_invoke")).filter(
      (c) => c.command === "operation_request" && requestAction(c.args) === "submit_project_update",
    ),
  ).toHaveLength(2);
});

test("late retry replies and duplicate events cannot replace a newer host-owned attempt", async ({
  page,
  backend,
}) => {
  const local = {
    ...installation(null),
    revision: 2,
    settled: true,
    items: [{ ...installation(null).items[0], phase: "failed" as const }],
  };

  await backend.seed({
    remoteHosts: hosts,
    operations: [local],
    remoteStates: { build: { operations: [installation("build")] } },
  });
  await page.goto("/install?tab=local");
  await backend.holdResponse("operation_request:retry_failed");
  await page.getByRole("button", { name: "Retry failed (1)" }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Installed",
  );
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();

  const latest: OperationSnapshot = {
    ...local,
    revision: 9,
    attempt_id: "latest",
    items: [
      {
        ...local.items[0],
        attempt_id: "latest",
        phase: "succeeded",
        message: "New authoritative result",
      },
    ],
  };

  await backend.patch({
    operations: [
      latest,
      {
        ...local,
        host_id: "gpu",
        revision: 99,
        items: [{ ...local.items[0], message: "Wrong host" }],
      },
    ],
  });
  await backend.emit("operation-changed", {
    host_id: null,
    operation_id: "colliding",
    revision: 9,
  });
  await backend.emit("operation-changed", {
    host_id: null,
    operation_id: "colliding",
    revision: 1,
  });
  await backend.releaseResponse("operation_request:retry_failed");
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /^Local/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "New authoritative result",
  );
  await expect(page.getByText("Wrong host")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: /build host · Install Skills.*1 in progress/ }),
  ).toBeVisible();
});

test("refresh failures and settlement refreshes are scoped by host even with the same operation and attempt", async ({
  page,
  backend,
}) => {
  const local = installation(null);
  const build = installation("build");
  await backend.seed({
    remoteHosts: hosts,
    operations: [local],
    remoteStates: { build: { operations: [build] } },
  });
  await page.goto("/install?tab=local");
  const progress = page.getByRole("region", { name: "Installation progress" });
  await expect(progress).toContainText("Queued");
  await backend.failNext("get_managed_skills", "Local refresh unavailable");
  await backend.patch({
    operations: [
      { ...local, revision: 2, settled: true, items: [{ ...local.items[0], phase: "succeeded" }] },
    ],
  });
  await backend.emit("operation-changed", { host_id: null });
  await expect(progress).toContainText("library could not be refreshed");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(progress).toContainText("Queued");
  await expect(progress).not.toContainText("library could not be refreshed");
  await expect(
    page.getByRole("button", { name: /Local · Install Skills.*1 need attention/ }),
  ).toBeVisible();
  await backend.failNext("build:get_managed_skills", "Build refresh unavailable");
  await backend.patch({
    remoteStates: {
      build: {
        operations: [
          {
            ...build,
            revision: 2,
            settled: true,
            items: [{ ...build.items[0], phase: "succeeded" }],
          },
        ],
      },
    },
  });
  await backend.emit("operation-changed", { host_id: "build" });
  await expect(progress).toContainText("library could not be refreshed");
  await progress.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(progress).not.toContainText("library could not be refreshed");
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /^Local/ }).click();
  await expect(progress).toContainText("library could not be refreshed");
});

for (const theme of ["light", "dark"]) {
  test(`host shortcut opens its original project with keyboard navigation in ${theme}`, async ({
    page,
    backend,
  }) => {
    const operation: OperationSnapshot = {
      ...installation("build"),
      operation_id: "project-work",
      work: "project_update",
      project_id: "shared-project",
      project_path: "/projects/build-app",
      destination: { kind: "project", id: "shared-project", path: "/projects/build-app" },
      sources: [],
      items: [
        {
          ...installation("build").items[0],
          agent: "claude_code",
          source: null,
          relative_path: "demo",
        },
      ],
    };

    await backend.seed({
      remoteHosts: hosts,
      retainedHosts: ["build"],
      settings: { theme, text_size: "xlarge" },
      projects: [project("shared-project", "Local app", 0)],
      remoteStates: {
        build: {
          projects: [project("shared-project", "Build app", 0, { path: "/projects/build-app" })],
          operations: [operation],
        },
      },
    });
    await page.setViewportSize({ width: 1000, height: 900 });
    await page.goto("/my-skills");
    const shortcut = page.getByRole("button", { name: /build host · build-app.*1 in progress/ });
    await expect(shortcut).toBeVisible();
    await page.screenshot({
      animations: "disabled",
      path: test.info().outputPath(`host-shortcut-${theme}.png`),
    });
    await shortcut.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("heading", { name: "Build app", exact: true })).toBeVisible();
    await expect(page.getByRole("region", { name: "Project update progress" })).toContainText(
      "0 of 1",
    );
    await expect(page.getByRole("button", { name: "build host", exact: true })).toBeFocused();
    await page.getByRole("button", { name: "build host", exact: true }).click();
    await expect(
      page.getByRole("menuitemradio", { name: /build host.*1 in progress/ }),
    ).toBeVisible();
    await page.keyboard.press("Escape");
    await page.screenshot({
      animations: "disabled",
      path: test.info().outputPath(`host-operations-${theme}.png`),
    });
  });
}

test("a slow shortcut target cannot navigate a later viewed host when its refresh finishes", async ({
  page,
  backend,
}) => {
  const operation: OperationSnapshot = {
    ...installation("build"),
    operation_id: "project-work",
    work: "project_update",
    project_id: "shared-project",
    project_path: "/projects/build-app",
    destination: { kind: "project", id: "shared-project", path: "/projects/build-app" },
    sources: [],
  };

  await backend.seed({
    remoteHosts: hosts,
    retainedHosts: ["build"],
    remoteStates: {
      build: {
        projects: [project("shared-project", "Build app", 0, { path: "/projects/build-app" })],
        operations: [operation],
      },
      gpu: { projects: [project("shared-project", "GPU app", 0)] },
    },
  });
  await page.goto("/my-skills");
  await backend.hold("build:get_projects");
  await page.getByRole("button", { name: /build host · build-app/ }).click();
  await expect(
    page.getByRole("button", { name: "Connecting to build host…", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Connecting to build host…", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /gpu host/ }).click();
  await expect(page.getByRole("button", { name: "gpu host", exact: true })).toBeVisible();
  await backend.release("build:get_projects");
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));
  await expect(page).toHaveURL(/\/my-skills$/);
});

test("off-screen response-loss recovery checks its original host without navigating or replaying", async ({
  page,
  backend,
}) => {
  const build = installation("build");
  await backend.seed({
    remoteHosts: hosts,
    remoteStates: { build: { operations: [build] }, gpu: { operations: [installation("gpu")] } },
  });
  await page.goto("/install?tab=local");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "build host library",
  );
  await page.getByRole("button", { name: "build host", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /gpu host/ }).click();
  await backend.hold("remote_host_connect");
  await backend.emit("remote-session-state", { host_id: "build", state: "disconnected" });
  await expect(
    page.getByRole("button", {
      name: /build host · Install Skills.*Connection lost — checking result/,
    }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Cancel installation" }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Canceled",
  );
  await backend.patch({
    remoteStates: {
      build: {
        operations: [
          {
            ...build,
            revision: 2,
            settled: true,
            items: [{ ...build.items[0], phase: "succeeded" }],
          },
        ],
      },
    },
  });
  await backend.release("remote_host_connect");
  await expect.poll(() => backend.calls("remote_host_release")).toContainEqual({ hostId: "build" });
  await expect(page.getByRole("button", { name: "gpu host", exact: true })).toBeVisible();
  expect((await backend.calls("remote_host_connect")).filter((c) => c.select === false)).toEqual([
    { hostId: "build", select: false },
  ]);
  expect(
    (await backend.calls("remote_invoke")).filter(
      (c) => c.hostId === "build" && requestAction(c.args) === "submit_install",
    ),
  ).toEqual([]);
});

for (const theme of ["light", "dark"]) {
  test(`bounded recovery retains successes and an unknown result offers reconnect without Retry in ${theme}`, async ({
    page,
    backend,
  }) => {
    const build: OperationSnapshot = {
      ...installation("build"),
      items: [
        {
          ...installation("build").items[0],
          id: "done",
          relative_path: "done",
          phase: "succeeded",
        },
        {
          ...installation("build").items[0],
          id: "remaining",
          relative_path: "remaining",
          phase: "applying",
        },
      ],
    };

    await backend.seed({
      settings: { theme, text_size: "xlarge" },
      remoteHosts: hosts,
      retainedHosts: ["build"],
      remoteStates: { build: { operations: [build] } },
    });
    await page.setViewportSize({ width: 1000, height: 900 });
    await page.goto("/install?tab=local");
    await expect(page.getByRole("button", { name: /build host · Install Skills/ })).toBeVisible();
    await page.clock.install();
    await backend.hold("remote_host_connect");
    await backend.emit("remote-session-state", { host_id: "build", state: "disconnected" });
    await expect(
      page.getByRole("button", {
        name: /build host · Install Skills.*Connection lost — checking result/,
      }),
    ).toBeVisible();
    await page.screenshot({
      animations: "disabled",
      path: `/private/tmp/ski-23-checking-${theme}.png`,
    });
    await page.clock.fastForward(12500);
    await expect(
      page.getByRole("button", { name: /build host · Install Skills.*Outcome unknown/ }),
    ).toBeVisible();
    await backend.patch({ remoteStates: { build: { operations: [] } } });
    await backend.release("remote_host_connect");
    await page
      .getByRole("button", { name: /build host · Install Skills.*Outcome unknown/ })
      .click();
    const region = page.getByRole("region", { name: "Installation progress" });
    await expect(region).toContainText("done: Installed");
    await expect(region).toContainText("remaining: Outcome unknown");
    await expect(region.getByRole("button", { name: /Retry|Cancel/ })).toHaveCount(0);
    await expect(region.getByRole("button", { name: "Reconnect and check result" })).toBeVisible();
    await page.screenshot({
      animations: "disabled",
      path: `/private/tmp/ski-23-recovery-${theme}.png`,
    });
    await backend.patch({
      remoteStates: {
        build: {
          operations: [
            {
              ...build,
              revision: 2,
              settled: true,
              items: build.items.map((item) => ({ ...item, phase: "succeeded" })),
            },
          ],
        },
      },
    });
    await region.getByRole("button", { name: "Reconnect and check result" }).focus();
    await page.keyboard.press("Enter");
    await expect(region).toContainText("remaining: Installed");
    await expect(region).not.toContainText("Outcome unknown");
    await expect(region).toBeFocused();
    expect(
      (await backend.calls("remote_invoke")).filter((c) =>
        ["submit_install", "retry_failed", "confirm_install"].includes(
          String(requestAction(c.args)),
        ),
      ),
    ).toEqual([]);
  });
}

test("lost submit before admission stays correlated and cannot be resubmitted as a new write", async ({
  page,
  backend,
}) => {
  await backend.seed({
    remoteHosts: hosts,
    market: [
      { id: "demo", name: "demo", skill_id: "demo", source: "fixture/skills", installs: 10 },
    ],
  });
  await page.goto("/install");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await backend.dropNext("build:operation_request:submit_install", false);
  await backend.hold("remote_host_connect");
  await page.clock.install();
  await page.getByRole("button", { name: "Install", exact: true }).click();
  const region = page.getByRole("region", { name: "Installation progress" });
  await expect(region).toContainText("Connection lost — checking result");
  await page.clock.fastForward(12500);
  await expect(region).toContainText("Outcome unknown");
  await expect(page.getByRole("button", { name: "Install", exact: true })).toBeDisabled();
  expect(
    (await backend.calls("remote_invoke")).filter(
      (c) => requestAction(c.args) === "submit_install",
    ),
  ).toHaveLength(1);
  await backend.release("remote_host_connect");
  await region.getByRole("button", { name: "Reconnect and check result" }).click();
  await page.clock.fastForward(2500);
  await expect(region).toContainText("Outcome unknown");
  await expect(region.getByRole("button", { name: /Retry/ })).toHaveCount(0);

  const reads = (await backend.calls("remote_invoke")).filter(
    (c) => requestAction(c.args) === "reconcile",
  );

  expect(reads.length).toBeGreaterThan(0);

  const submits = (await backend.calls("remote_invoke")).filter(
    (c) => requestAction(c.args) === "submit_install",
  );

  const submitted = submits[0].args;

  if (
    !(submitted instanceof Object) ||
    !("request" in submitted) ||
    !(submitted.request instanceof Object) ||
    !("operation_id" in submitted.request)
  )
    throw new Error("Missing submitted operation identity");
  const submittedId = String(submitted.request.operation_id);

  expect(reads.every((read) => JSON.stringify(read.args).includes(submittedId))).toBe(true);
});

for (const action of ["retry_failed", "confirm_install", "cancel"] as const) {
  for (const afterAdmission of [false, true]) {
    test(`lost ${action} response ${afterAdmission ? "after" : "before"} admission preserves correlation and review authority`, async ({
      page,
      backend,
    }) => {
      const source: InstallSource =
        action === "confirm_install"
          ? { kind: "git", url: "https://fixture.invalid/repo" }
          : { kind: "local", path: "/sources/demo" };

      const original: OperationSnapshot = {
        ...installation("build"),
        settled: action !== "cancel",
        sources: [source],
        preview:
          action === "confirm_install"
            ? [{ rel_path: "demo", name: "demo", description: null }]
            : null,
        items: [
          {
            ...installation("build").items[0],
            source,
            phase:
              action === "retry_failed"
                ? "failed"
                : action === "confirm_install"
                  ? "needs_review"
                  : "queued",
          },
        ],
      };

      await backend.seed({
        remoteHosts: hosts,
        remoteStates: { build: { operations: [original], holdOperations: action === "cancel" } },
      });
      await page.goto(action === "confirm_install" ? "/install?tab=git" : "/install?tab=local");
      await page.getByRole("button", { name: "Local", exact: true }).click();
      await page.getByRole("menuitemradio", { name: /build host/ }).click();
      await backend.dropNext(`build:operation_request:${action}`, afterAdmission);
      await backend.hold("remote_host_connect");
      const region = page.getByRole("region", { name: "Installation progress" });

      if (action === "confirm_install") {
        await region.getByRole("button", { name: "Review selection" }).click();
        await page.getByRole("button", { name: "Import Selected", exact: true }).click();
      } else
        await region
          .getByRole("button", {
            name: action === "cancel" ? "Cancel installation" : "Retry failed (1)",
          })
          .click();
      await expect(region).toContainText("Connection lost — checking result");
      await expect(
        region.getByRole("button", { name: /Retry|Review selection|Cancel installation/ }),
      ).toHaveCount(0);
      await backend.release("remote_host_connect");

      if (afterAdmission) {
        await expect(region).toContainText(action === "cancel" ? "Canceled" : "Installed");
        await expect(region).not.toContainText("Connection lost");
      } else {
        await expect(region).toContainText("Outcome unknown");
        await expect(
          region.getByRole("button", { name: "Reconnect and check result" }),
        ).toBeVisible();
        await expect(region.getByRole("button", { name: /Retry|Review selection/ })).toHaveCount(0);
      }

      expect(
        (await backend.calls("remote_invoke")).filter((c) => requestAction(c.args) === action),
      ).toHaveLength(1);
    });
  }
}

test("lost committed submit repairs the source result with no second installation", async ({
  page,
  backend,
}) => {
  await backend.seed({
    remoteHosts: hosts,
    market: [
      { id: "demo", name: "demo", skill_id: "demo", source: "fixture/skills", installs: 10 },
    ],
  });
  await page.goto("/install");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await backend.dropNext("build:operation_request:submit_install", true);
  await backend.hold("remote_host_connect");
  await page.getByRole("button", { name: "Install", exact: true }).click();
  const region = page.getByRole("region", { name: "Installation progress" });
  await expect(region).toContainText("Connection lost — checking result");
  await backend.release("remote_host_connect");
  await expect(region).toContainText("Installed");
  await expect(region).not.toContainText("Outcome unknown");
  expect(
    (await backend.calls("remote_invoke")).filter(
      (c) => requestAction(c.args) === "submit_install",
    ),
  ).toHaveLength(1);
});

test("lost project admission remains in its original project navigation and never repeats Update All", async ({
  page,
  backend,
}) => {
  const app = project("app", "Build app", 1);
  await backend.seed({
    remoteHosts: hosts,
    projects: [project("app", "Local app", 1)],
    remoteStates: {
      build: {
        projects: [app],
        projectSkills: {
          app: [
            projectSkill("demo", "claude_code", {
              in_center: true,
              center_skill_id: "demo",
              sync_status: "center_newer",
            }),
          ],
        },
        projectAgentTargets: { app: [agentTarget("claude_code", "Claude Code")] },
      },
    },
  });
  await page.goto("/project/app");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("button", { name: /^Update All/ })).toBeVisible();
  await backend.dropNext("build:operation_request:submit_project_update", false);
  await backend.hold("remote_host_connect");
  await page.clock.install();
  await page.getByRole("button", { name: /^Update All/ }).click();
  const region = page.getByRole("region", { name: "Project update progress" });
  await expect(region).toContainText("Connection lost — checking result");
  await page.clock.fastForward(12500);
  await expect(region).toContainText("Outcome unknown");
  await expect(
    page.getByRole("button", { name: /Build app.*Outcome unknown/ }).first(),
  ).toBeVisible();
  await expect(region.getByRole("button", { name: "Reconnect and check result" })).toBeVisible();
  await page.getByRole("button", { name: /^Update All/ }).click();
  expect(
    (await backend.calls("remote_invoke")).filter(
      (c) => requestAction(c.args) === "submit_project_update",
    ),
  ).toHaveLength(1);
  await backend.release("remote_host_connect");
});

test("recovery rejects another attempt and host even at the captured receipt revision", async ({
  page,
  backend,
}) => {
  const original = { ...installation("build"), revision: 5 };
  await backend.seed({
    remoteHosts: hosts,
    retainedHosts: ["build"],
    remoteStates: { build: { operations: [original] } },
  });
  await page.goto("/my-skills");
  await expect(page.getByRole("button", { name: /build host · Install Skills/ })).toBeVisible();
  await backend.hold("remote_host_connect");
  await backend.emit("remote-session-state", { host_id: "build", state: "disconnected" });

  const checking = page.getByRole("button", {
    name: /build host · Install Skills.*Connection lost — checking result/,
  });

  await expect(checking).toBeVisible();
  await backend.patch({
    remoteStates: {
      build: {
        operations: [
          {
            ...original,
            attempt_id: "older-attempt",
            settled: true,
            items: [{ ...original.items[0], phase: "succeeded" }],
          },
          {
            ...original,
            host_id: "gpu",
            revision: 100,
            settled: true,
            items: [{ ...original.items[0], phase: "succeeded" }],
          },
        ],
      },
    },
  });
  await backend.emit("operation-changed", { host_id: "build" });
  await expect
    .poll(
      async () =>
        (await backend.calls("remote_invoke")).filter(
          (c) => c.hostId === "build" && requestAction(c.args) === "list",
        ).length,
    )
    .toBeGreaterThan(1);
  await expect(checking).toBeVisible();
  await backend.patch({
    remoteStates: {
      build: {
        operations: [
          { ...original, settled: true, items: [{ ...original.items[0], phase: "unchanged" }] },
        ],
      },
    },
  });
  await backend.release("remote_host_connect");
  await expect.poll(() => backend.calls("remote_host_release")).toContainEqual({ hostId: "build" });
  await expect(checking).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Local", exact: true })).toBeVisible();
});

test("an uncertain Git source cannot be cloned again with equivalent sanitized intent", async ({
  page,
  backend,
}) => {
  const source: InstallSource = { kind: "git", url: "https://fixture.invalid/repo" };

  const unknown: OperationSnapshot = {
    ...installation("build"),
    settled: true,
    sources: [source],
    items: [{ ...installation("build").items[0], source, phase: "outcome_unknown" }],
  };

  await backend.seed({ remoteHosts: hosts, remoteStates: { build: { operations: [unknown] } } });
  await page.goto("/install?tab=git");
  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();
  await expect(page.getByRole("region", { name: "Installation progress" })).toContainText(
    "Outcome unknown",
  );
  await page
    .getByPlaceholder("https://github.com/user/repo.git or git@github.com...")
    .fill("https://fixture.invalid/repo?ref=untrusted#selection");
  await page
    .getByPlaceholder("https://github.com/user/repo.git or git@github.com...")
    .press("Enter");
  await expect
    .poll(
      async () =>
        (await backend.calls("remote_invoke")).filter(
          (c) => requestAction(c.args) === "submit_install",
        ).length,
    )
    .toBe(0);
});
