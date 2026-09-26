import { expect, test } from "../fixtures";

test("shows the pending local library move while the current location stays active", async ({ page, backend }) => {
  await backend.seed({ centralRepoPendingPath: "/home/e2e/.skills-manager-next" });
  await page.goto("/settings/library");

  await expect(
    page.getByText("Restart the app to move the library to ~/.skills-manager-next. Until then this session keeps using the current location."),
  ).toBeVisible();
  await expect(page.getByText("~/.skills-manager", { exact: true })).toBeVisible();
});
