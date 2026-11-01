import type { ChangeReview } from "../../src/lib/changeReview";
import { gitStatus, preset, skill, tool } from "../fake-backend/state";
import { expect, test } from "../fixtures";

const target = {
  id: "target",
  skill_id: "old",
  tool: "codex",
  target_path: "/fixture/agents/old",
  mode: "symlink",
  status: "ok",
  synced_at: 1,
};

const conflict = { skill_id: "s", name: "frontend", this_host: "Laptop", other_host: "Build host" };

const comparison: ChangeReview = {
  kind: "skill",
  key: "s",
  tool: null,
  left_label: "Laptop",
  right_label: "Build host",
  files: [
    {
      relative_path: "SKILL.md",
      status: "modified",
      content_kind: "text",
      original_text: "Keep the local instruction",
      updated_text: "Use the shared instruction",
      executable_before: false,
      executable_after: false,
    },
  ],
};

const syncSeed = {
  gitStatus: gitStatus(),
  settings: {
    git_backup_remote_url: "https://example.test/sync.git",
    backup_sync_notice_dismissed: "1",
  },
  syncConflicts: [conflict],
  changeReviews: { "skill:s": comparison },
};

test("keyboard inspection and selection remain separate in both Library views", async ({
  page,
  backend,
}) => {
  await backend.seed({
    skills: [skill("example", "example")],
    presets: [preset("p", "Writing", 0)],
    activePresetId: "p",
  });
  await page.goto("/my-skills");
  await expect(page.getByText("Viewing preset: Writing")).toBeVisible();
  await expect(page.getByText("Shared active preset: Writing")).toBeVisible();

  for (const view of ["Grid view", "List view"]) {
    await page.getByRole("button", { name: view, exact: true }).click();
    const inspect = page.getByRole("button", { name: "example", exact: true });
    await inspect.focus();
    await page.keyboard.press("Enter");
    await expect(page.getByRole("button", { name: /Close/ }).last()).toBeVisible();
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Select", exact: true }).click();
    const select = page.getByRole("checkbox", { name: "Select example" });
    await select.focus();
    await page.keyboard.press("Space");
    await expect(select).toBeChecked();
    await page.getByRole("button", { name: "Cancel selection" }).click();
  }

  expect(await backend.calls("apply_preset_to_default")).toHaveLength(0);
});

test("every preset switch that removes skills requires review even after the old one-time flag", async ({
  page,
  backend,
}) => {
  await backend.seed({
    skills: [
      skill("old", "old", { preset_ids: ["old-p"], targets: [target] }),
      skill("new", "new", { preset_ids: ["new-p"] }),
    ],
    presets: [preset("old-p", "Old", 0), preset("new-p", "New", 1)],
    activePresetId: "old-p",
    tools: [tool("codex", "Codex")],
    settings: { preset_switch_previewed: "1" },
  });
  await page.goto("/global-workspace");

  const switchButton = async () => {
    await page.getByRole("button", { name: "Switch shared preset…", exact: true }).click();
    await page.getByLabel("Preset", { exact: true }).selectOption("new-p");
    await page.getByRole("button", { name: "Review preset changes" }).click();
  };

  await switchButton();
  let dialog = page.getByRole("dialog", { name: "Switch to New?" });
  await expect(dialog).toContainText("/fixture/agents/old");
  await expect(dialog).toContainText("including direct additions");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await backend.calls("apply_preset_to_default")).toHaveLength(0);
  await switchButton();
  dialog = page.getByRole("dialog", { name: "Switch to New?" });
  await dialog.getByRole("button", { name: "Switch", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(await backend.calls("apply_preset_to_default")).toHaveLength(1);
  await backend.patch({
    activePresetId: "old-p",
    skills: [
      skill("old", "old", { preset_ids: ["old-p"], targets: [target] }),
      skill("new", "new", { preset_ids: ["new-p"] }),
    ],
  });
  await page.reload();
  await switchButton();
  await expect(page.getByRole("dialog", { name: "Switch to New?" })).toBeVisible();
});

test("comparison is read-only, cancellation restores focus, and a stale choice preserves review", async ({
  page,
  backend,
}) => {
  await backend.seed(syncSeed);
  await page.goto("/backup");
  await page.getByRole("button", { name: /Needs review/ }).click();
  const compare = page.getByRole("button", { name: "Compare changes for frontend" });
  await compare.click();
  const sheet = page.getByRole("dialog", { name: "Compare frontend" });
  await expect(sheet).toContainText("Keep the local instruction");
  await expect(sheet).toContainText("Use the shared instruction");
  expect(
    (await backend.calls("sync_screen_call")).filter((c) => c.command === "sync_resolve"),
  ).toHaveLength(0);
  await sheet.getByRole("button", { name: "Keep Build host's version" }).click();
  let confirmation = page.getByRole("dialog", { name: "Keep Build host's version" });
  await expect(confirmation).toBeVisible();

  // Native dialog containment: repeated Tab never reaches the background sheet.
  for (let index = 0; index < 8; index++) {
    await page.keyboard.press("Tab");
    await expect
      .poll(() => confirmation.evaluate((element) => element.contains(document.activeElement)))
      .toBe(true);
  }

  await page.keyboard.press("Escape");
  await expect(confirmation).toHaveCount(0);
  await expect(sheet.getByRole("button", { name: "Keep Build host's version" })).toBeFocused();
  await backend.patch({
    changeReviews: { "skill:s": { ...comparison, right_label: "A newer version" } },
  });
  await sheet.getByRole("button", { name: "Keep Build host's version" }).click();
  confirmation = page.getByRole("dialog", { name: "Keep Build host's version" });
  await confirmation.getByRole("button", { name: "Apply reviewed choice" }).click();
  await expect(confirmation.getByRole("alert")).toContainText("Changes since review");
  await expect(page.getByRole("heading", { name: "frontend", exact: true })).toHaveCount(1);
});

test("failed comparison offers retry without offering a blind resolution", async ({
  page,
  backend,
}) => {
  await backend.seed(syncSeed);
  await page.goto("/backup");
  await page.getByRole("button", { name: /Needs review/ }).click();
  await backend.failNext("sync_review", "Host CLI does not support comparison");
  await page.getByRole("button", { name: "Compare changes for frontend" }).click();
  const sheet = page.getByRole("dialog", { name: "Compare frontend" });
  await expect(sheet.getByRole("alert")).toContainText("matching CLI");
  await expect(sheet.getByRole("button", { name: "Keep Build host's version" })).toHaveCount(0);
  await sheet.getByRole("button", { name: "Refresh comparison" }).click();
  await expect(sheet).toContainText("Use the shared instruction");
});

test("log failure is distinct from empty success and supports retry", async ({ page, backend }) => {
  await backend.seed(syncSeed);
  await page.goto("/backup");
  await page
    .getByRole("button", { name: /this host/ })
    .first()
    .click();
  await page.getByRole("button", { name: "Details", exact: true }).click();
  await backend.failNext("sync_agent_log", "Network unavailable");
  await page.getByRole("button", { name: "Agent log", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Could not load");
  await page.getByRole("button", { name: "Retry log" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
});

for (const theme of ["light", "dark"]) {
  for (const textSize of ["default", "xlarge"]) {
    for (const width of [1440, 1100]) {
      test(`long Library and comparison stay usable: ${theme}, ${textSize}, ${width}`, async ({
        page,
        backend,
      }) => {
        await page.setViewportSize({ width, height: width === 1100 ? 640 : 860 });
        await backend.seed({
          ...syncSeed,
          presets: Array.from({ length: 25 }, (_, index) =>
            preset(
              `p-${index}`,
              index === 0 ? "Writing" : `Preset ${index} with a long descriptive name`,
              index,
            ),
          ),
          skills: Array.from({ length: 8 }, (_, index) =>
            skill(
              `s-${index}`,
              `Resource ${index} with a long name for inspecting and editing agent instructions`,
              {
                preset_ids: ["p-0"],
                description:
                  "A meaningful description that should remain readable when a narrow desktop window and the largest supported text setting are used.",
              },
            ),
          ),
          settings: { ...syncSeed.settings, theme, text_size: textSize },
          activePresetId: "p-0",
        });
        await page.goto("/my-skills");
        await expect(page.getByText("Viewing preset: Writing")).toBeVisible();
        await expect(
          page.getByRole("button", { name: "Global Workspace", exact: true }),
        ).toBeInViewport();
        await expect(page.getByRole("button", { name: "Presets", exact: true })).toHaveAttribute(
          "aria-expanded",
          "true",
        );

        const overflow = () =>
          page.evaluate(
            () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
          );

        await expect.poll(overflow).toBe(false);

        if (
          (width === 1100 && textSize === "xlarge") ||
          (width === 1440 && textSize === "default" && theme === "light")
        ) {
          await page.screenshot({
            path: `.impeccable/review/2026-11-01-remediation/library-${theme}-${textSize}-${width}.png`,
          });
        }

        await page.goto("/backup");
        await page.getByRole("button", { name: /Needs review/ }).click();
        await page.getByRole("button", { name: "Compare changes for frontend" }).click();
        const dialog = page.getByRole("dialog", { name: "Compare frontend" });
        await expect(dialog).toContainText("Use the shared instruction");
        const bounds = await dialog.boundingBox();
        expect(bounds).not.toBeNull();
        expect(bounds!.x).toBeGreaterThanOrEqual(0);
        expect(bounds!.y).toBeGreaterThanOrEqual(0);
        expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width + 1);
        expect(bounds!.y + bounds!.height).toBeLessThanOrEqual((width === 1100 ? 640 : 860) + 1);

        await expect.poll(overflow).toBe(false);

        if (
          (width === 1100 && textSize === "xlarge") ||
          (width === 1440 && textSize === "default" && theme === "light")
        ) {
          await page.screenshot({
            path: `.impeccable/review/2026-11-01-remediation/comparison-${theme}-${textSize}-${width}.png`,
          });
        }
      });
    }
  }
}

test("Help is reachable from the command palette and closes from the keyboard", async ({
  page,
  backend,
}) => {
  await backend.seed({ skills: [skill("example", "example")] });
  await page.goto("/my-skills");
  await page.getByRole("link", { name: "Library", exact: true }).focus();
  await page.keyboard.press(process.platform === "darwin" ? "Meta+k" : "Control+k");
  await page.getByPlaceholder("Jump to skill, preset, project, action…").fill("Quick Start");
  await page.keyboard.press("Enter");
  const help = page.getByRole("dialog", { name: "Quick Start for Agents Manager" });
  await expect(help).toContainText("The viewed preset");
  await expect(help.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(help.getByRole("button", { name: "Close", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(help).toHaveCount(0);
});

test("a shared switch without removals still requires its explicit impact review", async ({
  page,
  backend,
}) => {
  await backend.seed({
    skills: [skill("s", "example", { preset_ids: ["p"] })],
    presets: [preset("p", "Writing", 0)],
    tools: [tool("codex", "Codex")],
  });
  await page.goto("/global-workspace");
  await page.getByRole("button", { name: "Switch shared preset…", exact: true }).click();
  await page.getByRole("button", { name: "Review preset changes" }).click();
  expect(await backend.calls("apply_preset_to_default")).toHaveLength(0);
  await page
    .getByRole("dialog", { name: "Switch to Writing?" })
    .getByRole("button", { name: "Switch", exact: true })
    .click();
  await expect.poll(async () => (await backend.calls("apply_preset_to_default")).length).toBe(1);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect((await backend.calls("apply_preset_to_default"))[0].review).toContain('"removals":[]');
});

test("composing Escape keeps confirmation open, then normal Escape closes only confirmation", async ({
  page,
  backend,
}) => {
  await backend.seed(syncSeed);
  await page.goto("/backup");
  await page.getByRole("button", { name: /Needs review/ }).click();
  await page.getByRole("button", { name: "Compare changes for frontend" }).click();
  const sheet = page.getByRole("dialog", { name: "Compare frontend" });
  await sheet.getByRole("button", { name: "Keep Build host's version" }).click();
  const confirmation = page.getByRole("dialog", { name: "Keep Build host's version" });
  await confirmation.evaluate((element) =>
    element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }),
    ),
  );
  await expect(confirmation).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(confirmation).toHaveCount(0);
  await expect(sheet).toBeVisible();
  expect(
    (await backend.calls("sync_screen_call")).filter((call) => call.command === "sync_resolve"),
  ).toHaveLength(0);
});
