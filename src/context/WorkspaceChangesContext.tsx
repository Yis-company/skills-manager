/* eslint-disable react-refresh/only-export-components */
import { useBlocker } from "@tanstack/react-router";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";

import { Modal } from "../components/Modal";
import {
  applyWorkspaceChanges,
  type ReviewedWorkspaceChange,
  type WorkspaceChange,
} from "../lib/workspaceChanges";

interface ChangesContext {
  changes: WorkspaceChange[];
  busy: boolean;
  stage: (change: WorkspaceChange) => void;
  discard: () => void;
  confirmLeave: () => Promise<boolean>;
  review: () => Promise<void>;
}

const Context = createContext<ChangesContext | null>(null);

export function useWorkspaceChanges() {
  const value = useContext(Context);

  if (!value) throw new Error("WorkspaceChangesProvider is missing");

  return value;
}

export function WorkspaceChangesProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [changes, setChanges] = useState<WorkspaceChange[]>([]);
  const [prepared, setPrepared] = useState<null | ReviewedWorkspaceChange[]>(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const trayRef = useRef<HTMLElement>(null);
  const [error, setError] = useState("");
  const [result, setResult] = useState("");
  const [acknowledged, setAcknowledged] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const answer = useRef<((value: boolean) => void) | null>(null);

  const settleLeave = (value: boolean) => {
    if (value) {
      setChanges([]);
      setPrepared(null);
      setError("");
    }

    answer.current?.(value);
    answer.current = null;
    setLeaving(false);
  };

  const confirmLeave = useCallback(async () => {
    if (inFlight.current) return false;

    if (!changes.length) return true;

    if (answer.current) return false;

    return new Promise<boolean>((resolve) => {
      answer.current = resolve;
      setLeaving(true);
    });
  }, [changes.length]);

  useEffect(() => () => answer.current?.(false), []);
  useBlocker({
    shouldBlockFn: async () => !(await confirmLeave()),
    enableBeforeUnload: () => changes.length > 0,
  });

  const review = async () => {
    if (inFlight.current || !changes.length) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    setResult("");

    try {
      const next: ReviewedWorkspaceChange[] = [];

      for (const change of changes) next.push({ ...change, prepared: await change.prepare() });
      setAcknowledged(false);
      setPrepared(next);
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  const apply = async () => {
    if (inFlight.current || !prepared) return;
    inFlight.current = true;
    setBusy(true);
    setError("");

    try {
      const outcome = await applyWorkspaceChanges(prepared);
      setChanges((current) => current.filter((change) => !outcome.succeeded.includes(change.id)));
      setPrepared(null);
      setResult(t("workspace.applied", { count: outcome.succeeded.length }));
      setError(outcome.failed.map((item) => `${item.name}: ${item.error}`).join("\n"));
      window.dispatchEvent(new Event("workspace-targets-changed"));
    } catch (error) {
      // Refresh actual presence without changing the retained user intent.
      window.dispatchEvent(new Event("workspace-targets-changed"));
      setPrepared(null);
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  useEffect(() => {
    if (result || error) trayRef.current?.focus();
  }, [result, error]);

  const warnings = prepared?.flatMap((change) => change.prepared.warnings) ?? [];

  return (
    <Context.Provider
      value={{
        changes,
        busy,
        confirmLeave,
        review,
        stage: (change) => {
          if (inFlight.current) return;
          setPrepared(null);
          setError("");
          setResult("");
          setChanges((current) =>
            current.some((item) => item.id === change.id)
              ? current.filter((item) => item.id !== change.id)
              : [...current, change],
          );
        },
        discard: () => {
          if (!inFlight.current) {
            setChanges([]);
            setPrepared(null);
            setError("");
          }
        },
      }}
    >
      {children}
      {(changes.length > 0 || error || result) && (
        <aside
          className="workspace-change-tray"
          aria-label={t("workspace.stagedChanges")}
          ref={trayRef}
          tabIndex={-1}
        >
          <div className="min-w-0 flex-1">
            {changes.length > 0 && (
              <p className="font-medium">{t("workspace.stagedCount", { count: changes.length })}</p>
            )}
            {changes.length > 0 && (
              <p className="text-[12px] text-muted">
                {Array.from(new Set(changes.map((change) => change.destination))).join(" · ")}
              </p>
            )}
            {result && (
              <p role="status" className="text-[12px]">
                {result}
              </p>
            )}
            {error && (
              <p role="alert" className="whitespace-pre-wrap text-[12px] text-danger">
                {error}
              </p>
            )}
          </div>
          {changes.length > 0 ? (
            <div className="flex shrink-0 gap-2">
              <button
                className="app-button-secondary"
                disabled={busy}
                onClick={() => {
                  setChanges([]);
                  setPrepared(null);
                  setError("");
                }}
              >
                {t("workspace.discard")}
              </button>
              <button className="app-button-primary" disabled={busy} onClick={() => void review()}>
                {busy ? t("common.loading") : t("workspace.reviewChanges")}
              </button>
            </div>
          ) : (
            <button
              className="app-button-secondary"
              onClick={() => {
                setResult("");
                setError("");
              }}
            >
              {t("common.close")}
            </button>
          )}
        </aside>
      )}
      <Modal open={leaving} title={t("workspace.leaveTitle")} onClose={() => settleLeave(false)}>
        <p className="mb-5 text-[13px] text-muted">
          {t("workspace.leaveDescription", { count: changes.length })}
        </p>
        <div className="flex justify-end gap-2">
          <button className="app-button-secondary" onClick={() => settleLeave(false)}>
            {t("workspace.keepEditing")}
          </button>
          <button className="app-button-secondary text-danger" onClick={() => settleLeave(true)}>
            {t("workspace.discardContinue")}
          </button>
        </div>
      </Modal>
      <Modal
        open={prepared !== null}
        wide
        title={t("workspace.reviewChanges")}
        locked={busy}
        onClose={() => setPrepared(null)}
      >
        <p className="mb-4 text-[13px] text-muted">{t("workspace.reviewDescription")}</p>
        <ul className="divide-y divide-border-subtle">
          {prepared?.map((change) => (
            <li
              key={change.id}
              className="flex flex-wrap items-center justify-between gap-3 py-3 text-[13px]"
            >
              <div>
                <strong>{change.name}</strong>
                <p className="text-[12px] text-muted">
                  {change.agent} · {change.destination}
                </p>
              </div>
              <span>{t(change.add ? "workspace.addToAgent" : "workspace.removeFromAgent")}</span>
            </li>
          ))}
        </ul>
        {warnings.length > 0 && (
          <div className="my-4 space-y-2 rounded-lg bg-bg-secondary p-3 text-[13px]">
            <ul className="list-inside list-disc">
              {Array.from(new Set(warnings)).map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
            <label className="flex items-start gap-2">
              <input
                type="checkbox"
                checked={acknowledged}
                onChange={(event) => setAcknowledged(event.target.checked)}
                disabled={busy}
              />
              {t("workspace.acknowledge")}
            </label>
          </div>
        )}
        <div className="mt-5 flex justify-end gap-2">
          <button
            className="app-button-secondary"
            disabled={busy}
            onClick={() => setPrepared(null)}
          >
            {t("common.cancel")}
          </button>
          <button
            className="app-button-primary"
            disabled={busy || (warnings.length > 0 && !acknowledged)}
            onClick={() => void apply()}
          >
            {busy ? t("common.loading") : t("workspace.applyReviewed")}
          </button>
        </div>
      </Modal>
    </Context.Provider>
  );
}
