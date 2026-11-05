import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { centralRepoPathQueryOptions } from "../lib/appQueries";
import { getErrorKind, getErrorMessage } from "../lib/error";
import { createSkill, type CreateSkillTarget } from "../lib/tauri";

export interface CreateSkillDraft {
  name: string;
  description: string;
  instructions: string;
  agents: string[];
  presetId: string;
  projectId: string;
}

const EMPTY_DRAFT: CreateSkillDraft = {
  name: "",
  description: "",
  instructions: "",
  agents: [],
  presetId: "",
  projectId: "",
};

/** The backend applies the same rule. This only gives feedback while typing. */
const isValidName = (name: string) => name.length <= 64 && /^[a-z0-9]+(-[a-z0-9]+)*$/.test(name);

/**
 * Kept outside the component because the layout remounts every view when the
 * host changes, and typed text shouldn't be lost to that. Targets belong to one
 * host, so they restart empty on each mount.
 */
let keptDraft = EMPTY_DRAFT;

/** The Create tab's draft and submission, held by the Install page across tabs. */
export function useCreateSkill(active: boolean) {
  const { t } = useTranslation();

  const {
    activeHostId,
    refreshManagedSkills,
    refreshPresets,
    refreshProjects,
    openSkillDetailById,
  } = useApp();

  const [draft, setDraft] = useState<CreateSkillDraft>({
    ...keptDraft,
    agents: [],
    presetId: "",
    projectId: "",
  });

  const [submitting, setSubmitting] = useState(false);
  const [nameTaken, setNameTaken] = useState<null | string>(null);
  const [error, setError] = useState<null | string>(null);

  const [partial, setPartial] = useState<{ name: string; failed: CreateSkillTarget[] } | null>(
    null,
  );

  const { data: libraryPath } = useQuery({
    ...centralRepoPathQueryOptions(activeHostId),
    enabled: active,
  });

  // Tiptap keeps the callback from when the editor was created, so this can't
  // read `draft` directly.
  const update = (changes: Partial<CreateSkillDraft>) => {
    keptDraft = { ...keptDraft, ...changes };
    setDraft((current) => ({ ...current, ...changes }));

    if (changes.name !== undefined) setNameTaken(null);
  };

  const nameInvalid = draft.name !== "" && !isValidName(draft.name);

  const canSubmit =
    !submitting &&
    isValidName(draft.name) &&
    draft.description.trim() !== "" &&
    draft.instructions.trim() !== "";

  const submit = async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setError(null);
    setNameTaken(null);
    setPartial(null);

    try {
      const report = await createSkill({
        name: draft.name,
        content: {
          kind: "written",
          description: draft.description,
          instructions: draft.instructions,
        },
        agents: draft.agents,
        preset: draft.presetId || null,
        project: draft.projectId || null,
      });

      keptDraft = EMPTY_DRAFT;
      setDraft(EMPTY_DRAFT);
      void Promise.allSettled([refreshManagedSkills(), refreshPresets(), refreshProjects()]);

      const failed = report.targets.filter((target) => target.error);

      if (failed.length > 0) {
        setPartial({ name: report.name, failed });
      } else {
        toast.success(t("install.create.success", { name: report.name }), {
          action: {
            label: t("install.toast.view"),
            onClick: () => openSkillDetailById(report.skill_id),
          },
        });
      }
    } catch (cause) {
      const message = getErrorMessage(cause, t("common.error"));

      if (getErrorKind(cause) === "skill_exists") setNameTaken(message);
      else setError(message);
    } finally {
      setSubmitting(false);
    }
  };

  return {
    draft,
    update,
    submit,
    submitting,
    canSubmit,
    nameInvalid,
    nameTaken,
    error,
    partial,
    destination: libraryPath
      ? `${libraryPath}/skills/${isValidName(draft.name) ? draft.name : "…"}/SKILL.md`
      : null,
  };
}
