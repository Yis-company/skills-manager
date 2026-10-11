// PROTOTYPE (SID-106, SID-112): stub data for the Sync page variants. Nothing here
// talks to the backend; every action only updates in-memory state or shows a toast.

export type SyncState =
  | "up_to_date"
  | "syncing"
  | "paused"
  | "offline"
  | "needs_review"
  | "update_required"
  | "update_pending";

export const STATE_LABEL: Record<SyncState, string> = {
  up_to_date: "Up to date",
  syncing: "Syncing",
  paused: "Paused",
  offline: "Offline",
  needs_review: "Needs review",
  update_required: "Update required",
  update_pending: "Update pending",
};

export interface Host {
  id: string;
  name: string;
  kind: "local" | "ssh";
  target?: string;
  state: SyncState;
  reason?: string;
  version: string;
  lastSync: string;
  agents: string[];
  editedHere: number;
  failures: number;
  /** What runs this host's 5-minute sync. */
  runner: "app" | "systemd" | "on-connect";
  hasPreJoinCopy: boolean;
  agentLog: { when: string; text: string; tone?: "warn" | "error" }[];
}

export const SYNC_REMOTE = "github.com/ybtam/skills-manager-backup";
export const ACTIVE_PRESET = "Frontend";
export const APP_VERSION = "1.46.0";

export const INITIAL_HOSTS: Host[] = [
  {
    id: "local",
    name: "Mac",
    kind: "local",
    state: "up_to_date",
    version: APP_VERSION,
    lastSync: "2 min ago",
    agents: ["Claude Code", "Codex", "Cursor"],
    editedHere: 0,
    failures: 0,
    runner: "app",
    hasPreJoinCopy: false,
    agentLog: [
      { when: "10:42", text: "Switched to Frontend: added 4 skills, removed 2" },
      { when: "09:15", text: "Added shadcn to Claude Code, Codex, Cursor" },
    ],
  },
  {
    id: "remote-agent",
    name: "Remote-agent",
    kind: "ssh",
    target: "dev@9.9.9.241",
    state: "needs_review",
    reason: "1 copy edited here",
    version: APP_VERSION,
    lastSync: "4 min ago",
    agents: ["Claude Code", "Codex"],
    editedHere: 1,
    failures: 0,
    runner: "systemd",
    hasPreJoinCopy: true,
    agentLog: [
      { when: "10:44", text: "Followed switch to Frontend: added 4 skills, removed 2" },
      {
        when: "10:44",
        text: "pdf-tools on Claude Code is edited here; not updated",
        tone: "warn",
      },
      { when: "08:02", text: "Joined Sync; added 2 skills and 1 preset to Sync" },
    ],
  },
  {
    id: "gpu-box",
    name: "gpu-box",
    kind: "ssh",
    target: "yi@gpu-box.local",
    state: "update_pending",
    reason: "CLI 1.45.0; updates when the app can reach it",
    version: "1.45.0",
    lastSync: "3 h ago",
    agents: ["Claude Code"],
    editedHere: 0,
    failures: 1,
    runner: "systemd",
    hasPreJoinCopy: false,
    agentLog: [
      { when: "07:31", text: "Could not reach github.com:22; retrying", tone: "error" },
      { when: "07:26", text: "Added frontend-design to Claude Code" },
    ],
  },
  {
    id: "build-runner",
    name: "build-runner",
    kind: "ssh",
    target: "ci@10.0.0.12",
    state: "offline",
    reason: "No systemd; syncs only on connect or Sync now",
    version: APP_VERSION,
    lastSync: "yesterday",
    agents: ["Codex"],
    editedHere: 0,
    failures: 0,
    runner: "on-connect",
    hasPreJoinCopy: false,
    agentLog: [{ when: "Yesterday", text: "Synced on connect; nothing to change" }],
  },
];

export interface ReviewItem {
  id: string;
  kind: "skill" | "preset_name" | "instruction" | "edited_here";
  title: string;
  detail: string;
  host?: string;
  choices: string[];
}

export const INITIAL_REVIEW: ReviewItem[] = [
  {
    id: "r1",
    kind: "skill",
    title: "frontend-design",
    detail: "Changed on Mac and on Remote-agent since the last sync.",
    choices: ["Keep Remote-agent's version", "Keep Mac's version", "Keep both"],
  },
  {
    id: "r2",
    kind: "preset_name",
    title: "Presets “Writing” and “Writing (2)”",
    detail: "Created on Mac and on gpu-box at the same time.",
    choices: ["Keep both…", "Combine", "Keep one…"],
  },
  {
    id: "r3",
    kind: "instruction",
    title: "Global CLAUDE.md",
    detail: "Changed on Mac and on Remote-agent since the last sync.",
    choices: ["Keep Remote-agent's version", "Keep Mac's version"],
  },
  {
    id: "r4",
    kind: "edited_here",
    title: "pdf-tools on Claude Code",
    detail: "Edited in ~/.claude/skills/pdf-tools. Sync has a newer version.",
    host: "Remote-agent",
    choices: ["Use Sync version", "Keep my edit"],
  },
];

export const REVIEW_KIND_LABEL: Record<ReviewItem["kind"], string> = {
  skill: "Skill changed on two hosts",
  preset_name: "Same preset name",
  instruction: "Instruction changed on two hosts",
  edited_here: "Edited here",
};

export interface HistoryEntry {
  id: string;
  day: string;
  when: string;
  host: string;
  text: string;
  kind: "switch" | "add" | "edit" | "replaced" | "join" | "update" | "remove";
  canRestore?: boolean;
}

export const HISTORY: HistoryEntry[] = [
  {
    id: "h1",
    day: "Today",
    when: "10:42",
    host: "Mac",
    kind: "switch",
    text: "Switched the active preset to Frontend",
  },
  {
    id: "h2",
    day: "Today",
    when: "10:20",
    host: "gpu-box",
    kind: "edit",
    text: "Turned off Cursor for frontend-design in Frontend",
  },
  {
    id: "h3",
    day: "Today",
    when: "09:58",
    host: "Mac",
    kind: "replaced",
    text: "Your rename of “Research” was replaced by gpu-box's change",
    canRestore: true,
  },
  {
    id: "h4",
    day: "Today",
    when: "09:15",
    host: "Mac",
    kind: "add",
    text: "Added shadcn to the library and to Frontend",
  },
  {
    id: "h5",
    day: "Today",
    when: "08:02",
    host: "Remote-agent",
    kind: "join",
    text: "Joined Sync: added 2 skills and 1 preset",
  },
  {
    id: "h6",
    day: "Yesterday",
    when: "17:40",
    host: "Mac",
    kind: "update",
    text: "Updated pdf-tools from its source",
  },
  {
    id: "h7",
    day: "Yesterday",
    when: "11:03",
    host: "build-runner",
    kind: "remove",
    text: "Removed old-writing from the library",
    canRestore: true,
  },
];
