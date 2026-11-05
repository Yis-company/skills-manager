import type { Page } from "@playwright/test";

import { skill } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const hosts = [
  { id: "build", name: "build host", ssh_target: "dev@build", cli_path: null, created_at: 1 },
];

const nameField = (page: Page) => page.getByLabel("Name", { exact: true });

const descriptionField = (page: Page) => page.getByLabel(/^Description/);

const instructions = (page: Page) => page.getByRole("textbox", { name: "Instructions" });

const createButton = (page: Page) => page.getByRole("button", { name: "Create skill" });

async function fillDraft(page: Page, name: string) {
  await nameField(page).fill(name);
  await descriptionField(page).fill("Draft release notes from merged PRs.");
  await instructions(page).fill("List the PRs merged since the last tag.");
}

test("creates a skill with an agent chosen and finds it in the library", async ({
  page,
  backend,
}) => {
  await backend.seed({});
  await page.goto("/install?tab=create");
  await expect(createButton(page)).toBeDisabled();

  await fillDraft(page, "release-notes");
  await expect(page.getByText(/skills\/release-notes\/SKILL\.md/)).toBeVisible();
  await page.getByRole("checkbox", { name: "Claude Code" }).check();
  await createButton(page).click();

  await expect(page.getByText("release-notes created")).toBeVisible();
  await expect(nameField(page)).toHaveValue("");
  expect(await backend.calls("create_skill")).toEqual([
    {
      request: {
        name: "release-notes",
        content: {
          kind: "written",
          description: "Draft release notes from merged PRs.",
          instructions: expect.stringContaining("List the PRs merged"),
        },
        agents: ["claude_code"],
        preset: null,
        project: null,
      },
    },
  ]);

  await page.goto("/my-skills");
  await expect(page.getByRole("heading", { name: "release-notes", exact: true })).toBeVisible();
});

test("a taken name keeps the draft and shows the error on the field", async ({ page, backend }) => {
  await backend.seed({ skills: [skill("s1", "taken")] });
  await page.goto("/install?tab=create");

  await fillDraft(page, "taken");
  await createButton(page).click();

  await expect(page.getByRole("alert")).toContainText("already in the library");
  await expect(nameField(page)).toHaveAttribute("aria-invalid", "true");
  await expect(descriptionField(page)).toHaveValue("Draft release notes from merged PRs.");

  await nameField(page).fill("not-taken");
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("an invalid name explains the rule and can't be submitted", async ({ page, backend }) => {
  await backend.seed({});
  await page.goto("/install?tab=create");

  await fillDraft(page, "Release Notes");

  await expect(page.getByRole("alert")).toContainText("lowercase letters");
  await expect(createButton(page)).toBeDisabled();
});

test("another failure keeps the draft and shows a banner", async ({ page, backend }) => {
  await backend.seed({});
  await page.goto("/install?tab=create");
  await fillDraft(page, "release-notes");

  await backend.failNext("create_skill", "The library is busy");
  await createButton(page).click();

  await expect(page.getByRole("alert")).toContainText("The library is busy");
  await expect(nameField(page)).toHaveValue("release-notes");
});

test("a partial failure names the target that failed", async ({ page, backend }) => {
  await backend.seed({ createSkillTargetErrors: { codex: "disk full" } });
  await page.goto("/install?tab=create");

  await fillDraft(page, "release-notes");
  await page.getByRole("checkbox", { name: "Claude Code" }).check();
  await page.getByRole("checkbox", { name: "Codex" }).check();
  await createButton(page).click();

  const alert = page.getByRole("alert");
  await expect(alert).toContainText("release-notes was created, but some targets failed");
  await expect(alert).toContainText("Codex: disk full");
  await expect(alert).not.toContainText("Claude Code");
});

test("the draft survives switching tabs", async ({ page, backend }) => {
  await backend.seed({});
  await page.goto("/install?tab=create");
  await fillDraft(page, "release-notes");

  await page.getByRole("button", { name: "Git Install" }).click();
  await page.getByRole("button", { name: "Create", exact: true }).click();

  await expect(nameField(page)).toHaveValue("release-notes");
  await expect(instructions(page)).toContainText("List the PRs merged");
});

test("the form works by keyboard alone", async ({ page, backend }) => {
  await backend.seed({});
  await page.goto("/install?tab=create");

  await nameField(page).focus();
  await page.keyboard.type("by-keyboard");
  await page.keyboard.press("Tab");
  await page.keyboard.type("Created without a mouse.");
  await instructions(page).focus();
  await page.keyboard.type("Press the keys.");
  await page.getByRole("checkbox", { name: "Claude Code" }).focus();
  await page.keyboard.press("Space");
  await expect(page.getByRole("checkbox", { name: "Claude Code" })).toBeChecked();
  await createButton(page).focus();
  await page.keyboard.press("Enter");

  await expect(page.getByText("by-keyboard created")).toBeVisible();
});

test("changing host keeps the text, clears targets and creates on that host", async ({
  page,
  backend,
}) => {
  await backend.seed({
    remoteHosts: hosts,
    remoteStates: { build: { centralRepoPath: "/srv/agents-manager" } },
  });
  await page.goto("/install?tab=create");
  await fillDraft(page, "on-build");
  await page.getByRole("checkbox", { name: "Claude Code" }).check();

  await page.getByRole("button", { name: "Local", exact: true }).click();
  await page.getByRole("menuitemradio", { name: /build host/ }).click();

  await expect(page.getByText("Destination: build host library")).toBeVisible();
  await expect(page.getByText("/srv/agents-manager/skills/on-build/SKILL.md")).toBeVisible();
  await expect(nameField(page)).toHaveValue("on-build");
  await expect(page.getByRole("checkbox", { name: "Claude Code" })).not.toBeChecked();

  await createButton(page).click();
  await expect(page.getByText("on-build created")).toBeVisible();
});
