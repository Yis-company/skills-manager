import { X } from "lucide-react";
import { type ReactNode, useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { cn } from "../utils";

/** Shared modal focus, keyboard dismissal, and restoration across desktop webviews. */
export function Modal({
  open,
  title,
  locked = false,
  escapeDisabled = false,
  wide = false,
  onClose,
  children,
}: {
  open: boolean;
  title: string;
  locked?: boolean;
  escapeDisabled?: boolean;
  wide?: boolean;
  onClose: () => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement;
    const dialog = ref.current;
    dialog?.querySelector<HTMLElement>("button:not([disabled])")?.focus();

    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, [open]);

  if (!open) return null;

  return createPortal(
    <div
      className="fixed inset-0 z-[80] flex items-center justify-center bg-black/60 p-4"
      onClick={(event) => {
        event.stopPropagation();

        if (event.target === event.currentTarget && !locked) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        ref={ref}
        aria-labelledby={titleId}
        aria-busy={locked}
        onKeyDown={(event) => {
          event.stopPropagation();

          // Escape cancels IME composition without dismissing its dialog.
          if (event.nativeEvent.isComposing) return;

          if (event.key === "Escape") {
            event.stopPropagation();
            event.preventDefault();

            if (!locked && !escapeDisabled) onClose();
          }

          if (event.key !== "Tab") return;
          event.stopPropagation();

          const controls = Array.from(
            event.currentTarget.querySelectorAll<HTMLElement>(
              "button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
            ),
          ).filter((element) => element.getClientRects().length > 0);

          const first = controls[0];
          const last = controls[controls.length - 1];

          if (!first) {
            event.preventDefault();
            event.currentTarget.focus();
          } else if (event.shiftKey && document.activeElement === first) {
            event.preventDefault();
            last.focus();
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault();
            first.focus();
          }
        }}
        onClick={(event) => event.stopPropagation()}
        className={cn(
          "max-h-[calc(90vh/var(--app-scale))] w-full overflow-auto rounded-xl border border-border bg-surface p-5 text-primary",
          wide ? "max-w-4xl" : "max-w-sm",
        )}
      >
        <header className="mb-4 flex items-start justify-between gap-3">
          <h2 id={titleId} className="min-w-0 break-words text-[14px] font-semibold">
            {title}
          </h2>
          <button
            type="button"
            onClick={onClose}
            disabled={locked}
            aria-label={t("common.close")}
            className="shrink-0 rounded p-1 text-muted hover:text-primary"
          >
            <X className="h-4 w-4" />
          </button>
        </header>
        {open && children}
      </div>
    </div>,
    document.body,
  );
}
