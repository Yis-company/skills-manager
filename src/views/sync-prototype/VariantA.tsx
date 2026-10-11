// PROTOTYPE (SID-106, SID-112) · Variant A "Hosts table": a Tailscale-style table of
// hosts is the page; review, history and agent logs sit in tabs below it; "Add host"
// opens a side sheet with a vertical stepper.
import { MoreHorizontal, Plus, RefreshCw, RotateCcw, Server, X } from "lucide-react";
import { useState } from "react";

import { cn } from "../../utils";
import { ACTIVE_PRESET, HISTORY, type Host, REVIEW_KIND_LABEL, SYNC_REMOTE } from "./data";
import {
  BackupIsSyncNotice,
  JoinPreview,
  PasteKey,
  RemoveHostDialog,
  SCENARIOS,
  type Scenario,
  StatePill,
  STEPS,
  StepIcon,
  type SyncPrototype,
  useAddHostRun,
  useSyncPrototype,
} from "./shared";

export function VariantA() {
  const s = useSyncPrototype();
  const [tab, setTab] = useState<"review" | "history" | "log">("review");
  const [logHost, setLogHost] = useState("remote-agent");
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [removing, setRemoving] = useState<Host | null>(null);
  const [adding, setAdding] = useState(false);
  const local = s.hosts[0];

  return (
    <div className="app-page">
      <div className="app-page-header flex items-end justify-between gap-3">
        <div>
          <h1 className="app-page-title">Sync</h1>
          <p className="app-page-subtitle">
            Every host follows <span className="font-mono text-[12px]">{SYNC_REMOTE}</span> and uses
            the active preset <b className="text-secondary">{ACTIVE_PRESET}</b>.
          </p>
        </div>
        <div className="flex gap-2">
          <button
            type="button"
            className="app-toolbar-button app-toolbar-button-secondary h-9"
            onClick={() => s.syncNow(local.id)}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", local.state === "syncing" && "animate-spin")} />
            Sync now
          </button>
          <button
            type="button"
            className="app-toolbar-button app-toolbar-button-primary h-9"
            onClick={() => setAdding(true)}
          >
            <Plus className="h-3.5 w-3.5" />
            Add host
          </button>
        </div>
      </div>

      {s.noticeOpen && <BackupIsSyncNotice onDismiss={s.dismissNotice} />}

      <section className="app-panel overflow-hidden">
        <table className="w-full text-left text-[13px]">
          <thead className="border-b border-border-subtle bg-bg-secondary text-[12px] text-muted">
            <tr>
              <th className="px-4 py-2 font-medium">Host</th>
              <th className="px-4 py-2 font-medium">Sync state</th>
              <th className="px-4 py-2 font-medium">Agents</th>
              <th className="px-4 py-2 font-medium">Version</th>
              <th className="px-4 py-2 font-medium">Last sync</th>
              <th className="w-10" />
            </tr>
          </thead>
          <tbody>
            {s.hosts.map((h) => (
              <tr key={h.id} className="border-b border-border-subtle last:border-0">
                <td className="px-4 py-3">
                  <div className="flex items-center gap-2 font-medium text-primary">
                    {h.name}
                    {h.kind === "local" && <span className="app-badge !py-0 !text-[11px]">This host</span>}
                  </div>
                  <div className="font-mono text-[11px] text-faint">
                    {h.target ?? "this computer"} ·{" "}
                    {h.runner === "systemd" ? "timer" : h.runner === "app" ? "app" : "on connect"}
                  </div>
                </td>
                <td className="px-4 py-3">
                  <StatePill state={h.state} />
                  {h.reason && <div className="mt-1 text-[11px] text-muted">{h.reason}</div>}
                </td>
                <td className="px-4 py-3 text-tertiary">{h.agents.join(", ")}</td>
                <td className="px-4 py-3 font-mono text-[12px] text-tertiary">{h.version}</td>
                <td className="px-4 py-3 text-tertiary">{h.lastSync}</td>
                <td className="relative px-2">
                  <button
                    type="button"
                    aria-label="Host actions"
                    className="rounded-md p-1 text-muted hover:bg-surface-hover"
                    onClick={() => setMenuFor(menuFor === h.id ? null : h.id)}
                  >
                    <MoreHorizontal className="h-4 w-4" />
                  </button>
                  {menuFor === h.id && (
                    <HostMenu
                      host={h}
                      s={s}
                      onClose={() => setMenuFor(null)}
                      onRemove={() => setRemoving(h)}
                    />
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="space-y-3">
        <div className="app-segmented w-fit">
          {(
            [
              ["review", `Needs review (${s.review.length})`],
              ["history", "History"],
              ["log", "Agent log"],
            ] as const
          ).map(([key, label]) => (
            <button
              key={key}
              type="button"
              onClick={() => setTab(key)}
              className={cn("app-segmented-button", tab === key && "app-segmented-button-active")}
            >
              {label}
            </button>
          ))}
        </div>

        {tab === "review" && (
          <div className="app-panel divide-y divide-border-subtle">
            {s.review.length === 0 && (
              <div className="p-6 text-center text-[13px] text-muted">Nothing needs review.</div>
            )}
            {s.review.map((item) => (
              <div key={item.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
                <div className="min-w-0 flex-1">
                  <div className="text-[11px] font-medium uppercase tracking-wide text-amber-600 dark:text-amber-400">
                    {REVIEW_KIND_LABEL[item.kind]}
                    {item.host ? ` · ${item.host}` : ""}
                  </div>
                  <div className="text-[13px] font-medium text-primary">{item.title}</div>
                  <div className="text-[12px] text-muted">{item.detail}</div>
                </div>
                <div className="flex flex-wrap gap-1.5">
                  {item.choices.map((c) => (
                    <button
                      key={c}
                      type="button"
                      className="rounded-lg border border-border-subtle bg-surface px-2.5 py-1.5 text-[12px] font-medium text-secondary hover:bg-surface-hover"
                      onClick={() => s.resolve(item, c)}
                    >
                      {c}
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </div>
        )}

        {tab === "history" && (
          <div className="app-panel divide-y divide-border-subtle">
            {HISTORY.map((e) => (
              <div key={e.id} className="grid grid-cols-[110px_130px_1fr_auto] items-center gap-3 px-4 py-2.5 text-[13px]">
                <span className="text-[12px] text-faint">
                  {e.day} {e.when}
                </span>
                <span className="truncate font-medium text-tertiary">{e.host}</span>
                <span className="text-secondary">{e.text}</span>
                {e.canRestore ? (
                  <button type="button" className="inline-flex items-center gap-1 text-[12px] font-medium text-accent hover:underline">
                    <RotateCcw className="h-3 w-3" /> Restore
                  </button>
                ) : (
                  <span />
                )}
              </div>
            ))}
          </div>
        )}

        {tab === "log" && (
          <div className="app-panel p-4">
            <select
              value={logHost}
              onChange={(e) => setLogHost(e.target.value)}
              className="app-input mb-3 h-8"
            >
              {s.hosts.map((h) => (
                <option key={h.id} value={h.id}>
                  {h.name}
                </option>
              ))}
            </select>
            <ul className="space-y-1.5 text-[13px]">
              {s.hosts
                .find((h) => h.id === logHost)
                ?.agentLog.map((l, i) => (
                  <li key={i} className="flex gap-3">
                    <span className="w-16 shrink-0 text-[12px] text-faint">{l.when}</span>
                    <span
                      className={cn(
                        "text-secondary",
                        l.tone === "warn" && "text-amber-600 dark:text-amber-400",
                        l.tone === "error" && "text-red-600 dark:text-red-300",
                      )}
                    >
                      {l.text}
                    </span>
                  </li>
                ))}
            </ul>
          </div>
        )}
      </section>

      {adding && <AddHostSheet s={s} onClose={() => setAdding(false)} />}
      {removing && (
        <RemoveHostDialog
          host={removing}
          onCancel={() => setRemoving(null)}
          onConfirm={(alsoLeave) => {
            s.removeHost(removing, alsoLeave);
            setRemoving(null);
          }}
        />
      )}
    </div>
  );
}

function HostMenu({
  host,
  s,
  onClose,
  onRemove,
}: {
  host: Host;
  s: SyncPrototype;
  onClose: () => void;
  onRemove: () => void;
}) {
  const item = (label: string, fn: () => void, danger = false) => (
    <button
      type="button"
      onClick={() => {
        fn();
        onClose();
      }}
      className={cn(
        "block w-full rounded-md px-2.5 py-1.5 text-left text-[13px] hover:bg-surface-hover",
        danger ? "text-red-600 dark:text-red-300" : "text-secondary",
      )}
    >
      {label}
    </button>
  );

  return (
    <div className="absolute right-2 top-10 z-30 w-48 rounded-lg border border-border-subtle bg-surface p-1 shadow-xl">
      {item("Sync now", () => s.syncNow(host.id))}
      {item(host.state === "paused" ? "Resume sync" : "Pause sync", () => s.togglePause(host))}
      {host.kind === "ssh" && item("Open host", () => {})}
      {host.hasPreJoinCopy && item("Undo join", () => s.undoJoin(host))}
      {host.hasPreJoinCopy && item("Delete pre-join backup", () => s.deleteBackup(host))}
      {host.kind === "ssh" && item("Remove host…", onRemove, true)}
    </div>
  );
}

function AddHostSheet({ s, onClose }: { s: SyncPrototype; onClose: () => void }) {
  const add = useAddHostRun(s.addHost);
  const [name, setName] = useState("gpu-2");
  const [target, setTarget] = useState("yi@gpu-2.local");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const run = add.run;

  return (
    <div className="fixed bottom-0 left-[220px] right-0 top-[28px] z-40">
      <div className="absolute inset-0 bg-black/60" onClick={onClose} />
      <div className="absolute inset-y-0 right-0 flex w-[min(520px,100%)] flex-col border-l border-border-subtle bg-bg-secondary shadow-2xl">
        <div className="flex items-center justify-between border-b border-border-subtle px-5 py-3">
          <div className="flex items-center gap-2 text-[15px] font-semibold text-primary">
            <Server className="h-4 w-4 text-muted" /> Add host
          </div>
          <button type="button" aria-label="Close" onClick={onClose} className="rounded-md p-1 text-muted hover:bg-surface-hover">
            <X className="h-4 w-4" />
          </button>
        </div>
        <div className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
          <div className="grid grid-cols-2 gap-2">
            <label className="text-[12px] text-muted">
              Name
              <input className="app-input mt-1 h-9 w-full" value={name} onChange={(e) => setName(e.target.value)} disabled={!!run} />
            </label>
            <label className="text-[12px] text-muted">
              SSH target
              <input className="app-input mt-1 h-9 w-full font-mono" value={target} onChange={(e) => setTarget(e.target.value)} disabled={!!run} />
            </label>
          </div>
          {!run && (
            <>
              <label className="block text-[12px] text-muted">
                Prototype scenario
                <select className="app-input mt-1 h-9 w-full" value={scenario} onChange={(e) => setScenario(e.target.value as Scenario)}>
                  {SCENARIOS.map((sc) => (
                    <option key={sc.key} value={sc.key}>
                      {sc.label}
                    </option>
                  ))}
                </select>
              </label>
              <button type="button" className="app-button-primary w-full" onClick={() => add.start(name, target, scenario)}>
                Connect and add
              </button>
            </>
          )}

          {run && (
            <ol className="relative space-y-4 border-l border-border-subtle pl-5">
              {STEPS.map((st) => {
                const cur = run.steps[st.key];

                return (
                  <li key={st.key} className="relative">
                    <span className="absolute -left-[29px] top-0 rounded-full bg-bg-secondary p-0.5">
                      <StepIcon status={cur.status} />
                    </span>
                    <div className={cn("text-[13px] font-medium", cur.status === "todo" ? "text-faint" : "text-primary")}>
                      {st.label}
                    </div>
                    {cur.note && <div className="mt-0.5 text-[12px] text-muted">{cur.note}</div>}
                    {st.key === "key" && run.waitingFor === "paste_key" && (
                      <div className="mt-2 rounded-lg border border-amber-500/30 bg-amber-500/10 p-3">
                        <PasteKey hostName={run.name} />
                        <button type="button" className="app-button-primary mt-2 !py-1.5" onClick={add.proceed}>
                          I've added it
                        </button>
                      </div>
                    )}
                    {st.key === "preview" && run.waitingFor === "confirm_join" && (
                      <div className="mt-2 rounded-lg border border-border-subtle bg-surface p-3">
                        <JoinPreview />
                        <div className="mt-3 flex gap-2">
                          <button type="button" className="app-button-primary !py-1.5" onClick={add.proceed}>
                            Join Sync
                          </button>
                          <button type="button" className="app-button-secondary !py-1.5" onClick={add.reset}>
                            Cancel
                          </button>
                        </div>
                      </div>
                    )}
                  </li>
                );
              })}
            </ol>
          )}

          {run?.phase === "blocked" && (
            <button type="button" className="app-button-secondary w-full" onClick={add.reset}>
              Back to start
            </button>
          )}
          {run?.phase === "done" && (
            <button type="button" className="app-button-primary w-full" onClick={onClose}>
              Done: {run.name} is up to date
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
