import { AlertTriangle, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

interface Props {
  open: boolean;
  title?: string;
  message: string;
  details?: string[];
  confirmLabel?: string;
  tone?: "danger" | "warning";
  lockWhilePending?: boolean;
  onClose: () => void;
  onConfirm: () => Promise<void>;
}

export function ConfirmDialog({
  open,
  title,
  message,
  details,
  confirmLabel,
  tone = "danger",
  lockWhilePending = false,
  onClose,
  onConfirm,
}: Props) {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(false);

  // Escape closes the dialog, except while the confirmed action is running.
  useEffect(() => {
    if (!open || loading) return;

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };

    window.addEventListener("keydown", onKey);

    return () => window.removeEventListener("keydown", onKey);
  }, [open, loading, onClose]);

  if (!open) return null;

  const handleConfirm = async () => {
    setLoading(true);

    try {
      await onConfirm();
      onClose();
    } finally {
      setLoading(false);
    }
  };

  return (
    // The panel is capped so the footer buttons stay reachable (#430). The cap
    // divides by --app-scale because the text-size setting applies `zoom` to
    // <html> and zoom does not scale vh: a bare 85vh renders at 102% of the
    // viewport on the largest size. Same compensation as html/body in index.css.
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div
        className="absolute inset-0 bg-black/70 backdrop-blur-sm"
        onClick={loading && lockWhilePending ? undefined : onClose}
      />
      <div className="relative flex max-h-[calc(85vh/var(--app-scale))] w-full max-w-sm flex-col rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-[13px] font-semibold text-primary">
            <AlertTriangle className="h-4 w-4 text-amber-400" />
            {title || t("common.confirm")}
          </h2>
          <button
            onClick={onClose}
            disabled={loading && lockWhilePending}
            className="rounded p-1 text-muted outline-none transition-colors hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <p className="mb-5 text-[13px] text-tertiary">{message}</p>
        {details && details.length > 0 ? (
          <div className="mb-5 flex min-h-0 flex-wrap gap-2 overflow-y-auto">
            {details.map((detail) => (
              <span
                key={detail}
                className="rounded-full border border-border-subtle bg-bg-secondary px-2.5 py-1 text-[13px] text-secondary"
              >
                {detail}
              </span>
            ))}
          </div>
        ) : null}

        <div className="flex justify-end gap-2">
          <button
            onClick={onClose}
            disabled={loading && lockWhilePending}
            className="rounded-lg px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none transition-colors hover:bg-surface-hover hover:text-secondary"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={handleConfirm}
            disabled={loading}
            className={
              tone === "warning"
                ? "rounded-lg border border-accent-border bg-accent-dark px-3 py-1.5 text-[13px] font-medium text-white outline-none transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
                : "rounded-lg border border-red-500/50 bg-red-600/90 px-3 py-1.5 text-[13px] font-medium text-white outline-none transition-colors hover:bg-red-500 disabled:cursor-not-allowed disabled:opacity-50"
            }
          >
            {loading ? t("common.loading") : confirmLabel || t("common.delete")}
          </button>
        </div>
      </div>
    </div>
  );
}
