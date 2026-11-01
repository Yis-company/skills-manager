import { type ReactNode, useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useApp } from "../context/AppContext";
import { useWorkspaceChanges } from "../context/WorkspaceChangesContext";
import { invokeHost } from "../lib/hostCall";
import { AgentMatrix, type MatrixAgent, type MatrixRow } from "./AgentMatrix";
import type {
  InstructionsRequest,
  InstructionTarget,
  McpRequest,
  McpTarget,
  WorkspaceScope,
} from "./ResourceWorkspace";

type Resource = {
  id: string;
  name: string;
  description?: string;
  revision: string;
  transport?: string;
  auth?: unknown;
  server?: unknown;
};

type Entry = { name: string; managedId?: string; status: string };

type Deployment = {
  id: string;
  instruction_id: string;
  target: { agent_key: string; project_id?: null | string; relative_dir?: null | string };
  files: { path: string }[];
};

type Capability = {
  agentKey: string;
  scopes: { kind: string; supported: boolean }[];
  features: string[];
  limitations: string[];
};

type Inventory = { entries?: Entry[]; error?: string };

interface Props {
  onEditingChange: (editing: boolean) => void;
  kind: "instructions" | "mcps";
  scope: WorkspaceScope;
  projectId?: string;
  agents: MatrixAgent[];
  editor: (initialId: null | string) => ReactNode;
}

function mcpTarget(agentKey: string, projectId?: string) {
  const target: McpTarget = { agentKey };

  if (projectId) target.projectId = projectId;

  return target;
}

function instructionTarget(agent_key: string, project_id?: string) {
  const target: InstructionTarget = { agent_key };

  if (project_id) target.project_id = project_id;

  return target;
}

/** Library metadata and target inventory stay primary; native editors are opened deliberately. */
export function ResourceMatrix({ kind, scope, projectId, agents, editor, onEditingChange }: Props) {
  const { t } = useTranslation();
  const { activeHostId: hostId, activeHost, projects } = useApp();
  const changes = useWorkspaceChanges();
  const [items, setItems] = useState<Resource[]>([]);
  const [inventory, setInventory] = useState<Record<string, Inventory>>({});
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [capabilities, setCapabilities] = useState<Capability[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [editing, setEditing] = useState<null | string | undefined>(undefined);
  useEffect(() => () => onEditingChange(false), [onEditingChange]);
  const agentKeys = agents.map((agent) => agent.key).join(",");

  const request = <T,>(body: InstructionsRequest | McpRequest) =>
    invokeHost<T>(hostId, kind === "mcps" ? "mcps_request" : "instructions_request", {
      request: body,
    });

  const load = useCallback(
    async (current: () => boolean = () => true) => {
      await Promise.resolve();

      if (!current()) return;
      setLoading(true);
      setError("");

      try {
        const library = await invokeHost<{ definitions?: Resource[]; items?: Resource[] }>(
          hostId,
          kind === "mcps" ? "mcps_request" : "instructions_request",
          { request: { action: "list" } },
        );

        if (!current()) return;
        setItems(library.definitions ?? library.items ?? []);

        if (kind === "instructions") {
          const deployed = await invokeHost<{ items: Deployment[] }>(
            hostId,
            "instructions_request",
            { request: { action: "deployments" } },
          );

          if (current()) setDeployments(deployed.items);
        } else {
          const caps = await invokeHost<{ targets: Capability[] }>(hostId, "mcps_request", {
            request: { action: "capabilities" },
          });

          const results = await Promise.all(
            agentKeys
              .split(",")
              .filter(Boolean)
              .map(async (agentKey) => {
                try {
                  const result = await invokeHost<{ entries: Entry[] }>(hostId, "mcps_request", {
                    request: { action: "inspect", target: mcpTarget(agentKey, projectId) },
                  });

                  return [agentKey, { entries: result.entries }] as const;
                } catch (error) {
                  return [agentKey, { error: String(error) }] as const;
                }
              }),
          );

          if (current()) {
            setCapabilities(caps.targets);
            setInventory(Object.fromEntries(results));
          }
        }
      } catch (error) {
        if (current()) {
          setError(error instanceof Error ? error.message : String(error));
          setInventory({});
        }
      } finally {
        if (current()) setLoading(false);
      }
    },
    [hostId, kind, projectId, agentKeys],
  );

  useEffect(() => {
    let current = true;
    void Promise.resolve().then(() => load(() => current));
    const refresh = () => void load(() => current);
    window.addEventListener("workspace-targets-changed", refresh);

    return () => {
      current = false;
      window.removeEventListener("workspace-targets-changed", refresh);
    };
  }, [load]);

  const destination = t("workspace.hostDestination", {
    host: activeHost?.name ?? t("hostSwitcher.local"),
    scope: projectId
      ? t("workspace.projectDestination", {
          name: projects.find((project) => project.id === projectId)?.name ?? projectId,
        })
      : t("workspace.globalDestination"),
  });

  const openEditor = async (id: null | string) => {
    if (await changes.confirmLeave()) {
      setEditing(id);
      onEditingChange(true);
    }
  };

  if (editing !== undefined)
    return (
      <div className="space-y-4">
        <button
          className="app-button-secondary"
          onClick={async () => {
            if (await changes.confirmLeave()) {
              setEditing(undefined);
              onEditingChange(false);
              void load();
            }
          }}
        >
          {t("workspace.backToCollection")}
        </button>
        {editor(editing)}
      </div>
    );

  const rows: MatrixRow[] = items
    .filter((item) =>
      `${item.name} ${item.description ?? ""}`.toLowerCase().includes(query.toLowerCase()),
    )
    .map((item) => ({
      id: item.id,
      name: item.name,
      description: item.description,
      metadata: item.transport ?? t("help.items.sync.title", { defaultValue: "Library" }),
      onInspect: () => void openEditor(item.id),
      cells: agents.map((agent) => {
        const matches = deployments.filter(
          (deployment) =>
            deployment.instruction_id === item.id &&
            deployment.target.agent_key === agent.key &&
            (deployment.target.project_id ?? null) === (projectId ?? null),
        );

        const entries = inventory[agent.key]?.entries;
        const existing = entries?.find((entry) => entry.managedId === item.id);
        const present = kind === "instructions" ? matches.length > 0 : Boolean(existing);
        const capability = capabilities.find((entry) => entry.agentKey === agent.key);

        const supported =
          kind === "instructions"
            ? !(agent.key === "cursor" && !projectId)
            : Boolean(
                capability?.scopes.some(
                  (entry) => entry.kind === (projectId ? "project" : "global") && entry.supported,
                ) && capability.features.includes(item.transport ?? ""),
              );

        const credentialsUnsupported =
          kind === "mcps" &&
          agent.key === "antigravity" &&
          Boolean(
            (item.auth && Object.keys(item.auth).length) ||
            /\$\{env:/.test(JSON.stringify(item.server)),
          );

        const disabled =
          error || inventory[agent.key]?.error
            ? t("workspace.unknown")
            : !supported || credentialsUnsupported
              ? capability?.limitations.join(" ") || t("workspace.unsupported")
              : kind === "instructions" &&
                  (matches.length > 1 || matches.some((entry) => entry.target.relative_dir))
                ? t("workspace.multipleDirectories")
                : kind === "mcps" && !existing && entries?.some((entry) => entry.name === item.name)
                  ? t("workspace.nameConflict")
                  : undefined;

        const expected = JSON.stringify(kind === "instructions" ? matches : (existing ?? null));

        return {
          agent: agent.key,
          present: disabled === t("workspace.unknown") ? null : present,
          disabled,
          change: {
            id: JSON.stringify([hostId, projectId ?? null, kind, item.id, agent.key]),
            name: item.name,
            agent: agent.display_name,
            destination,
            add: !present,
            prepare: async () => {
              const target = mcpTarget(agent.key, projectId);

              const read = async () => {
                if (kind === "instructions") {
                  const result = await request<{ items: Deployment[] }>({ action: "deployments" });

                  return JSON.stringify(
                    result.items.filter(
                      (entry) =>
                        entry.instruction_id === item.id &&
                        entry.target.agent_key === agent.key &&
                        (entry.target.project_id ?? null) === (projectId ?? null),
                    ),
                  );
                }

                const result = await request<{ entries: Entry[] }>({ action: "inspect", target });

                return JSON.stringify(
                  result.entries.find((entry) => entry.managedId === item.id) ?? null,
                );
              };

              const validate = async () => {
                if ((await read()) !== expected) throw new Error(t("workspace.inventoryChanged"));
              };

              await validate();

              if (present)
                return {
                  validate,
                  warnings: [
                    kind === "instructions"
                      ? t("workspace.detachInstructions")
                      : t("workspace.removeMcp"),
                  ],
                  apply: async () => {
                    await validate();
                    await request(
                      kind === "instructions"
                        ? { action: "undeploy", deployment_id: matches[0].id }
                        : {
                            action: "undeploy",
                            target,
                            definitionId: item.id,
                            serverName: existing!.name,
                          },
                    );
                  },
                };

              const preview = await request<{
                previewId?: string;
                preview_id?: string;
                changes: {
                  path?: string;
                  name?: string;
                  status?: string;
                  conflict?: unknown;
                  warnings?: string[];
                }[];
                warnings?: string[];
              }>({
                action: "preview",
                ...(kind === "instructions"
                  ? { target: instructionTarget(agent.key, projectId), instruction_id: item.id }
                  : {
                      target,
                      operations: [{ kind: "deploy", definitionId: item.id, conflict: "keep" }],
                    }),
              });

              if (preview.changes.some((change) => change.status === "conflict" || change.conflict))
                throw new Error(t("workspace.reviewConflictsInEditor"));

              const previewId = kind === "instructions" ? preview.preview_id : preview.previewId;

              if (!previewId) throw new Error(t("workspace.interrupted"));

              return {
                validate,
                warnings: [
                  ...(preview.warnings ?? []),
                  ...preview.changes
                    .flatMap((change) => [
                      change.path ?? change.name ?? "",
                      ...(change.warnings ?? []),
                    ])
                    .filter(Boolean),
                  ...(kind === "mcps" ? [t("workspace.mcpCredentials")] : []),
                ],
                apply: async () => {
                  await validate();

                  const result = await request<{
                    failed?: { path: string; error: string }[];
                    partial?: boolean;
                  }>({
                    action: "apply",
                    ...(kind === "instructions"
                      ? { preview_id: previewId, resolutions: {} }
                      : { previewId: previewId }),
                  });

                  if (result.failed?.length || result.partial)
                    throw new Error(
                      result.failed?.map((entry) => `${entry.path}: ${entry.error}`).join("; ") ||
                        t("workspace.interrupted"),
                    );
                },
              };
            },
          },
        };
      }),
    }));

  return (
    <div className="space-y-4">
      <div className="app-toolbar">
        <label className="min-w-[200px] flex-1">
          <span className="sr-only">{t("common.search")}</span>
          <input
            aria-label={t("workspace.searchResources")}
            className="app-input w-full"
            value={query}
            placeholder={t("workspace.searchResources")}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <button className="app-button-secondary" onClick={() => void load()} disabled={loading}>
          {t("workspace.refresh")}
        </button>
        <button className="app-button-secondary" onClick={() => void openEditor("")}>
          {t("workspace.manageResources")}
        </button>
        <button className="app-button-primary" onClick={() => void openEditor(null)}>
          {t("workspace.newResource")}
        </button>
      </div>
      {error && (
        <p role="alert" className="text-[13px] text-danger">
          {error}
        </p>
      )}
      {loading ? (
        <p role="status" className="text-[13px] text-muted">
          {t("common.loading")}
        </p>
      ) : (
        <AgentMatrix
          rows={rows}
          agents={agents}
          destination={destination}
          hint={t("workspace.managedPresenceHint")}
        />
      )}
      {scope.kind === "project" && kind === "instructions" && (
        <p className="text-[12px] text-muted">{t("workspace.worktreeEditor")}</p>
      )}
    </div>
  );
}
