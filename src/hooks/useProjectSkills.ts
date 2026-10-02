import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";

import { useApp } from "../context/AppContext";
import { projectSkillsQueryOptions, refreshQuery } from "../lib/appQueries";
import type { ProjectSkill } from "../lib/tauri";

const EMPTY_PROJECT_SKILLS: ProjectSkill[] = [];

/** A project's scanned skills, cached by the host and project that own them. */
export function useProjectSkills(id: string | undefined) {
  const { activeHostId } = useApp();
  const queryClient = useQueryClient();
  const options = projectSkillsQueryOptions(activeHostId, id ?? "");
  const query = useQuery({ ...options, enabled: Boolean(id) });

  // Keep the Promise-returning API used by project actions. A refresh always
  // cancels an older list read before fetching the authoritative list again.
  const loadSkills = useCallback(async () => {
    if (!id) return;

    try {
      await refreshQuery(queryClient, projectSkillsQueryOptions(activeHostId, id));
    } catch (error) {
      // Query state carries the visible error; preserve the existing action API
      // contract so callers can finish their own mutation cleanup.
      console.error("Failed to load project skills:", error);
    }
  }, [activeHostId, id, queryClient]);

  return {
    skills: query.data ?? EMPTY_PROJECT_SKILLS,
    loading: Boolean(id) && query.isPending,
    fetching: query.isFetching,
    error: query.error,
    loadSkills,
  };
}
