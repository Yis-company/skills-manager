// PROTOTYPE (SID-106, SID-112) · Variant C "Host detail": a master-detail split. The
// left column lists Overview, Needs review and every host; the right pane shows the
// selection. "Add host" adds a row at once and shows setup as a live log, with the
// decisions inline in the log.
import { AlertTriangle, LayoutGrid, Plus, RefreshCw, RotateCcw, Terminal } from "lucide-react";
import { type ReactNode, useState } from "react";

import { cn } from "../../utils";
import { ACTIVE_PRESET, HISTORY, type Host, REVIEW_KIND_LABEL, SYNC_REMOTE } from "./data";
import {
  BackupIsSyncNotice,
  JoinPreview,
  PasteKey,
  RemoveHostDialog,
  SCENARIOS,
  type Scenario,
  StateDot,
  StatePill,
  STEPS,
  StepIcon,
  type SyncPrototype,
  useAddHostRun,
  useSyncPrototype,
} from "./shared";

type Selection = "overview" | "review" | "add" | string;

export function VariantC() {
  const s = useSyncPrototype();
  const [sel, setSel] = useState<Selection>("overview");
  const [removing, setRemoving] = useState<Host | null>(null);
  const add = useAddHostRun((host) => {
    s.addHost(host);
    setSel(host.id);
  });
  const host = s.hosts.find((h) => h.id === sel);

  const navItem = (key: Selection, label: ReactNode, extra?: ReactNode) => (
    <button
      key={key}
      type="button"
      onClick={() => setSel(key)}
      className={cn(
        "flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[13px]",
        sel === key ? "bg-surface-active text-primary" : "text-secondary hover:bg-surface-hover",
      )}
    >
      {label}
      <span className="ml-auto">{extra}</span>
    </button>
  );

  return (
    <div className="app-page !gap-4">
      <div className="app-page-header">
        <h1 className="app-page-title">Sync</h1>
        <p className="app-page-subtitle">
          <span className="font-mono text-[12px]">{SYNC_REMOTE}</span> · active preset {ACTIVE_PRESET}
        </p>
      </div>
      {s.noticeOpen && <BackupIsSyncNotice onDismiss={s.dismissNotice} />}

      <div className="grid min-h-[560px] grid-cols-[240px_minmax(0,1fr)] overflow-hidden rounded-xl border border-border-subtle">
        <aside className="space-y-0.5 border-r border-border-subtle bg-bg-secondary p-2">
          {navItem(
            "overview",
            <>
              <LayoutGrid className="h-3.5 w-3.5 text-muted" /> Overview
            </>,
          )}
          {navItem(
            "review",
            <>
              <AlertTriangle className="h-3.5 w-3.5 text-amber-500" /> Needs review
            </>,
            s.review.length > 0 && <span className="app-badge !px-1.5 !py-0 !text-[11px]">{s.review.length}</span>,
          )}
          <div className="app-section-title px-2.5 pb-1 pt-4">Hosts</div>
          {s.hosts.map((h) =>
            navItem(
              h.id,
              <>
                <StateDot state={h.state} />
                <span className="truncate">{h.name}</span>
                {h.kind === "local" && <span className="text-[11px] text-faint">this host</span>}
              </>,
              h.editedHere + h.failures > 0 && <span className="text-[11px] text-amber-600 dark:text-amber-400">{h.editedHere + h.failures}</span>,
            ),
          )}
          {add.run && add.run.phase !== "done" &&
            navItem(
              "add",
              <>
                <RefreshCw className="h-3 w-3 animate-spin text-accent" />
                <span className="truncate">{add.run.name}</span>
                <span className="text-[11px] text-faint">setting up</span>
              </>,
            )}
          <button
            type="button"
            onClick={() => setSel("add")}
            className="mt-1 flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-[13px] text-muted hover:bg-surface-hover hover:text-secondary"
          >
            <Plus className="h-3.5 w-3.5" /> Add host
          </button>
        </aside>

        <main className="min-w-0 bg-surface p-5">
          {sel === "overview" && <Overview s={s} onOpen={setSel} />}
          {sel === "review" && <Review s={s} />}
          {sel === "add" && <AddHostLog add={add} />}
          {host && <HostDetail host={host} s={s} onRemove={() => setRemoving(host)} />}
        </main>
      </div>

      {removing && (
        <RemoveHostDialog
          host={removing}
          onCancel={() => setRemoving(null)}
          onConfirm={(alsoLeave) => {
            s.removeHost(removing, alsoLeave);
            setRemoving(null);
            setSel("overview");
          }}
        />
      )}
    </div>
  );
}

function Overview({ s, onOpen }: { s: SyncPrototype; onOpen: (id: string) => void }) {
  return (
    <div className="space-y-5">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        {s.hosts.map((h) => (
          <button key={h.id} type="button" onClick={() => onOpen(h.id)} className="rounded-lg border border-border-subtle p-3 text-left hover:bg-surface-hover">
            <div className="truncate text-[13px] font-medium text-primary">{h.name}</div>
            <StatePill state={h.state} className="mt-2" />
            <div className="mt-1 text-[11px] text-faint">{h.lastSync}</div>
          </button>
        ))}
      </div>
      <div>
        <div className="app-section-title mb-2">Recent changes</div>
        <ul className="divide-y divide-border-subtle text-[13px]">
          {HISTORY.slice(0, 5).map((e) => (
            <li key={e.id} className="flex gap-3 py-2">
              <span className="w-12 shrink-0 text-[12px] text-faint">{e.when}</span>
              <span className="w-28 shrink-0 truncate font-medium text-tertiary">{e.host}</span>
              <span className="text-secondary">{e.text}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

function Review({ s }: { s: SyncPrototype }) {
  if (s.review.length === 0) return <div className="py-16 text-center text-[13px] text-muted">Nothing needs review.</div>;

  return (
    <div className="space-y-3">
      <p className="text-[12px] text-muted">Choices are recorded in Sync. Each host applies them on its next pull, even when it's not connected.</p>
      {s.review.map((item) => (
        <div key={item.id} className="rounded-lg border border-border-subtle p-4">
          <div className="text-[11px] font-semibold uppercase tracking-wide text-amber-600 dark:text-amber-400">
            {REVIEW_KIND_LABEL[item.kind]}
            {item.host ? ` · ${item.host}` : ""}
          </div>
          <div className="mt-0.5 text-[14px] font-medium text-primary">{item.title}</div>
          <div className="text-[12px] text-muted">{item.detail}</div>
          {item.kind !== "preset_name" && (
            <div className="mt-3 grid grid-cols-2 gap-2 font-mono text-[11px]">
              <div className="rounded-md bg-red-500/10 p-2 text-red-600 dark:text-red-300">- description: Build UIs fast</div>
              <div className="rounded-md bg-emerald-500/10 p-2 text-emerald-700 dark:text-emerald-300">+ description: Build accessible UIs</div>
            </div>
          )}
          <div className="mt-3 flex flex-wrap gap-2">
            {item.choices.map((c) => (
              <button key={c} type="button" className="app-button-secondary !py-1.5" onClick={() => s.resolve(item, c)}>
                {c}
              </button>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function HostDetail({ host, s, onRemove }: { host: Host; s: SyncPrototype; onRemove: () => void }) {
  const [tab, setTab] = useState<"log" | "history" | "details">("log");
  const history = HISTORY.filter((e) => e.host === host.name);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h2 className="text-[18px] font-semibold text-primary">{host.name}</h2>
            <StatePill state={host.state} />
          </div>
          <div className="mt-0.5 font-mono text-[12px] text-faint">
            {host.target ?? "this computer"} · {host.version} · last sync {host.lastSync}
          </div>
          {host.reason && <div className="mt-1 text-[12px] text-muted">{host.reason}</div>}
        </div>
        <div className="flex flex-wrap gap-1.5">
          <button type="button" className="app-button-secondary !py-1.5" onClick={() => s.syncNow(host.id)}>
            <RefreshCw className={cn("h-3.5 w-3.5", host.state === "syncing" && "animate-spin")} /> Sync now
          </button>
          <button type="button" className="app-button-secondary !py-1.5" onClick={() => s.togglePause(host)}>
            {host.state === "paused" ? "Resume" : "Pause"}
          </button>
          {host.kind === "ssh" && (
            <button type="button" className="app-button-secondary !py-1.5">
              Open host
            </button>
          )}
        </div>
      </div>

      <div className="flex gap-4 border-b border-border-subtle text-[13px]">
        {(["log", "history", "details"] as const).map((t) => (
          <button
            key={t}
            type="button"
            onClick={() => setTab(t)}
            className={cn("-mb-px border-b-2 pb-2", tab === t ? "border-accent font-medium text-primary" : "border-transparent text-muted hover:text-secondary")}
          >
            {t === "log" ? "Agent log" : t === "history" ? "Changes from this host" : "Details"}
          </button>
        ))}
      </div>

      {tab === "log" && (
        <ul className="space-y-1.5 text-[13px]">
          {host.agentLog.map((l, i) => (
            <li key={i} className={cn("text-secondary", l.tone === "warn" && "text-amber-600 dark:text-amber-400", l.tone === "error" && "text-red-600 dark:text-red-300")}>
              <span className="mr-3 text-[12px] text-faint">{l.when}</span>
              {l.text}
              {l.tone === "warn" && (
                <span className="ml-2 inline-flex gap-1">
                  <button type="button" className="text-[12px] font-medium text-accent hover:underline">Use Sync version</button>·
                  <button type="button" className="text-[12px] font-medium text-accent hover:underline">Keep my edit</button>
                </span>
              )}
            </li>
          ))}
        </ul>
      )}

      {tab === "history" && (
        <ul className="divide-y divide-border-subtle text-[13px]">
          {history.length === 0 && <li className="py-3 text-muted">No changes from this host yet.</li>}
          {history.map((e) => (
            <li key={e.id} className="flex items-center gap-3 py-2">
              <span className="w-24 shrink-0 text-[12px] text-faint">{e.day} {e.when}</span>
              <span className="flex-1 text-secondary">{e.text}</span>
              {e.canRestore && (
                <button type="button" className="inline-flex items-center gap-1 text-[12px] font-medium text-accent hover:underline">
                  <RotateCcw className="h-3 w-3" /> Restore
                </button>
              )}
            </li>
          ))}
        </ul>
      )}

      {tab === "details" && (
        <dl className="grid grid-cols-[160px_1fr] gap-y-2 text-[13px]">
          <dt className="text-muted">Agents</dt>
          <dd className="text-secondary">{host.agents.join(", ")}</dd>
          <dt className="text-muted">Automatic sync</dt>
          <dd className="text-secondary">
            {host.runner === "systemd" ? "systemd timer, every 5 minutes" : host.runner === "app" ? "this app, every 5 minutes" : "only on connect or Sync now"}
          </dd>
          <dt className="text-muted">Access</dt>
          <dd className="text-secondary">{host.kind === "ssh" ? `Deploy key “Agents Manager · ${host.name}”` : "GitHub sign-in on this Mac"}</dd>
          {host.hasPreJoinCopy && (
            <>
              <dt className="text-muted">Pre-join copy</dt>
              <dd className="flex gap-3">
                <button type="button" className="text-[13px] font-medium text-accent hover:underline" onClick={() => s.undoJoin(host)}>
                  Undo join
                </button>
                <button type="button" className="text-[13px] font-medium text-accent hover:underline" onClick={() => s.deleteBackup(host)}>
                  Delete backup
                </button>
              </dd>
            </>
          )}
          {host.kind === "ssh" && (
            <>
              <dt className="text-muted">Danger zone</dt>
              <dd>
                <button type="button" className="text-[13px] font-medium text-red-600 hover:underline dark:text-red-300" onClick={onRemove}>
                  Remove host…
                </button>
              </dd>
            </>
          )}
        </dl>
      )}
    </div>
  );
}

function AddHostLog({ add }: { add: ReturnType<typeof useAddHostRun> }) {
  const [name, setName] = useState("gpu-2");
  const [target, setTarget] = useState("yi@gpu-2.local");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const run = add.run;

  if (!run)
    return (
      <div className="max-w-[420px] space-y-3">
        <h2 className="text-[18px] font-semibold text-primary">Add host</h2>
        <input className="app-input w-full" value={name} onChange={(e) => setName(e.target.value)} placeholder="Name" />
        <input className="app-input w-full font-mono" value={target} onChange={(e) => setTarget(e.target.value)} placeholder="user@host" />
        <select className="app-input w-full" value={scenario} onChange={(e) => setScenario(e.target.value as Scenario)}>
          {SCENARIOS.map((sc) => (
            <option key={sc.key} value={sc.key}>
              Prototype: {sc.label}
            </option>
          ))}
        </select>
        <button type="button" className="app-button-primary w-full" onClick={() => add.start(name, target, scenario)}>
          Connect and add
        </button>
      </div>
    );

  const done = STEPS.filter((st) => run.steps[st.key].status === "done").length;

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <Terminal className="h-4 w-4 text-muted" />
        <h2 className="text-[16px] font-semibold text-primary">Setting up {run.name}</h2>
        <span className="font-mono text-[12px] text-faint">{run.target}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-surface-active">
        <div className="h-full bg-accent transition-all" style={{ width: `${(done / STEPS.length) * 100}%` }} />
      </div>
      <div className="space-y-2 rounded-lg bg-zinc-950 p-4 font-mono text-[12px] leading-5 text-zinc-300">
        {STEPS.filter((st) => run.steps[st.key].status !== "todo").map((st) => {
          const cur = run.steps[st.key];

          return (
            <div key={st.key}>
              <div className="flex items-start gap-2">
                <StepIcon status={cur.status} />
                <span>
                  <span className="text-zinc-100">{st.label}</span>
                  {cur.note && <span className="block text-zinc-400">{cur.note}</span>}
                </span>
              </div>
              {st.key === "key" && run.waitingFor === "paste_key" && (
                <div className="ml-6 mt-2 rounded-md bg-surface p-3 font-sans">
                  <PasteKey hostName={run.name} />
                  <button type="button" className="app-button-primary mt-2 !py-1.5" onClick={add.proceed}>
                    I've added it
                  </button>
                </div>
              )}
              {st.key === "preview" && run.waitingFor === "confirm_join" && (
                <div className="ml-6 mt-2 rounded-md bg-surface p-3 font-sans">
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
            </div>
          );
        })}
      </div>
      {run.phase === "blocked" && (
        <button type="button" className="app-button-secondary" onClick={add.reset}>
          Start over
        </button>
      )}
    </div>
  );
}
