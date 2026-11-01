import { expect, test } from "../fixtures";

test("shows the pending local library move while the current location stays active", async ({
  page,
  backend,
}) => {
  await backend.seed({ centralRepoPendingPath: "/home/e2e/.skills-manager-next" });
  await page.goto("/settings/library");

  await expect(
    page.getByText(
      "Restart the app to move the library to ~/.skills-manager-next. Until then this session keeps using the current location.",
    ),
  ).toBeVisible();
  await expect(page.getByText("~/.skills-manager", { exact: true })).toBeVisible();
});

test("a refused library move stays editable and explains the destination problem", async ({
  page,
  backend,
}) => {
  await backend.seed({});
  await page.goto("/settings/library");
  await page.getByRole("button", { name: "Change Directory" }).click();
  const input = page.getByRole("textbox");
  await input.fill("/home/e2e/not-empty");
  await backend.failNext("set_central_repo_path", "REPO_TARGET_NOT_EMPTY: /home/e2e/not-empty");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Choose an empty folder for the library.")).toBeVisible();
  await expect(input).toHaveValue("/home/e2e/not-empty");
  await input.press("Escape");
  await expect(page.getByText("~/.skills-manager", { exact: true })).toBeVisible();
});
