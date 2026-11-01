import { type ReactNode, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { getErrorMessage } from "../lib/error";
import { Modal } from "./Modal";

interface Props {
  open: boolean;
  title?: string;
  message: string;
  details?: string[];
  /** Extra choices shown under the message, such as a checkbox. */
  children?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
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
  children,
  confirmLabel,
  cancelLabel,
  tone = "danger",
  lockWhilePending = false,
  onClose,
  onConfirm,
}: Props) {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(false);

  const submitting = useRef(false);
  const [error, setError] = useState<null | string>(null);

  const close = () => {
    setError(null);
    onClose();
  };

  const handleConfirm = async () => {
    if (submitting.current) return;
    submitting.current = true;
    setError(null);
    setLoading(true);

    try {
      await onConfirm();
      close();
    } catch (error) {
      setError(getErrorMessage(error, t("common.error")));
    } finally {
      submitting.current = false;
      setLoading(false);
    }
  };

  return (
    <Modal
      open={open}
      title={title || t("common.confirm")}
      locked={loading && lockWhilePending}
      onClose={close}
    >
      {error && (
        <p role="alert" className="mb-3 text-[13px] text-red-600 dark:text-red-300">
          {error}
        </p>
      )}
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
      {children ? <div className="mb-5">{children}</div> : null}

      <div className="flex justify-end gap-2">
        <button
          onClick={close}
          disabled={loading && lockWhilePending}
          className="rounded-lg px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none transition-colors hover:bg-surface-hover hover:text-secondary"
        >
          {cancelLabel || t("common.cancel")}
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
    </Modal>
  );
}
