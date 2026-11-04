import { Check, CircleHelp, Plus } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";

import { useWorkspaceChanges } from "../context/WorkspaceChangesContext";
import type { WorkspaceChange } from "../lib/workspaceChanges";
import { cn } from "../utils";
import { AgentIcon } from "./AgentIcon";

export interface MatrixAgent {
  key: string;
  display_name: string;
  installed?: boolean;
  enabled?: boolean;
}

export interface MatrixRow {
  id: string;
  name: string;
  description?: null | string;
  metadata?: ReactNode;
  details?: ReactNode;
  selected?: boolean;
  onSelect?: () => void;
  onInspect?: () => void;
  cells: {
    agent: string;
    present: boolean | null;
    note?: string;
    disabled?: string;
    change: WorkspaceChange;
  }[];
}

export function AgentMatrix({
  rows,
  agents,
  destination,
  hint,
}: {
  rows: MatrixRow[];
  agents: MatrixAgent[];
  destination: string;
  hint?: string;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(new Set<string>());
  const { changes, stage, busy } = useWorkspaceChanges();

  return (
    <section aria-label={t("workspace.agentPresence")} className="min-w-0">
      <div className="mb-3 flex flex-wrap justify-between gap-2 text-[12px]">
        <p>
          <strong>{t("workspace.agentPresence")}</strong> · {destination}
        </p>
        <p className="text-muted">{hint ?? t("workspace.presenceHint")}</p>
      </div>
      <div className="workspace-matrix-scroll">
        <table className="workspace-matrix">
          <thead>
            <tr>
              <th scope="col">{t("workspace.resource")}</th>
              {agents.map((agent) => (
                <th key={agent.key} scope="col">
                  <span className="inline-flex flex-col items-center gap-1.5">
                    <span aria-hidden="true">
                      <AgentIcon
                        agentKey={agent.key}
                        displayName={agent.display_name}
                        className="h-5 w-5"
                      />
                    </span>
                    <span>{agent.display_name}</span>
                  </span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.id}>
                <th scope="row">
                  <div className="flex items-center gap-2">
                    {row.onSelect && (
                      <input
                        type="checkbox"
                        checked={row.selected}
                        aria-label={t("mySkills.selectSkill", { name: row.name })}
                        onChange={row.onSelect}
                      />
                    )}
                    <h3 className="min-w-0 text-[13px] font-semibold">
                      {row.onInspect ? (
                        <button className="text-left hover:underline" onClick={row.onInspect}>
                          {row.name}
                        </button>
                      ) : (
                        row.name
                      )}
                    </h3>
                  </div>
                  {row.description && (
                    <p
                      className="mt-1.5 line-clamp-2 text-[13px] font-normal leading-5 text-muted"
                      title={row.description}
                    >
                      {row.description}
                    </p>
                  )}
                  {row.metadata && (
                    <div className="mt-2 flex flex-wrap items-center gap-2 text-[12px] font-normal text-muted">
                      {row.metadata}
                    </div>
                  )}
                  {row.details && (
                    <details
                      className="mt-2 font-normal"
                      onToggle={(event) => {
                        const open = event.currentTarget.open;
                        setExpanded((current) => {
                          if (current.has(row.id) === open) return current;
                          const next = new Set(current);

                          if (open) next.add(row.id);
                          else next.delete(row.id);

                          return next;
                        });
                      }}
                    >
                      <summary className="w-fit cursor-pointer text-[12px] text-muted">
                        {t("workspace.sourceActions")}
                      </summary>
                      {expanded.has(row.id) && <div className="py-3">{row.details}</div>}
                    </details>
                  )}
                </th>
                {agents.map((agent) => {
                  const cell = row.cells.find((item) => item.agent === agent.key);

                  if (!cell)
                    return (
                      <td key={agent.key}>
                        <span className="text-muted">—</span>
                      </td>
                    );
                  const pending = changes.find((change) => change.id === cell.change.id);
                  const staged = Boolean(pending);
                  const present = pending ? pending.add : cell.present;

                  return (
                    <td key={agent.key}>
                      <button
                        type="button"
                        disabled={
                          busy || Boolean(row.onSelect) || (!staged && Boolean(cell.disabled))
                        }
                        aria-pressed={present ?? undefined}
                        aria-label={t(
                          staged
                            ? "workspace.undoTargetChange"
                            : present
                              ? "workspace.removeNamed"
                              : "workspace.addNamed",
                          { name: row.name, agent: agent.display_name },
                        )}
                        title={cell.disabled || cell.note || undefined}
                        onClick={() => stage(cell.change)}
                        className={cn(
                          "workspace-matrix-cell",
                          present && "workspace-matrix-cell-present",
                          staged && "workspace-matrix-cell-staged",
                        )}
                      >
                        {present === null ? (
                          <CircleHelp className="h-4 w-4" />
                        ) : present ? (
                          <Check className="h-4 w-4" />
                        ) : (
                          <Plus className="h-4 w-4" />
                        )}
                      </button>
                      {(staged || cell.note || cell.disabled) && (
                        <span className="mt-1 block text-[11px] leading-4 text-muted">
                          {staged
                            ? t(pending?.add ? "workspace.toAdd" : "workspace.toRemove")
                            : cell.note || cell.disabled}
                        </span>
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!rows.length && (
        <p className="py-8 text-center text-[13px] text-muted">{t("workspace.noResources")}</p>
      )}
      <p className="mt-3 text-[12px] text-muted">{t("workspace.matrixLegend")}</p>
    </section>
  );
}
