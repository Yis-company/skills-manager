import { queryOptions } from "@tanstack/react-query";

import { invokeHost } from "./hostCall";

export interface ProjectGitFile {
  path: string;
  original_path: null | string;
  status: string;
  skill_related: boolean;
  blocked_reason: null | string;
  diff: string;
  truncated: boolean;
}

export interface ProjectGitStatus {
  review_id: string;
  root: string;
  branch: null | string;
  head: null | string;
  blocked_reason: null | string;
  files: ProjectGitFile[];
  remotes: { name: string; url: string }[];
  upstream_remote: null | string;
  upstream_branch: null | string;
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
  existing_url: null | string;
}

export type ProjectGitRequest =
  | { action: "commit"; review_id: string; paths: string[]; message: string }
  | { action: "create_branch"; review_id: string; name: string }
  | { action: "create_pr"; review_id: string; title: string }
  | { action: "pr_preview"; remote: string; base: null | string }
  | { action: "push_preview"; remote: string; branch: string }
  | { action: "push"; review_id: string }
  | { action: "status" };

type ProjectGitResponse<R extends ProjectGitRequest> = R["action"] extends "status"
  ? ProjectGitStatus
  : R["action"] extends "push_preview"
    ? ProjectGitPushReview
    : R["action"] extends "pr_preview"
      ? ProjectGitPrReview
      : R["action"] extends "commit" | "create_pr"
        ? string
        : null;

export function projectGitRequest<R extends ProjectGitRequest>(
  hostId: null | string,
  projectId: string,
  request: R,
): Promise<ProjectGitResponse<R>> {
  return invokeHost(hostId, "project_git_request", { projectId, request });
}

export const projectGitQueryKey = (hostId: null | string, projectId: string) =>
  ["host", hostId, "projectGit", projectId] as const;

export const projectGitQueryOptions = (hostId: null | string, projectId: string) =>
  queryOptions({
    queryKey: projectGitQueryKey(hostId, projectId),
    queryFn: () => projectGitRequest(hostId, projectId, { action: "status" }),
    retry: false,
    networkMode: "always",
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
