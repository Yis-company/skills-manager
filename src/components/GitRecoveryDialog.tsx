import { AlertTriangle, GitBranch, RotateCcw, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import type { GitUpstreamHealth } from "../lib/tauri";
import { cn } from "../utils";

type RecoveryReason = "conflict" | GitUpstreamHealth;

interface Props {
  open: boolean;
  reason: RecoveryReason;
  onClose: () => void;
  onReclone: () => Promise<void>;
}

export function GitRecoveryDialog({ open, reason, onClose, onReclone }: Props) {
  const { t } = useTranslation();
  const [loading, setLoading] = useState<"reclone" | null>(null);

  if (!open) return null;

  // A conflict is already aborted by the backend; re-cloning is the only safe
  // in-app fix, so we hide the "keep local" path for it.
  const isConflict = reason === "conflict";

  const subtitleKey =
    reason === "conflict"
      ? "settings.gitRecoverySubtitleConflict"
      : reason === "unrelated_histories"
        ? "settings.gitRecoverySubtitleUnrelated"
        : reason === "no_upstream"
          ? "settings.gitRecoverySubtitleNoUpstream"
          : "settings.gitRecoverySubtitleDetached";

  const handleReclone = async () => {
    setLoading("reclone");

    try {
      await onReclone();
      onClose();
    } finally {
      setLoading(null);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div
        className="absolute inset-0 bg-black/70 backdrop-blur-sm"
        onClick={() => !loading && onClose()}
      />
      <div className="relative w-full max-w-lg rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-start justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-[14px] font-semibold text-primary">
              <AlertTriangle className="h-4 w-4 text-red-500" />
              {t("settings.gitRecoveryTitle")}
            </h2>
            <p className="mt-1 text-[12px] leading-relaxed text-muted">{t(subtitleKey)}</p>
          </div>
          <button
            onClick={() => !loading && onClose()}
            disabled={!!loading}
            className="rounded p-1 text-muted outline-none transition-colors hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-2">
          <button
            type="button"
            onClick={handleReclone}
            disabled={!!loading}
            className={cn(
              "w-full text-left rounded-md border border-accent bg-accent-bg px-3 py-3 transition-colors outline-none",
              "disabled:cursor-not-allowed disabled:opacity-60 hover:bg-accent-bg/80",
            )}
          >
            <div className="flex items-center gap-2">
              <span className="bg-accent/20 rounded-full p-1 text-accent-light">
                <RotateCcw className="h-4 w-4" />
              </span>
              <span className="text-[13px] font-semibold text-primary">
                {loading === "reclone"
                  ? t("settings.gitRecoveryRecloning")
                  : t("settings.gitRecoveryCardRecloneTitle")}
              </span>
            </div>
            <p className="mt-1.5 pl-7 text-[12px] leading-relaxed text-tertiary">
              {t("settings.gitRecoveryCardRecloneDesc")}
            </p>
          </button>

          {!isConflict && (
            <button
              type="button"
              onClick={() => toast.info(t("settings.gitRecoveryFallbackHint"))}
              disabled={!!loading}
              className="w-full rounded-md border border-border-subtle bg-bg-secondary px-3 py-3 text-left outline-none transition-colors hover:bg-surface-hover disabled:cursor-not-allowed disabled:opacity-60"
            >
              <div className="flex items-center gap-2">
                <span className="rounded-full bg-surface p-1 text-muted">
                  <GitBranch className="h-4 w-4" />
                </span>
                <span className="text-[13px] font-semibold text-primary">
                  {t("settings.gitRecoveryCardKeepLocalTitle")}
                </span>
              </div>
              <p className="mt-1.5 pl-7 text-[12px] leading-relaxed text-tertiary">
                {t("settings.gitRecoveryCardKeepLocalDesc")}
              </p>
            </button>
          )}
        </div>

        <div className="mt-5 flex justify-end">
          <button
            onClick={() => !loading && onClose()}
            disabled={!!loading}
            className="rounded-lg px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none transition-colors hover:bg-surface-hover hover:text-secondary disabled:opacity-50"
          >
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}
