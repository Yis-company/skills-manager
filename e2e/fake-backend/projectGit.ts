import type { ProjectGitRequest } from "../../src/lib/projectGit";
import type { State } from "./state";

export function projectGitHandler(
  { projectId, request }: { projectId: string; request: ProjectGitRequest },
  state: State,
) {
  const git = state.projectGit[projectId];
  if (!git) throw new Error("This project is not in a Git repository.");
  switch (request.action) {
    case "status": return git.status;
    case "push_preview": return git.push;
    case "pr_preview": return { ...git.pr, base: request.base ?? git.pr.base };
    case "commit":
      git.status.files = git.status.files.filter((file) => !request.paths.includes(file.path));
      git.status.head = "new-commit";
      git.status.review_id = "after-commit";
      return git.status.head;
    case "create_branch":
      git.status.branch = request.name;
      git.status.review_id = "after-branch";
      return null;
    case "push":
      git.status.upstream_remote = git.push.remote;
      git.status.upstream_branch = git.push.branch;
      return null;
    case "create_pr":
      git.pr.existing_url = "https://github.com/example/project/pull/42";
      return git.pr.existing_url;
  }
}
