// PROTOTYPE (SID-106, SID-112): in-memory state, the simulated "Add host" run, and
// small pieces every variant may reuse. Layouts live in the variant files.
import {
  AlertTriangle,
  CheckCircle2,
  CircleDashed,
  CloudOff,
  Loader2,
  PauseCircle,
  RefreshCw,
  ShieldAlert,
  Sparkles,
  X,
} from "lucide-react";
import { useRef, useState } from "react";
import { toast } from "sonner";

import { cn } from "../../utils";
import {
  ACTIVE_PRESET,
  APP_VERSION,
  type Host,
  INITIAL_HOSTS,
  INITIAL_REVIEW,
  type ReviewItem,
  STATE_LABEL,
  type SyncState,
} from "./data";

// ── Page state ──

export function useSyncPrototype() {
  const [hosts, setHosts] = useState<Host[]>(INITIAL_HOSTS);
  const [review, setReview] = useState<ReviewItem[]>(INITIAL_REVIEW);
  const [noticeOpen, setNoticeOpen] = useState(true);

  const patch = (id: string, p: Partial<Host>) =>
    setHosts((hs) => hs.map((h) => (h.id === id ? { ...h, ...p } : h)));

  return {
    hosts,
    review,
    noticeOpen,
    dismissNotice: () => setNoticeOpen(false),
    resolve: (item: ReviewItem, choice: string) => {
      setReview((r) => r.filter((i) => i.id !== item.id));
      toast.success(`${choice}: recorded in Sync`, {
        description: "Each host applies the choice on its next pull.",
      });
    },
    syncNow: (id: string) => {
      patch(id, { state: "syncing" });
      setTimeout(() => patch(id, { state: "up_to_date", lastSync: "just now" }), 1200);
    },
    togglePause: (host: Host) =>
      patch(host.id, { state: host.state === "paused" ? "up_to_date" : "paused" }),
    undoJoin: (host: Host) =>
      toast(`Undo join on ${host.name}`, {
        description: "Restores the pre-join copy and leaves Sync (prototype: no-op).",
      }),
    deleteBackup: (host: Host) => {
      patch(host.id, { hasPreJoinCopy: false });
      toast(`Deleted the pre-join copy on ${host.name}`);
    },
    removeHost: (host: Host, alsoLeave: boolean) => {
      setHosts((hs) => hs.filter((h) => h.id !== host.id));
      toast(`Removed ${host.name}`, {
        description: alsoLeave
          ? "Deleted its deploy key on GitHub and its timer. Its library and agents stay as they are."
          : "Forgotten by this app only. It keeps syncing on its own.",
      });
    },
    addHost: (host: Host) => setHosts((hs) => [...hs, host]),
  };
}

export type SyncPrototype = ReturnType<typeof useSyncPrototype>;

// ── Simulated "Add host" run ──

export const SCENARIOS = [
  { key: "normal", label: "Everything works" },
  { key: "key_refused", label: "GitHub refuses the deploy key" },
  { key: "old_desktop", label: "Host has an older desktop app" },
  { key: "newer_cli", label: "Host CLI is newer than this app" },
  { key: "no_systemd", label: "Host has no systemd" },
  { key: "unreachable", label: "SSH connection fails" },
  { key: "empty_host", label: "Host library is empty" },
] as const;

export type Scenario = (typeof SCENARIOS)[number]["key"];

export const STEPS = [
  { key: "connect", label: "Connect over SSH", short: "Connect" },
  { key: "cli", label: "Install the Agents Manager CLI", short: "Install CLI" },
  { key: "key", label: "Give the host access to the Sync remote", short: "Access" },
  { key: "preview", label: "Review what joining changes", short: "Review" },
  { key: "timer", label: "Turn on automatic sync", short: "Auto-sync" },
  { key: "apply", label: "Apply the active preset", short: "Apply" },
] as const;

export type StepKey = (typeof STEPS)[number]["key"];
export type StepStatus = "todo" | "running" | "done" | "blocked" | "waiting";

export interface AddHostRun {
  name: string;
  target: string;
  scenario: Scenario;
  phase: "running" | "done" | "blocked";
  steps: Record<StepKey, { status: StepStatus; note?: string }>;
  waitingFor: "paste_key" | "confirm_join" | null;
}

export const JOIN_PREVIEW = {
  adds: ["pdf-tools", "k8s-debug", "Preset “Ops”"],
  replaced: ["frontend-design (Sync's version wins)"],
  activePreset: ACTIVE_PRESET,
  agents: ["Claude Code: +6 skills, −1 (old-writing)", "Codex: +6 skills"],
};

export const DEPLOY_PUBLIC_KEY =
  "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK7q0prototypeonlyx2Vb agents-manager@";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function useAddHostRun(onDone: (host: Host) => void) {
  const [run, setRun] = useState<AddHostRun | null>(null);
  const token = useRef(0);
  const resolver = useRef<(() => void) | null>(null);

  const step = (key: StepKey, status: StepStatus, note?: string) =>
    setRun((r) => r && { ...r, steps: { ...r.steps, [key]: { status, note } } });
  const block = () => setRun((r) => r && { ...r, phase: "blocked" });
  const waitFor = (what: AddHostRun["waitingFor"]) =>
    new Promise<void>((res) => {
      resolver.current = res;
      setRun((r) => r && { ...r, waitingFor: what });
    });

  async function start(name: string, target: string, scenario: Scenario) {
    const mine = ++token.current;
    const alive = () => token.current === mine;
    const steps = Object.fromEntries(STEPS.map((s) => [s.key, { status: "todo" }])) as AddHostRun["steps"];

    setRun({ name, target, scenario, phase: "running", steps, waitingFor: null });

    step("connect", "running", `ssh ${target}`);
    await sleep(900);
    if (!alive()) return;
    if (scenario === "unreachable") {
      step("connect", "blocked", `Couldn't connect to ${target}: connection timed out. Check the address, then retry.`);
      return block();
    }
    step("connect", "done", "Connected · Linux arm64");

    step("cli", "running", `Downloading CLI ${APP_VERSION} on this Mac and copying it over SSH`);
    await sleep(1200);
    if (!alive()) return;
    if (scenario === "old_desktop") {
      step("cli", "blocked", `${name} runs the Agents Manager desktop app 1.44. Update Agents Manager on ${name} first, then retry.`);
      return block();
    }
    if (scenario === "newer_cli") {
      step("cli", "blocked", `${name} runs CLI 1.47.0, newer than this app (${APP_VERSION}). Update this app to manage it.`);
      return block();
    }
    step("cli", "done", `CLI ${APP_VERSION} installed`);

    step("key", "running", "Creating a deploy key on the host and adding it to GitHub");
    await sleep(900);
    if (!alive()) return;
    if (scenario === "key_refused") {
      step("key", "waiting", "GitHub refused to add the key. Add it yourself with write access, then continue.");
      await waitFor("paste_key");
      if (!alive()) return;
    }
    step("key", "done", `Deploy key “Agents Manager · ${name}” has write access`);

    if (scenario === "empty_host") {
      step("preview", "done", "The host's library is empty; it takes Sync as is");
    } else {
      step("preview", "waiting", "Saved a pre-join copy. Review the changes, then join.");
      await waitFor("confirm_join");
      if (!alive()) return;
      step("preview", "done", "Joined; pre-join copy kept for Undo join");
    }

    step("timer", "running", "Installing the 5-minute systemd user timer");
    await sleep(600);
    if (!alive()) return;
    step(
      "timer",
      "done",
      scenario === "no_systemd"
        ? "No systemd user services: this host syncs only when you connect or press Sync now"
        : "Syncs every 5 minutes, even when this Mac is off",
    );

    step("apply", "running", `Switching agents to ${ACTIVE_PRESET}`);
    await sleep(1100);
    if (!alive()) return;
    step("apply", "done", `${ACTIVE_PRESET} applied: 12 skills on Claude Code, 11 on Codex`);
    setRun((r) => r && { ...r, phase: "done" });
    onDone({
      id: `host-${mine}`,
      name,
      kind: "ssh",
      target,
      state: "up_to_date",
      version: APP_VERSION,
      lastSync: "just now",
      agents: ["Claude Code", "Codex"],
      editedHere: 0,
      failures: 0,
      runner: scenario === "no_systemd" ? "on-connect" : "systemd",
      hasPreJoinCopy: scenario !== "empty_host",
      agentLog: [{ when: "now", text: `Joined Sync and applied ${ACTIVE_PRESET}` }],
    });
  }

  return {
    run,
    start,
    proceed: () => {
      const r = resolver.current;

      resolver.current = null;
      setRun((x) => x && { ...x, waitingFor: null });
      r?.();
    },
    reset: () => {
      token.current++;
      resolver.current = null;
      setRun(null);
    },
  };
}

// ── Small shared pieces ──

const PILL: Record<SyncState, string> = {
  up_to_date: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
  syncing: "bg-accent-bg text-accent",
  paused: "bg-surface-hover text-muted",
  offline: "bg-surface-hover text-muted",
  needs_review: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
  update_required: "bg-red-500/10 text-red-600 dark:text-red-300",
  update_pending: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
};

const ICON: Record<SyncState, typeof CheckCircle2> = {
  up_to_date: CheckCircle2,
  syncing: Loader2,
  paused: PauseCircle,
  offline: CloudOff,
  needs_review: AlertTriangle,
  update_required: ShieldAlert,
  update_pending: RefreshCw,
};

export function StatePill({ state, className }: { state: SyncState; className?: string }) {
  const Icon = ICON[state];

  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 whitespace-nowrap rounded-full px-2 py-0.5 text-[12px] font-medium",
        PILL[state],
        className,
      )}
    >
      <Icon className={cn("h-3 w-3", state === "syncing" && "animate-spin")} />
      {STATE_LABEL[state]}
    </span>
  );
}

export function StateDot({ state }: { state: SyncState }) {
  const color = {
    up_to_date: "bg-emerald-500",
    syncing: "bg-accent animate-pulse",
    paused: "bg-zinc-400",
    offline: "bg-zinc-400",
    needs_review: "bg-amber-500",
    update_required: "bg-red-500",
    update_pending: "bg-amber-500",
  }[state];

  return <span className={cn("inline-block h-2 w-2 shrink-0 rounded-full", color)} />;
}

export function StepIcon({ status }: { status: StepStatus }) {
  if (status === "done") return <CheckCircle2 className="h-4 w-4 text-emerald-500" />;
  if (status === "running") return <Loader2 className="h-4 w-4 animate-spin text-accent" />;
  if (status === "blocked") return <ShieldAlert className="h-4 w-4 text-red-500" />;
  if (status === "waiting") return <AlertTriangle className="h-4 w-4 text-amber-500" />;

  return <CircleDashed className="h-4 w-4 text-faint" />;
}

export function BackupIsSyncNotice({ onDismiss }: { onDismiss: () => void }) {
  return (
    <div className="flex items-start gap-3 rounded-xl border border-accent-border bg-accent-bg px-4 py-3">
      <Sparkles className="mt-0.5 h-4 w-4 shrink-0 text-accent" />
      <div className="min-w-0 flex-1 text-[13px] leading-5 text-secondary">
        <div className="font-semibold text-primary">Backup is now Sync</div>
        <ul className="mt-1 list-disc space-y-0.5 pl-4 text-tertiary">
          <li>Other hosts can join and share this library.</li>
          <li>Clicking a preset now switches it on every host.</li>
          <li>Nothing changes on your agents until you switch.</li>
        </ul>
      </div>
      <button
        type="button"
        aria-label="Dismiss"
        onClick={onDismiss}
        className="rounded-md p-1 text-muted hover:bg-surface-hover"
      >
        <X className="h-4 w-4" />
      </button>
    </div>
  );
}

export function RemoveHostDialog({
  host,
  onCancel,
  onConfirm,
}: {
  host: Host;
  onCancel: () => void;
  onConfirm: (alsoLeave: boolean) => void;
}) {
  const [alsoLeave, setAlsoLeave] = useState(true);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-[440px] rounded-xl border border-border-subtle bg-bg-secondary p-5 shadow-2xl">
        <h2 className="text-[15px] font-semibold text-primary">Remove {host.name}?</h2>
        <p className="mt-2 text-[13px] leading-5 text-tertiary">
          This app forgets {host.name}. Its library and the skills on its agents stay as they are.
        </p>
        <label className="mt-4 flex items-start gap-2 rounded-lg border border-border-subtle bg-surface p-3 text-[13px] text-secondary">
          <input
            type="checkbox"
            checked={alsoLeave}
            onChange={(e) => setAlsoLeave(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            <span className="font-medium">Also remove from Sync</span>
            <span className="block text-[12px] text-muted">
              Deletes its deploy key on GitHub and its timer, so it stops syncing. Works even if
              the host is offline.
            </span>
          </span>
        </label>
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" className="app-button-secondary" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="app-button-primary !border-red-500/40 !bg-red-600 hover:!bg-red-500"
            onClick={() => onConfirm(alsoLeave)}
          >
            Remove host
          </button>
        </div>
      </div>
    </div>
  );
}

export function JoinPreview() {
  return (
    <div className="space-y-3 text-[13px] text-secondary">
      <div>
        <div className="app-section-title mb-1">Adds to Sync from this host</div>
        <ul className="list-disc pl-5 text-tertiary">
          {JOIN_PREVIEW.adds.map((a) => (
            <li key={a}>{a}</li>
          ))}
        </ul>
      </div>
      <div>
        <div className="app-section-title mb-1">Replaced by Sync's version</div>
        <ul className="list-disc pl-5 text-tertiary">
          {JOIN_PREVIEW.replaced.map((a) => (
            <li key={a}>{a}</li>
          ))}
        </ul>
      </div>
      <div>
        <div className="app-section-title mb-1">Active preset becomes {JOIN_PREVIEW.activePreset}</div>
        <ul className="list-disc pl-5 text-tertiary">
          {JOIN_PREVIEW.agents.map((a) => (
            <li key={a}>{a}</li>
          ))}
        </ul>
      </div>
      <p className="text-[12px] text-muted">
        A copy of this host's library is saved first. You can undo the join later.
      </p>
    </div>
  );
}

export function PasteKey({ hostName }: { hostName: string }) {
  return (
    <div className="space-y-2 text-[13px]">
      <p className="text-tertiary">
        Open the repository's Settings › Deploy keys, add this key with <b>Allow write access</b>,
        then continue.
      </p>
      <pre className="overflow-x-auto rounded-lg border border-border-subtle bg-surface p-2 font-mono text-[11px] text-secondary">
        {DEPLOY_PUBLIC_KEY}
        {hostName}
      </pre>
      <button
        type="button"
        className="text-[12px] font-medium text-accent hover:underline"
        onClick={() => toast("Opened GitHub › Deploy keys (prototype)")}
      >
        Open GitHub deploy keys
      </button>
    </div>
  );
}
