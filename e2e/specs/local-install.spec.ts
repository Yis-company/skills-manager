import { expect, test } from "../fixtures";

test("batch folder control submits shared library work and keeps other sources available", async ({
  page,
  backend,
}) => {
  await backend.seed({ dialogPaths: ["/sources/team"], holdOperations: true });
  await page.goto("/install?tab=local");
  await page.getByRole("button", { name: "Batch import from folder" }).click();
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
        sources: [{ kind: "local_batch", path: "/sources/team" }],
      },
    ]);
  await expect(page.getByRole("button", { name: "Select folder", exact: true })).toBeEnabled();
  await expect(page.getByRole("region", { name: "Installation progress" })).toBeVisible();
});
