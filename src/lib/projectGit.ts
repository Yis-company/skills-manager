import { queryOptions } from "@tanstack/react-query";
import { invokeHost } from "./hostCall";

export interface ProjectGitFile {
  path: string;
  original_path: string | null;
  status: string;
  skill_related: boolean;
  blocked_reason: string | null;
  diff: string;
  truncated: boolean;
}

export interface ProjectGitStatus {
  review_id: string;
  root: string;
  branch: string | null;
  head: string | null;
  blocked_reason: string | null;
  files: ProjectGitFile[];
  remotes: { name: string; url: string }[];
  upstream_remote: string | null;
  upstream_branch: string | null;
}

export interface ProjectGitPushReview {
  review_id: string;
  remote: string;
  branch: string;
  url: string;
  commits: { id: string; summary: string }[];
}

export interface ProjectGitPrReview {
  review_id: string;
  repository: string;
  head: string;
  base: string;
  bases: string[];
  existing_url: string | null;
}

export type ProjectGitRequest =
  | { action: "status" }
  | { action: "commit"; review_id: string; paths: string[]; message: string }
  | { action: "create_branch"; review_id: string; name: string }
  | { action: "push_preview"; remote: string; branch: string }
  | { action: "push"; review_id: string }
  | { action: "pr_preview"; remote: string; base: string | null }
  | { action: "create_pr"; review_id: string; title: string };

type ProjectGitResponse<R extends ProjectGitRequest> =
  R["action"] extends "status" ? ProjectGitStatus :
  R["action"] extends "push_preview" ? ProjectGitPushReview :
  R["action"] extends "pr_preview" ? ProjectGitPrReview :
  R["action"] extends "commit" | "create_pr" ? string : null;

export function projectGitRequest<R extends ProjectGitRequest>(
  hostId: string | null,
  projectId: string,
  request: R,
): Promise<ProjectGitResponse<R>> {
  return invokeHost(hostId, "project_git_request", { projectId, request });
}

export const projectGitQueryKey = (hostId: string | null, projectId: string) =>
  ["host", hostId, "projectGit", projectId] as const;

export const projectGitQueryOptions = (hostId: string | null, projectId: string) =>
  queryOptions({
    queryKey: projectGitQueryKey(hostId, projectId),
    queryFn: () => projectGitRequest(hostId, projectId, { action: "status" }),
    retry: false,
    networkMode: "always",
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
