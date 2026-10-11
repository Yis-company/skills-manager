// PROTOTYPE (SID-106, SID-112) · Variant B "Activity feed": one centered column. A strip
// of host cards on top, open review items as cards, then the Sync history as a feed.
// "Add host" replaces the page with a one-step-per-screen wizard.
import { ArrowLeft, Plus, RefreshCw, RotateCcw } from "lucide-react";
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

export function VariantB() {
  const s = useSyncPrototype();
  const [focus, setFocus] = useState<string | null>(null);
  const [wizard, setWizard] = useState(false);
  const [removing, setRemoving] = useState<Host | null>(null);
  const focused = s.hosts.find((h) => h.id === focus) ?? null;
  const needAttention = s.hosts.filter((h) => h.state !== "up_to_date").length;

  if (wizard) return <AddHostWizard s={s} onExit={() => setWizard(false)} />;

  const feed = focused ? HISTORY.filter((e) => e.host === focused.name) : HISTORY;
  const days = [...new Set(feed.map((e) => e.day))];

  return (
    <div className="app-page app-page-narrow !max-w-[820px]">
      <div className="pt-2">
        <h1 className="text-[22px] font-semibold tracking-tight text-primary">
          {s.review.length === 0 && needAttention === 0
            ? "Every host is up to date"
            : `${s.review.length} to review · ${needAttention} host${needAttention === 1 ? "" : "s"} need attention`}
        </h1>
        <p className="mt-1 text-[13px] text-muted">
          Active preset <b className="text-secondary">{ACTIVE_PRESET}</b> · Sync remote{" "}
          <span className="font-mono text-[12px]">{SYNC_REMOTE}</span>
        </p>
      </div>

      {s.noticeOpen && <BackupIsSyncNotice onDismiss={s.dismissNotice} />}

      <div className="flex gap-2 overflow-x-auto pb-1">
        {s.hosts.map((h) => (
          <button
            key={h.id}
            type="button"
            onClick={() => setFocus(focus === h.id ? null : h.id)}
            className={cn(
              "w-[170px] shrink-0 rounded-xl border bg-surface p-3 text-left transition-colors hover:bg-surface-hover",
              focus === h.id ? "border-accent-border ring-1 ring-accent-border" : "border-border-subtle",
            )}
          >
            <div className="flex items-center gap-2">
              <span className="flex h-7 w-7 items-center justify-center rounded-full bg-surface-active text-[12px] font-semibold text-secondary">
                {h.name[0].toUpperCase()}
              </span>
              <span className="min-w-0 truncate text-[13px] font-medium text-primary">{h.name}</span>
            </div>
            <div className="mt-2 flex items-center gap-1.5 text-[12px] text-tertiary">
              <StateDot state={h.state} />
              <span className="truncate">{h.kind === "local" ? "This host" : h.lastSync}</span>
            </div>
          </button>
        ))}
        <button
          type="button"
          onClick={() => setWizard(true)}
          className="flex w-[120px] shrink-0 flex-col items-center justify-center gap-1 rounded-xl border border-dashed border-border text-[13px] font-medium text-muted hover:bg-surface-hover hover:text-secondary"
        >
          <Plus className="h-4 w-4" /> Add host
        </button>
      </div>

      {focused && (
        <div className="app-panel space-y-3 p-4">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-[15px] font-semibold text-primary">{focused.name}</span>
            <StatePill state={focused.state} />
            <span className="font-mono text-[11px] text-faint">{focused.target ?? "this computer"} · {focused.version}</span>
            <div className="ml-auto flex flex-wrap gap-1.5">
              <SmallButton onClick={() => s.syncNow(focused.id)}>Sync now</SmallButton>
              <SmallButton onClick={() => s.togglePause(focused)}>{focused.state === "paused" ? "Resume" : "Pause"}</SmallButton>
              {focused.hasPreJoinCopy && <SmallButton onClick={() => s.undoJoin(focused)}>Undo join</SmallButton>}
              {focused.kind === "ssh" && (
                <SmallButton danger onClick={() => setRemoving(focused)}>
                  Remove…
                </SmallButton>
              )}
            </div>
          </div>
          {focused.reason && <p className="text-[12px] text-muted">{focused.reason}</p>}
          <ul className="space-y-1 border-t border-border-subtle pt-3 text-[13px]">
            {focused.agentLog.map((l, i) => (
              <li key={i} className={cn("text-secondary", l.tone === "warn" && "text-amber-600 dark:text-amber-400", l.tone === "error" && "text-red-600 dark:text-red-300")}>
                <span className="mr-2 text-[12px] text-faint">{l.when}</span>
                {l.text}
              </li>
            ))}
          </ul>
        </div>
      )}

      {!focused &&
        s.review.map((item) => (
          <div key={item.id} className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-4">
            <div className="text-[11px] font-semibold uppercase tracking-wide text-amber-600 dark:text-amber-400">
              Needs review · {REVIEW_KIND_LABEL[item.kind]}
              {item.host ? ` · ${item.host}` : ""}
            </div>
            <div className="mt-1 text-[15px] font-semibold text-primary">{item.title}</div>
            <div className="text-[13px] text-tertiary">{item.detail}</div>
            <div className="mt-3 flex flex-wrap gap-2">
              {item.choices.map((c, i) => (
                <button
                  key={c}
                  type="button"
                  className={i === 0 ? "app-button-primary !py-1.5" : "app-button-secondary !py-1.5"}
                  onClick={() => s.resolve(item, c)}
                >
                  {c}
                </button>
              ))}
            </div>
          </div>
        ))}

      <div className="space-y-5">
        {days.map((day) => (
          <div key={day}>
            <div className="app-section-title mb-2">{day}</div>
            <ol className="space-y-0">
              {feed
                .filter((e) => e.day === day)
                .map((e) => (
                  <li key={e.id} className="flex gap-3 border-l border-border-subtle py-2 pl-4">
                    <span className="-ml-[23px] mt-0.5 flex h-[14px] w-[14px] items-center justify-center rounded-full bg-surface-active text-[8px] font-bold text-secondary">
                      {e.host[0].toUpperCase()}
                    </span>
                    <div className="min-w-0 flex-1 text-[13px]">
                      <span className="font-medium text-primary">{e.host}</span>{" "}
                      <span className="text-secondary">{e.text}</span>
                      <div className="text-[11px] text-faint">{e.when}</div>
                    </div>
                    {e.canRestore && (
                      <button type="button" className="inline-flex h-7 items-center gap-1 rounded-md px-2 text-[12px] font-medium text-accent hover:bg-surface-hover">
                        <RotateCcw className="h-3 w-3" /> Restore
                      </button>
                    )}
                  </li>
                ))}
            </ol>
          </div>
        ))}
      </div>

      <button
        type="button"
        onClick={() => s.syncNow(s.hosts[0].id)}
        className="fixed bottom-20 right-8 inline-flex items-center gap-1.5 rounded-full bg-accent-dark px-4 py-2.5 text-[13px] font-medium text-white shadow-xl hover:bg-accent"
      >
        <RefreshCw className={cn("h-3.5 w-3.5", s.hosts[0].state === "syncing" && "animate-spin")} />
        Sync now
      </button>

      {removing && (
        <RemoveHostDialog
          host={removing}
          onCancel={() => setRemoving(null)}
          onConfirm={(alsoLeave) => {
            s.removeHost(removing, alsoLeave);
            setRemoving(null);
            setFocus(null);
          }}
        />
      )}
    </div>
  );
}

function SmallButton({ children, onClick, danger }: { children: ReactNode; onClick: () => void; danger?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "rounded-md border border-border-subtle bg-surface px-2 py-1 text-[12px] font-medium hover:bg-surface-hover",
        danger ? "text-red-600 dark:text-red-300" : "text-secondary",
      )}
    >
      {children}
    </button>
  );
}

function AddHostWizard({ s, onExit }: { s: SyncPrototype; onExit: () => void }) {
  const add = useAddHostRun(s.addHost);
  const [name, setName] = useState("gpu-2");
  const [target, setTarget] = useState("yi@gpu-2.local");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const run = add.run;
  const current = run ? STEPS.find((st) => run.steps[st.key].status !== "done") : undefined;
  const currentIndex = run ? (current ? STEPS.indexOf(current) : STEPS.length) : -1;

  return (
    <div className="app-page app-page-narrow !max-w-[640px]">
      <button type="button" onClick={onExit} className="inline-flex w-fit items-center gap-1 text-[13px] text-muted hover:text-secondary">
        <ArrowLeft className="h-3.5 w-3.5" /> Sync
      </button>

      <div className="flex items-center gap-1.5">
        {["Host", ...STEPS.map((st) => st.short)].map((label, i) => (
          <div key={label + i} className="flex flex-1 flex-col gap-1">
            <div className={cn("h-1 rounded-full", i <= currentIndex + 1 ? "bg-accent" : "bg-surface-active")} />
            <span className="text-[11px] text-faint">{label}</span>
          </div>
        ))}
      </div>

      <div className="app-panel space-y-4 p-6">
        {!run && (
          <>
            <h1 className="text-[20px] font-semibold text-primary">Add a host</h1>
            <p className="text-[13px] text-tertiary">
              The app connects over SSH, installs its CLI, gives the host its own key to the Sync remote,
              and switches it to {ACTIVE_PRESET}. The host then syncs by itself every 5 minutes.
            </p>
            <label className="block text-[12px] text-muted">
              Name
              <input className="app-input mt-1 w-full" value={name} onChange={(e) => setName(e.target.value)} />
            </label>
            <label className="block text-[12px] text-muted">
              SSH target
              <input className="app-input mt-1 w-full font-mono" value={target} onChange={(e) => setTarget(e.target.value)} />
            </label>
            <label className="block text-[12px] text-muted">
              Prototype scenario
              <select className="app-input mt-1 w-full" value={scenario} onChange={(e) => setScenario(e.target.value as Scenario)}>
                {SCENARIOS.map((sc) => (
                  <option key={sc.key} value={sc.key}>
                    {sc.label}
                  </option>
                ))}
              </select>
            </label>
            <button type="button" className="app-button-primary w-full" onClick={() => add.start(name, target, scenario)}>
              Continue
            </button>
          </>
        )}

        {run && run.phase !== "done" && current && (
          <>
            <div className="flex items-center gap-2">
              <StepIcon status={run.steps[current.key].status} />
              <h1 className="text-[20px] font-semibold text-primary">{current.label}</h1>
            </div>
            <p className="text-[13px] text-tertiary">{run.steps[current.key].note}</p>
            {run.waitingFor === "paste_key" && (
              <>
                <PasteKey hostName={run.name} />
                <button type="button" className="app-button-primary w-full" onClick={add.proceed}>
                  I've added it
                </button>
              </>
            )}
            {run.waitingFor === "confirm_join" && (
              <>
                <JoinPreview />
                <div className="flex gap-2">
                  <button type="button" className="app-button-primary flex-1" onClick={add.proceed}>
                    Join Sync
                  </button>
                  <button type="button" className="app-button-secondary" onClick={add.reset}>
                    Cancel
                  </button>
                </div>
              </>
            )}
            {run.phase === "blocked" && (
              <button type="button" className="app-button-secondary w-full" onClick={add.reset}>
                Retry
              </button>
            )}
            <ul className="space-y-1 border-t border-border-subtle pt-3 text-[12px] text-muted">
              {STEPS.filter((st) => run.steps[st.key].status === "done").map((st) => (
                <li key={st.key} className="flex items-center gap-1.5">
                  <StepIcon status="done" /> {run.steps[st.key].note}
                </li>
              ))}
            </ul>
          </>
        )}

        {run?.phase === "done" && (
          <>
            <h1 className="text-[20px] font-semibold text-primary">{run.name} is in Sync</h1>
            <ul className="space-y-1.5 text-[13px] text-secondary">
              {STEPS.map((st) => (
                <li key={st.key} className="flex items-start gap-2">
                  <StepIcon status="done" />
                  {run.steps[st.key].note}
                </li>
              ))}
            </ul>
            <button type="button" className="app-button-primary w-full" onClick={onExit}>
              Back to Sync
            </button>
          </>
        )}
      </div>
    </div>
  );
}
