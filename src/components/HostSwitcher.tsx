import { useNavigate } from "@tanstack/react-router";
import { Check, ChevronsUpDown, Loader2, Monitor, Server, Settings2, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useApp } from "../context/AppContext";
import { useOperations } from "../context/OperationsContext";
import { getActiveHostId, invokeHost } from "../lib/hostCall";
import {
  isLibraryOperation,
  operationHostMatches,
  operationIdentity,
  operationNeedsAttention,
  operationNeedsSession,
  type OperationSnapshot,
} from "../lib/operations";
import type { Project } from "../lib/tauri";
import { cn } from "../utils";
import { settingsLink } from "../views/settings/categories";
import { RestoredOperationDialog } from "./RestoredOperationDialog";

/** Sidebar control choosing which machine the whole app operates on. */
export function HostSwitcher() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { remoteHosts, activeHost, hostSession, connectingHostId, switchHost } = useApp();

  const { allOperations, unavailableHosts, refreshErrors, persistenceStatus, flush } =
    useOperations();

  const [open, setOpen] = useState(false);
  const [restoredContext, setRestoredContext] = useState<null | OperationSnapshot>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;

    const handlePointer = (e: MouseEvent) => {
      if (
        containerRef.current &&
        !(e.target instanceof Node && containerRef.current.contains(e.target))
      )
        setOpen(false);
    };

    const handleEscape = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      setOpen(false);
      triggerRef.current?.focus();
    };

    document.addEventListener("mousedown", handlePointer);
    document.addEventListener("keydown", handleEscape);

    return () => {
      document.removeEventListener("mousedown", handlePointer);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [open]);

  const connectingHost = remoteHosts.find((host) => host.id === connectingHostId);
  const lost = hostSession?.lostMessage != null;
  const current = activeHost?.id ?? null;

  const needsAttention = (operation: OperationSnapshot) =>
    operationNeedsAttention(operation) || Boolean(refreshErrors[operationIdentity(operation)]);

  const hostWork = (hostId: null | string) =>
    allOperations.filter(
      (operation) =>
        operation.host_id === hostId &&
        (operationNeedsSession(operation) || needsAttention(operation)),
    );

  const summary = (hostId: null | string) => {
    const work = hostWork(hostId);
    const active = work.filter((operation) => !operation.settled).length;
    const attention = work.filter(needsAttention).length;

    return [
      work.some((o) => o.recovery === "checking")
        ? t("operations.connectionLost")
        : active
          ? t("operations.installNav", { count: active })
          : "",
      work.some((o) => o.recovery === "unknown") ? t("operations.phase.outcome_unknown") : "",
      attention ? t("operations.installAttention", { count: attention }) : "",
      unavailableHosts.includes(hostId) ? t("operations.hostUnavailable") : "",
    ]
      .filter(Boolean)
      .join(" · ");
  };

  const shortcuts = [
    ...new Set([
      null,
      ...remoteHosts.map((host) => host.id),
      ...allOperations.map((o) => o.host_id),
    ]),
  ]
    .filter((id) => id !== current)
    .flatMap((id) => {
      const work = hostWork(id).filter((o) => !o.restored);
      const operation = work.find(needsAttention) ?? work[0];

      if (!operation) return [];

      const host =
        operation.origin?.host_name ??
        remoteHosts.find((host) => host.id === id)?.name ??
        id ??
        t("hostSwitcher.local");

      const destination = isLibraryOperation(operation)
        ? t("sidebar.installSkills")
        : (operation.project_path?.split(/[\\/]/).filter(Boolean).at(-1) ?? operation.project_id);

      return [{ id, operation, label: `${host} · ${destination} · ${summary(id)}` }];
    });

  for (const operation of allOperations.filter((o) => o.restored)) {
    const destination = isLibraryOperation(operation)
      ? t("sidebar.installSkills")
      : (operation.origin?.project_name ?? operation.project_path ?? operation.project_id);

    const host = operation.origin?.host_name ?? operation.host_id ?? t("operations.hostLocal");
    shortcuts.push({
      id: operation.host_id,
      operation,
      label: `${host} · ${destination} · ${operation.recovery === "unknown" ? t("operations.phase.outcome_unknown") : t("operations.installAttention", { count: 1 })}`,
    });
  }

  // Stays usable while a switch connects: picking another host replaces the
  // switch, and picking the current one cancels it.
  const choose = (hostId: null | string) => {
    setOpen(false);

    if (connectingHostId === null ? hostId === current && !lost : hostId === connectingHostId) {
      return;
    }

    void switchHost(hostId);
  };

  const option = (
    key: string,
    hostId: null | string,
    icon: typeof Monitor,
    label: string,
    detail: string,
    monoDetail: boolean,
  ) => {
    const Icon = icon;
    const selected = hostId === current;
    const connecting = hostId !== null && hostId === connectingHostId;

    return (
      <button
        key={key}
        type="button"
        role="menuitemradio"
        aria-checked={selected}
        onClick={() => choose(hostId)}
        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-secondary transition-colors hover:bg-surface-hover"
      >
        <Icon className="h-3.5 w-3.5 shrink-0 text-muted" />
        <span className="min-w-0 flex-1">
          <span className="block truncate">{label}</span>
          <span className={cn("block truncate text-[11px] text-faint", monoDetail && "font-mono")}>
            {detail}
          </span>
          {summary(hostId) && (
            <span className="block text-[11px] text-secondary">{summary(hostId)}</span>
          )}
        </span>
        {connecting ? (
          <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-muted" />
        ) : selected ? (
          <Check className="h-3.5 w-3.5 shrink-0 text-accent" />
        ) : null}
      </button>
    );
  };

  return (
    <div ref={containerRef} className="relative shrink-0 px-2.5 pb-2">
      <button
        ref={triggerRef}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "flex w-full items-center gap-2 rounded-md border px-2.5 py-1.5 text-left text-[13px] transition-colors outline-none focus-visible:ring-2 focus-visible:ring-border",
          activeHost
            ? "border-accent-border bg-accent-bg text-primary"
            : "border-border-subtle bg-surface text-secondary hover:bg-surface-hover",
        )}
      >
        {connectingHost ? (
          <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-muted" />
        ) : activeHost ? (
          <Server className={cn("h-3.5 w-3.5 shrink-0", lost ? "text-amber-500" : "text-accent")} />
        ) : (
          <Monitor className="h-3.5 w-3.5 shrink-0 text-muted" />
        )}
        <span className="min-w-0 flex-1 truncate">
          {connectingHost
            ? t("hostSwitcher.connecting", { name: connectingHost.name })
            : (activeHost?.name ?? t("hostSwitcher.local"))}
        </span>
        <ChevronsUpDown className="h-3.5 w-3.5 shrink-0 text-faint" />
      </button>

      {persistenceStatus === "error" && (
        <div className="mt-2 space-y-1 px-2.5 text-[12px] text-secondary" role="alert">
          <p>{t("operations.saveFailed")}</p>
          <button
            className="app-button-secondary"
            onClick={() => {
              void flush().catch(() => {});
            }}
          >
            {t("operations.retrySaving")}
          </button>
        </div>
      )}
      {shortcuts.length > 0 && (
        <div className="mt-1 space-y-1" aria-label={t("operations.otherHosts")}>
          {shortcuts.map(({ id, operation, label }) => (
            <button
              key={operationIdentity(operation)}
              type="button"
              className="w-full rounded-md px-2.5 py-1.5 text-left text-[12px] text-secondary hover:bg-surface-hover focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent"
              onClick={() => {
                if (operation.restored || !operationHostMatches(operation, remoteHosts)) {
                  setRestoredContext(operation);

                  return;
                }

                void switchHost(id)
                  .then(async (switched) => {
                    if (!switched) return;
                    triggerRef.current?.focus();

                    if (isLibraryOperation(operation)) {
                      const source = operation.sources?.[0] ?? operation.items[0]?.source;
                      void navigate({
                        to: "/install",
                        search: {
                          tab:
                            source?.kind === "git"
                              ? "git"
                              : source?.kind === "marketplace"
                                ? "market"
                                : "local",
                        },
                      });
                    } else if (operation.project_id) {
                      const projects = await invokeHost<Project[]>(id, "get_projects");

                      if (getActiveHostId() !== id) return;

                      if (
                        projects.find((p) => p.id === operation.project_id)?.path !==
                        operation.project_path
                      ) {
                        setRestoredContext(operation);

                        return;
                      }

                      void navigate({ to: "/project/$id", params: { id: operation.project_id } });
                    }
                  })
                  .catch(() => setRestoredContext(operation));
              }}
            >
              {label}
            </button>
          ))}
        </div>
      )}

      {restoredContext && (
        <RestoredOperationDialog
          operation={
            allOperations.find(
              (o) => operationIdentity(o) === operationIdentity(restoredContext),
            ) ?? restoredContext
          }
          onClose={() => setRestoredContext(null)}
        />
      )}
      {open && (
        <div
          role="menu"
          aria-label={t("hostSwitcher.label")}
          className="absolute inset-x-2.5 top-full z-40 mt-1 rounded-lg border border-border bg-surface p-1 shadow-lg"
        >
          {option(
            "local",
            null,
            Monitor,
            t("hostSwitcher.local"),
            t("hostSwitcher.localDetail"),
            false,
          )}
          {remoteHosts.map((host) =>
            option(host.id, host.id, Server, host.name, host.ssh_target, true),
          )}
          <div className="my-1 border-t border-border-subtle" />
          {connectingHostId !== null && (
            <button
              type="button"
              role="menuitem"
              onClick={() => choose(current)}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-tertiary transition-colors hover:bg-surface-hover hover:text-secondary"
            >
              <X className="h-3.5 w-3.5 shrink-0 text-muted" />
              {t("hostSwitcher.cancelConnect")}
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              navigate(settingsLink("remote"));
            }}
            className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-tertiary transition-colors hover:bg-surface-hover hover:text-secondary"
          >
            <Settings2 className="h-3.5 w-3.5 shrink-0 text-muted" />
            {t("hostSwitcher.manage")}
          </button>
        </div>
      )}
    </div>
  );
}
