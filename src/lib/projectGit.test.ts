import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "vitest";

import { setActiveHostId } from "./hostCall";
import { projectGitQueryOptions, projectGitRequest } from "./projectGit";
import { mockTauriIpc } from "./tauriIpcTesting";

const invoke = mockTauriIpc();

afterEach(() => setActiveHostId(null));

describe("project Git host ownership", () => {
  it("keeps a reviewed mutation on its captured host after the active host changes", async () => {
    setActiveHostId("another-host");
    const request = { action: "push" as const, review_id: "review-on-originating-host" };
    invoke.mockResolvedValue(null);
    await projectGitRequest("originating-host", "project-1", request);
    expect(invoke).toHaveBeenCalledWith("remote_invoke", {
      hostId: "originating-host",
      command: "project_git_request",
      args: { projectId: "project-1", request },
    });
  });

  it("keeps local Git operations local when a remote is now active", async () => {
    setActiveHostId("remote");
    invoke.mockResolvedValue("commit-id");

    const request = {
      action: "commit" as const,
      review_id: "local-review",
      paths: [".agents/skills/tool/SKILL.md"],
      message: "Update tool",
    };

    await expect(projectGitRequest(null, "p1", request)).resolves.toBe("commit-id");
    expect(invoke).toHaveBeenCalledWith("project_git_request", { projectId: "p1", request });
  });

  it("isolates status caches by host and project and does not retry failed operations", async () => {
    const client = new QueryClient();
    invoke.mockResolvedValueOnce({ root: "/local" }).mockResolvedValueOnce({ root: "/remote" });
    const local = projectGitQueryOptions(null, "same-id");
    const remote = projectGitQueryOptions("remote", "same-id");
    await client.fetchQuery(local);
    await client.fetchQuery(remote);
    expect(client.getQueryData(local.queryKey)).toEqual({ root: "/local" });
    expect(client.getQueryData(remote.queryKey)).toEqual({ root: "/remote" });
    invoke.mockRejectedValueOnce(new Error("push result unknown"));
    await expect(
      projectGitRequest("remote", "same-id", { action: "push", review_id: "r" }),
    ).rejects.toThrow("push result unknown");
    expect(invoke).toHaveBeenCalledTimes(3);
    client.clear();
  });
});
