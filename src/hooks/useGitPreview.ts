import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { useOperations } from "../context/OperationsContext";
import { getErrorMessage } from "../lib/error";

/** A skill found in a previewed repo, and whether and as what to install it. */
export type GitSelection = {
  rel_path: string;
  name: string;
  description: null | string;
  selected: boolean;
};

/** The backend owns preparation and its preview; this hook owns only the open review. */
export function useGitPreview() {
  const { t } = useTranslation();
  const { activeHostId } = useApp();
  const { operations, act } = useOperations();
  const [gitUrl, setGitUrl] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [reviewId, setReviewId] = useState<null | string>(null);
  const [gitSelections, setGitSelections] = useState<GitSelection[]>([]);
  const [gitConfirmLoading, setGitConfirmLoading] = useState(false);
  const operation = operations.find((o) => o.operation_id === reviewId);

  const reviewOperation = (id: string) => {
    const preview = operations.find((o) => o.operation_id === id)?.preview;

    if (!preview) return;
    setGitSelections(preview.map((skill) => ({ ...skill, selected: true })));
    setReviewId(id);
  };

  const handleGitPreview = async () => {
    if (!gitUrl.trim() || submitting) return;
    setSubmitting(true);

    try {
      await act(activeHostId, {
        action: "submit_install",
        operation_id: crypto.randomUUID(),
        host_id: activeHostId,
        sources: [{ kind: "git", url: gitUrl.trim() }],
      });
      setGitUrl("");
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    } finally {
      setSubmitting(false);
    }
  };

  const handleGitConfirm = async () => {
    if (!operation?.preview || gitConfirmLoading) return;

    const items = gitSelections
      .filter((s) => s.selected)
      .map((s) => ({ rel_path: s.rel_path, name: s.name }));

    if (!items.length) return;
    setGitConfirmLoading(true);

    try {
      await act(operation.host_id, {
        action: "confirm_install",
        operation_id: operation.operation_id,
        attempt_id: crypto.randomUUID(),
        items,
      });
      setReviewId(null);
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    } finally {
      setGitConfirmLoading(false);
    }
  };

  return {
    gitUrl,
    setGitUrl,
    gitLoading: submitting,
    gitPreview: operation?.preview ?? null,
    reviewIsGit: [
      ...(operation?.sources ?? []),
      ...(operation?.items.flatMap((i) => (i.source ? [i.source] : [])) ?? []),
    ].some((s) => s.kind === "git"),
    gitSelections,
    setGitSelections,
    gitConfirmLoading,
    handleGitPreview,
    handleGitConfirm,
    handleGitPreviewClose: () => setReviewId(null),
    reviewOperation,
  };
}
