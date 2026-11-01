import { describe, expect, it, vi } from "vitest";

import { applyWorkspaceChanges, type ReviewedWorkspaceChange } from "./workspaceChanges";

const operation = (
  id: string,
  apply = vi.fn<() => Promise<void>>(async () => {}),
  validate = vi.fn<() => Promise<void>>(async () => {}),
): ReviewedWorkspaceChange => ({
  id,
  name: id,
  agent: "Codex",
  destination: "Local · project",
  add: true,
  prepare: vi.fn<ReviewedWorkspaceChange["prepare"]>(),
  prepared: { apply, validate, warnings: [] },
});

describe("reviewed workspace changes", () => {
  it("validates every reviewed target before the first write", async () => {
    const first = operation("first");

    const stale = operation(
      "stale",
      vi.fn<() => Promise<void>>(async () => {}),
      vi.fn<() => Promise<void>>(async () => {
        throw new Error("changed since review");
      }),
    );

    await expect(applyWorkspaceChanges([first, stale])).rejects.toThrow("changed since review");
    expect(first.prepared.apply).not.toHaveBeenCalled();
    expect(stale.prepared.apply).not.toHaveBeenCalled();
  });
  it("reports successful and failed pairs separately so retry can exclude success", async () => {
    const first = operation("first");

    const failed = operation(
      "failed",
      vi.fn<() => Promise<void>>(async () => {
        throw new Error("offline");
      }),
    );

    const last = operation("last");
    const result = await applyWorkspaceChanges([first, failed, last]);
    expect(result.succeeded).toEqual(["first", "last"]);
    expect(result.failed).toEqual([{ id: "failed", name: "failed · Codex", error: "offline" }]);
    const retry = operation("failed");
    await applyWorkspaceChanges([retry]);
    expect(first.prepared.apply).toHaveBeenCalledTimes(1);
    expect(last.prepared.apply).toHaveBeenCalledTimes(1);
  });
});
