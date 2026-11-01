import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import type { ChangeReview } from "../lib/changeReview";
import { getErrorMessage } from "../lib/error";
import * as api from "../lib/tauri";
import { ConfirmDialog } from "./ConfirmDialog";
import { Modal } from "./Modal";
import { SkillSourceDiffViewer } from "./SkillSourceDiffViewer";

export interface ReviewChoice {
  label: string;
  command: string;
  args: api.SyncCallArgs;
}

export interface ChangeReviewTarget {
  kind: "edited" | "preset" | "resource" | "skill";
  key: string;
  tool?: string;
  name: string;
  description?: string;
  choices: ReviewChoice[];
}

export function ChangeReviewDialog({
  hostId,
  target,
  onClose,
  onApplied,
}: {
  hostId: null | string;
  target: ChangeReviewTarget;
  onClose: () => void;
  onApplied: () => Promise<void>;
}) {
  const { t } = useTranslation();

  const query = useQuery({
    queryKey: ["change-review", hostId, target.kind, target.key, target.tool ?? null],
    queryFn: () =>
      api.syncCall<ChangeReview>(hostId, "sync_review", {
        kind: target.kind,
        key: target.key,
        tool: target.tool ?? null,
      }),
    retry: false,
  });

  const review = query.isFetching ? null : query.data;
  const error = query.error ? getErrorMessage(query.error, t("review.unavailable")) : null;
  const [choice, setChoice] = useState<null | ReviewChoice>(null);

  return (
    <Modal open title={t("review.title", { name: target.name })} wide onClose={onClose}>
      <p className="mb-4 text-[13px] text-secondary">{t("review.explanation")}</p>
      {error && (
        <div role="alert" className="mb-4 text-[13px] text-red-600 dark:text-red-300">
          <p>{t("review.unavailable")}</p>
          <p>{error}</p>
        </div>
      )}
      {!review && !error && <p role="status">{t("common.loading")}</p>}
      <button
        type="button"
        className="app-button-secondary mb-4"
        onClick={() => {
          setChoice(null);
          void query.refetch();
        }}
      >
        {t("review.refresh")}
      </button>
      {review && (
        <>
          <div className="mb-3 grid grid-cols-2 gap-3 text-[13px] font-medium">
            <p className="min-w-0 break-words">
              {t("review.original", { name: review.left_label })}
            </p>
            <p className="min-w-0 break-words">
              {t("review.alternative", { name: review.right_label })}
            </p>
          </div>
          <div className="min-w-0 overflow-x-auto">
            <SkillSourceDiffViewer entries={review.files} comparison />
          </div>
          <footer className="mt-5 flex flex-wrap justify-end gap-2">
            {target.choices.map((item) => (
              <button
                type="button"
                key={item.label}
                className="app-button-secondary"
                onClick={() => setChoice(item)}
              >
                {item.label}
              </button>
            ))}
          </footer>
        </>
      )}
      <ConfirmDialog
        open={choice !== null}
        title={choice?.label}
        message={t(target.kind === "edited" ? "review.editedOutcome" : "review.outcome")}
        confirmLabel={t("review.apply")}
        tone="warning"
        lockWhilePending
        onClose={() => setChoice(null)}
        onConfirm={async () => {
          if (!choice || !review) return;
          await api.syncCall(hostId, choice.command, {
            ...choice.args,
            review: JSON.stringify(review),
          });
          toast.success(
            t(target.kind === "edited" ? "review.editedApplied" : "review.applied", {
              choice: choice.label,
              name: target.name,
            }),
          );
          await onApplied();
          onClose();
        }}
      />
    </Modal>
  );
}
