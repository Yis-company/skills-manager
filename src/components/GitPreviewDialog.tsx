import { DownloadCloud, Loader2 } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import { useTranslation } from "react-i18next";

import type { GitSelection } from "../hooks/useGitPreview";
import { cn } from "../utils";
import { Modal } from "./Modal";

interface GitPreviewDialogProps {
  selections: GitSelection[];
  repository?: boolean;
  setSelections: Dispatch<SetStateAction<GitSelection[]>>;
  /** Install of the selection in flight; locks the dialog. */
  confirmLoading: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

/** Pick and name the skills to install from a previewed git repo. */
export function GitPreviewDialog({
  selections,
  repository = true,
  setSelections,
  confirmLoading,
  onClose,
  onConfirm,
}: GitPreviewDialogProps) {
  const { t } = useTranslation();

  return (
    <Modal open title={t("install.gitPreview.title")} locked={confirmLoading} onClose={onClose}>
      <p className="mb-3 text-[13px] text-muted">
        {t(repository ? "install.gitPreview.description" : "operations.reviewDescription")}
      </p>

      {/* Select all / deselect all */}
      <div className="mb-2 flex gap-2">
        <button
          type="button"
          onClick={() => setSelections((prev) => prev.map((s) => ({ ...s, selected: true })))}
          disabled={confirmLoading}
          className="text-[13px] text-accent-light hover:underline"
        >
          {t("install.gitPreview.selectAll")}
        </button>
        <span className="text-faint">·</span>
        <button
          type="button"
          onClick={() => setSelections((prev) => prev.map((s) => ({ ...s, selected: false })))}
          disabled={confirmLoading}
          className="text-[13px] text-muted hover:underline"
        >
          {t("install.gitPreview.deselectAll")}
        </button>
      </div>

      {selections.length === 0 ? (
        <p className="py-6 text-center text-[13px] text-muted">{t("install.gitPreview.empty")}</p>
      ) : (
        <div className="scrollbar-hide max-h-64 space-y-2 overflow-y-auto pr-1">
          {selections.map((item, idx) => (
            <div
              key={item.rel_path}
              className={cn(
                "flex items-center gap-3 rounded-lg border px-3 py-2 transition-colors",
                item.selected
                  ? "border-accent-border bg-accent-bg/40"
                  : "border-border-subtle bg-background opacity-50",
              )}
            >
              <input
                type="checkbox"
                checked={item.selected}
                disabled={confirmLoading}
                onChange={(e) =>
                  setSelections((prev) =>
                    prev.map((s, i) => (i === idx ? { ...s, selected: e.target.checked } : s)),
                  )
                }
                className="h-4 w-4 shrink-0 accent-accent"
              />
              <div className="min-w-0 flex-1">
                <input
                  type="text"
                  value={item.name}
                  onChange={(e) =>
                    setSelections((prev) =>
                      prev.map((s, i) => (i === idx ? { ...s, name: e.target.value } : s)),
                    )
                  }
                  disabled={!item.selected || confirmLoading}
                  placeholder={t("install.gitPreview.namePlaceholder")}
                  className="app-input w-full bg-background py-1 text-[13px]"
                />
                {item.description ? (
                  <p className="mt-1 truncate text-[12px] text-muted">{item.description}</p>
                ) : null}
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="mt-4 flex justify-end gap-2">
        <button
          type="button"
          onClick={onClose}
          disabled={confirmLoading}
          className="px-3 py-1.5 text-[13px] font-medium text-muted transition-colors hover:text-secondary"
        >
          {t("common.cancel")}
        </button>
        <button
          type="button"
          onClick={onConfirm}
          disabled={confirmLoading || selections.every((s) => !s.selected)}
          className="app-button-primary"
        >
          {confirmLoading ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <DownloadCloud className="h-3.5 w-3.5" />
          )}
          {t("install.gitPreview.confirm")}
        </button>
      </div>
    </Modal>
  );
}
