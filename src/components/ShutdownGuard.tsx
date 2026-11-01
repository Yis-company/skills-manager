import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useOperations } from "../context/OperationsContext";
import { useWorkspaceChanges } from "../context/WorkspaceChangesContext";
import { shutdownRequest, type ShutdownRequest } from "../lib/shutdown";
import { Modal } from "./Modal";

/** The native boundary owns exit authority; this presents the user's choice. */
export function ShutdownGuard() {
  const { t } = useTranslation();
  const { allOperations, prepareShutdown, releaseShutdown, flush } = useOperations();
  const { confirmLeave } = useWorkspaceChanges();
  const [pending, setPending] = useState<null | ShutdownRequest>(null);
  const handling = useRef(false);
  const current = useRef(pending);
  const pumping = useRef(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState(false);

  const latest = useRef({
    prepareShutdown,
    releaseShutdown,
    flush,
    confirmLeave,
    allOperations,
    error,
  });

  useEffect(() => {
    latest.current = {
      prepareShutdown,
      releaseShutdown,
      flush,
      confirmLeave,
      allOperations,
      error,
    };
  }, [prepareShutdown, releaseShutdown, flush, confirmLeave, allOperations, error]);

  const keepOpen = useCallback(
    async (id: number) => {
      current.current = null;
      setPending(null);
      setError(false);
      setSaving(false);
      releaseShutdown();
      await shutdownRequest({ action: "keep_open", id });
    },
    [releaseShutdown],
  );

  useEffect(() => {
    const begin = async (request: null | ShutdownRequest) => {
      if (!request || handling.current || current.current?.id === request.id) return;
      handling.current = true;

      try {
        if (!(await confirmLeave())) {
          await shutdownRequest({ action: "keep_open", id: request.id });

          return;
        }

        const inspected = await shutdownRequest<{ unfinished: boolean }>({ action: "inspect" });

        if (
          !request.mode &&
          !inspected.unfinished &&
          !latest.current.allOperations.some(
            (o) => !o.settled || o.admission || o.recovery === "checking",
          )
        ) {
          request = await shutdownRequest<ShutdownRequest>({
            action: "choose",
            id: request.id,
            mode: "drain",
          });
        }

        current.current = request;
        setPending(request);
      } finally {
        handling.current = false;
      }
    };

    const unlisten = listen<ShutdownRequest>(
      "shutdown-requested",
      ({ payload }) => void begin(payload),
    );

    void shutdownRequest<null | ShutdownRequest>({ action: "read" }).then(begin);

    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [confirmLeave]);

  const choose = async (mode: "cancel" | "drain") => {
    if (!pending) return;

    const request = await shutdownRequest<ShutdownRequest>({
      action: "choose",
      id: pending.id,
      mode,
    });

    if (current.current?.id === request.id) {
      current.current = request;
      setPending(request);
    }
  };

  useEffect(() => {
    if (!pending?.mode) return;
    const id = pending.id;

    const pump = async () => {
      if (pumping.current || latest.current.error || current.current?.id !== id) return;
      pumping.current = true;

      try {
        const prepared = await latest.current.prepareShutdown(
          id,
          current.current?.mode === "cancel",
        );

        if (!prepared.ready || current.current?.id !== id) return;
        setSaving(true);

        if (!(await latest.current.confirmLeave())) {
          await keepOpen(id);

          return;
        }

        await latest.current.flush();

        if (current.current?.id !== id) return;
        await shutdownRequest({ action: "finish", id });
        current.current = null;
        setPending(null);
      } catch {
        if (current.current?.id !== id) return;
        setError(true);
        latest.current.releaseShutdown();
        await shutdownRequest({ action: "resume", id }).catch(() => {});
      } finally {
        setSaving(false);
        pumping.current = false;
      }
    };

    void pump();
    const timer = setInterval(() => void pump(), 300);

    return () => clearInterval(timer);
  }, [pending?.id, pending?.mode, keepOpen]);

  return (
    <>
      <Modal
        open={!!pending && !pending.mode}
        title={t("shutdown.title")}
        onClose={() => {
          if (pending) void keepOpen(pending.id);
        }}
      >
        <p className="mb-5 text-[13px] text-muted">{t("shutdown.description")}</p>
        <div className="flex flex-col gap-2">
          <button
            className="app-button-secondary w-full whitespace-normal"
            onClick={() => {
              if (pending) void keepOpen(pending.id);
            }}
          >
            {t("shutdown.keepOpen")}
          </button>
          <button
            className="app-button-secondary w-full whitespace-normal text-danger"
            onClick={() => void choose("cancel")}
          >
            {t("shutdown.cancelQuit")}
          </button>
          <button
            className="app-button-primary w-full whitespace-normal"
            onClick={() => void choose("drain")}
          >
            {t("shutdown.autoQuit")}
          </button>
        </div>
      </Modal>
      <Modal
        open={saving && !!pending}
        title={t("shutdown.saving")}
        onClose={() => {
          if (pending) void keepOpen(pending.id);
        }}
      >
        <p className="mb-5 text-[13px] text-muted">{t("shutdown.savingDescription")}</p>
        <button
          className="app-button-secondary w-full"
          onClick={() => {
            if (pending) void keepOpen(pending.id);
          }}
        >
          {t("shutdown.keepOpen")}
        </button>
      </Modal>
      {pending?.mode && !saving && (
        <aside
          role="status"
          aria-label={t("shutdown.pending")}
          className="fixed bottom-4 right-4 z-50 flex max-w-sm flex-wrap items-center gap-3 rounded-lg border border-border bg-surface p-4 text-[13px] text-primary"
        >
          <div className="min-w-0 flex-1">
            <p>
              {t(
                pending.target === "restart"
                  ? "shutdown.willRestart"
                  : pending.target === "install_update"
                    ? "shutdown.willUpdate"
                    : "shutdown.willQuit",
              )}
            </p>
            <p role={error ? "alert" : undefined}>
              {error
                ? t("shutdown.saveError")
                : saving
                  ? t("shutdown.saving")
                  : t(pending.mode === "drain" ? "shutdown.draining" : "shutdown.canceling", {
                      count: allOperations.filter((o) => !o.settled || o.admission).length,
                    })}
            </p>
          </div>
          {error && (
            <button className="app-button-primary" onClick={() => setError(false)}>
              {t(
                pending.target === "restart"
                  ? "shutdown.retryRestart"
                  : pending.target === "install_update"
                    ? "shutdown.retryUpdate"
                    : "shutdown.retry",
              )}
            </button>
          )}
          <button className="app-button-secondary" onClick={() => void keepOpen(pending.id)}>
            {t("shutdown.keepOpen")}
          </button>
        </aside>
      )}
    </>
  );
}
