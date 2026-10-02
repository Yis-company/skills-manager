import { X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

interface Props {
  open: boolean;
  onCancel: () => void;
  onClose: (remember: boolean) => void;
  onHide: (remember: boolean) => void;
}

export function CloseActionDialog({ open, onCancel, onClose, onHide }: Props) {
  const { t } = useTranslation();
  const [remember, setRemember] = useState(false);

  const handleCancel = () => {
    setRemember(false);
    onCancel();
  };

  const handleClose = () => {
    onClose(remember);
    setRemember(false);
  };

  const handleHide = () => {
    onHide(remember);
    setRemember(false);
  };

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={handleCancel} />
      <div className="relative w-full max-w-sm rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-[13px] font-semibold text-primary">{t("closeAction.title")}</h2>
          <button
            onClick={handleCancel}
            className="rounded p-1 text-muted outline-none transition-colors hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <p className="mb-4 text-[13px] text-tertiary">{t("closeAction.message")}</p>

        <label className="mb-5 flex cursor-pointer select-none items-center gap-2">
          <input
            type="checkbox"
            checked={remember}
            onChange={(e) => setRemember(e.target.checked)}
            className="h-3.5 w-3.5 accent-[var(--color-accent)]"
          />
          <span className="text-[13px] text-muted">{t("closeAction.remember")}</span>
        </label>

        <div className="flex justify-end gap-2">
          <button
            onClick={handleClose}
            className="rounded-lg px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none transition-colors hover:bg-surface-hover hover:text-secondary"
          >
            {t("closeAction.close")}
          </button>
          <button
            onClick={handleHide}
            className="rounded-lg border border-accent-border bg-accent-dark px-3 py-1.5 text-[13px] font-medium text-white outline-none transition-colors hover:bg-accent"
          >
            {t("closeAction.hide")}
          </button>
        </div>
      </div>
    </div>
  );
}
