import { X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

interface Props {
  open: boolean;
  currentName: string;
  onClose: () => void;
  onRename: (newName: string) => Promise<void>;
}

export function TagRenameDialog({ open, ...props }: Props) {
  if (!open) return null;

  return <TagRenameDialogContent {...props} />;
}

function TagRenameDialogContent({ currentName, onClose, onRename }: Omit<Props, "open">) {
  const { t } = useTranslation();
  const [name, setName] = useState(currentName);
  const [loading, setLoading] = useState(false);

  const canSave = name.trim().length > 0 && name.trim() !== currentName;

  const handleRename = async () => {
    if (!canSave) return;
    setLoading(true);

    try {
      await onRename(name.trim());
      onClose();
    } catch {
      // The parent surfaces the error (toast) and re-throws; swallow it here so
      // there's no unhandled rejection, and leave the dialog open to retry.
    } finally {
      setLoading(false);
    }
  };

  const inputClass =
    "w-full bg-background border border-border-subtle rounded-lg px-3 py-2 text-[13px] text-secondary focus:outline-none focus:border-border transition-all placeholder-faint";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 z-0 bg-black/70 backdrop-blur-sm" onClick={onClose} />
      <div className="relative z-10 w-full max-w-[400px] rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-[13px] font-semibold text-primary">{t("mySkills.tags.renameTag")}</h2>
          <button
            onClick={onClose}
            className="rounded p-1 text-muted outline-none transition-colors hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-3">
          <div>
            <label className="mb-1 block text-[13px] font-medium text-tertiary">
              {t("mySkills.tags.tagName")}
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className={inputClass}
              autoFocus
              onKeyDown={(e) => e.key === "Enter" && handleRename()}
            />
          </div>
          <div className="flex justify-end gap-2 pt-1">
            <button
              onClick={onClose}
              className="rounded-lg px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none transition-colors hover:bg-surface-hover hover:text-secondary"
            >
              {t("common.cancel")}
            </button>
            <button
              onClick={handleRename}
              disabled={!canSave || loading}
              className="rounded-lg border border-accent-border bg-accent-dark px-3 py-1.5 text-[13px] font-medium text-white outline-none transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
            >
              {loading ? t("common.loading") : t("common.save")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
