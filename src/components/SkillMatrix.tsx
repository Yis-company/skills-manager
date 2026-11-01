import { type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { useApp } from "../context/AppContext";
import { useOperations } from "../context/OperationsContext";
import { invokeHost } from "../lib/hostCall";
import { isItemActive } from "../lib/operations";
import { getAssignedAgents, type ProjectSkillGroup } from "../lib/projectSkillGroups";
import type { ManagedSkill, ProjectSkill, SkillAgentOutcome, ToolCategory } from "../lib/tauri";
import type { WorkspaceChange } from "../lib/workspaceChanges";
import { AgentMatrix, type MatrixAgent, type MatrixRow } from "./AgentMatrix";
import { statusBadge } from "./librarySkillItem";
import { ProjectItemProgress } from "./ProjectOperationProgress";
import { getSyncStatusMeta } from "./projectSkillItem";

interface Props {
  library?: ManagedSkill[];
  category?: ToolCategory;
  groups?: ProjectSkillGroup[];
  local?: ProjectSkill[];
  projectId?: string;
  linked?: boolean;
  projectName?: string;
  agentKey?: string;
  agents: MatrixAgent[];
  details?: (id: string) => ReactNode;
  inspect?: (id: string) => void;
  selection?: { ids: Set<string>; toggle: (id: string) => void };
}

/** A separate presence control; never edits preset membership or enabled/disabled state. */
export function SkillMatrix(props: Props) {
  const { t } = useTranslation();
  const { activeHostId, activeHost, managedSkills, tools } = useApp();
  const hostId = activeHostId;
  const { operations } = useOperations();

  const projectOperations = props.projectId
    ? operations.filter(
        (operation) => operation.project_id === props.projectId && operation.host_id === hostId,
      )
    : [];

  const operationItems = (relativePath: string | undefined) =>
    projectOperations
      .slice()
      .reverse()
      .find((operation) => operation.items.some((item) => item.relative_path === relativePath))
      ?.items.filter((item) => item.relative_path === relativePath) ?? [];

  const destination = t("workspace.hostDestination", {
    host: activeHost?.name ?? t("hostSwitcher.local"),
    scope: props.projectId
      ? t("workspace.projectDestination", { name: props.projectName })
      : t("workspace.globalDestination"),
  });

  const observed = props.library
    ? props.library.flatMap((skill) => skill.targets.map((target) => target.tool))
    : props.groups
      ? props.groups.flatMap((group) => group.variants.map((copy) => copy.agent))
      : (props.local ?? []).map((copy) => copy.agent);

  const agents = [...props.agents];

  for (const key of new Set(observed)) {
    if (agents.some((agent) => agent.key === key)) continue;
    const known = tools.find((tool) => tool.key === key);

    if (props.category && known && known.category !== props.category) continue;
    agents.push(known ?? { key, display_name: key, installed: false, enabled: false });
  }

  const snapshot = (copy: ProjectSkill | undefined) =>
    JSON.stringify(
      copy
        ? {
            path: copy.relative_path,
            library: copy.center_skill_id,
            enabled: copy.enabled,
            status: copy.sync_status,
            vendored: copy.vendored,
            alias: copy.alias_of,
          }
        : null,
    );

  const row = (
    id: string,
    name: string,
    description: null | string,
    copies: ProjectSkill[],
    library?: ManagedSkill,
    group?: ProjectSkillGroup,
  ): MatrixRow => ({
    id,
    name,
    description,
    details: props.details?.(id),
    onInspect: props.inspect ? () => props.inspect?.(id) : undefined,
    onSelect: props.selection ? () => props.selection?.toggle(id) : undefined,
    selected: props.selection?.ids.has(id),
    metadata: (
      <>
        {operationItems(group?.relative_path ?? copies[0]?.relative_path).length > 0 && (
          <div className="basis-full">
            <ProjectItemProgress
              items={operationItems(group?.relative_path ?? copies[0]?.relative_path)}
              targets={agents}
            />
          </div>
        )}
        <span>
          {props.projectId
            ? group?.relative_path
            : (library?.source_type ?? copies[0]?.relative_path)}
        </span>
        {(() => {
          const badge =
            group || copies[0]
              ? getSyncStatusMeta(t, group?.status ?? copies[0].sync_status)
              : library
                ? statusBadge(library, t)
                : null;

          return badge ? (
            <span className={`rounded-full px-2 py-0.5 ${badge.className}`}>{badge.label}</span>
          ) : null;
        })()}
      </>
    ),
    cells: agents.map((agent) => {
      const copy = copies.find((item) => item.agent === agent.key);
      const target = library?.targets.find((item) => item.tool === agent.key);
      const present = props.library ? Boolean(target) : Boolean(copy);
      const add = !present;

      const edited = props.library
        ? target?.status.startsWith("edited")
        : copy?.sync_status === "diverged" || copy?.sync_status === "project_newer";

      const vendoredLock = props.projectId && copy?.vendored;

      const updating = operationItems(group?.relative_path ?? copies[0]?.relative_path).some(
        isItemActive,
      );

      const disabled = updating
        ? t("operations.destinationBusy")
        : vendoredLock
          ? t("project.vendored.lockedReason", { agents: agent.display_name })
          : edited && !add
            ? t("workspace.editedProtection")
            : add && (agent.installed === false || agent.enabled === false)
              ? t("workspace.unsupported")
              : add && !library && !group?.vendoredVariant
                ? t("project.agentAddRequiresCenter", { agent: agent.display_name })
                : undefined;

      const change: WorkspaceChange = {
        id: JSON.stringify([
          hostId,
          props.projectId ?? null,
          props.agentKey ?? null,
          id,
          agent.key,
        ]),
        name,
        agent: agent.display_name,
        destination,
        add,
        prepare: async () => {
          const read = async () => {
            if (props.library) {
              const all = await invokeHost<ManagedSkill[]>(hostId, "get_managed_skills");
              const skill = all.find((item) => item.id === library?.id);

              if (!skill) throw new Error(t("workspace.inventoryChanged"));
              const current = skill.targets.find((item) => item.tool === agent.key);

              return {
                signature: JSON.stringify(
                  current
                    ? { path: current.target_path, status: current.status, mode: current.mode }
                    : null,
                ),
                variants: new Array<ProjectSkill>(),
              };
            }

            const all = await invokeHost<ProjectSkill[]>(
              hostId,
              props.projectId ? "get_project_skills" : "get_global_local_skills",
              props.projectId ? { projectId: props.projectId } : { agent: props.agentKey },
            );

            const variants = all.filter(
              (item) =>
                item.relative_path.toLowerCase() ===
                (group?.relative_path ?? copies[0]?.relative_path)?.toLowerCase(),
            );

            return {
              signature: snapshot(variants.find((item) => item.agent === agent.key)),
              variants,
            };
          };

          const expected = props.library
            ? JSON.stringify(
                target
                  ? { path: target.target_path, status: target.status, mode: target.mode }
                  : null,
              )
            : snapshot(copy);

          const validate = async () => {
            if ((await read()).signature !== expected)
              throw new Error(t("workspace.inventoryChanged"));
          };

          await validate();
          const warnings: string[] = [];

          if (add && !props.projectId) warnings.push(t("workspace.directWarning"));

          if (!add && copy && !copy.center_skill_id)
            warnings.push(
              t("workspace.localRemovalWarning", {
                name,
                agent: agent.display_name,
                path: copy.path,
              }),
            );

          return {
            validate,
            warnings,
            apply: async () => {
              // Read immediately before each write; changes on other cells are preserved.
              const fresh = await read();

              if (fresh.signature !== expected) throw new Error(t("workspace.inventoryChanged"));

              if (props.projectId && group && props.linked) {
                if (add && library)
                  await invokeHost(hostId, "export_skill_to_project", {
                    skillId: library.id,
                    projectId: props.projectId,
                    agents: [agent.key],
                  });
                else if (copy)
                  await invokeHost(hostId, "delete_project_skill", {
                    projectId: props.projectId,
                    skillRelativePath: copy.relative_path,
                    agent: agent.key,
                    wholeSkill: false,
                  });
              } else if (props.projectId && group) {
                const assigned = getAssignedAgents(fresh.variants);

                const next = add
                  ? [...assigned, agent.key]
                  : assigned.filter((key) => key !== agent.key);

                const outcome = await invokeHost<SkillAgentOutcome>(
                  hostId,
                  "set_project_skill_agents",
                  {
                    projectId: props.projectId,
                    skillRelativePath: group.relative_path,
                    agentKeys: next,
                  },
                );

                if (outcome.failed.length)
                  throw new Error(outcome.failed.map((item) => item.error).join("; "));
              } else if (library) {
                await invokeHost<void>(
                  hostId,
                  add ? "sync_skill_to_tool" : "unsync_skill_from_tool",
                  { skillId: library.id, tool: agent.key },
                );
              } else if (copy && !add) {
                await invokeHost<void>(hostId, "delete_global_local_skill", {
                  agent: agent.key,
                  skillRelativePath: copy.relative_path,
                });
              }
            },
          };
        },
      };

      return {
        agent: agent.key,
        present,
        disabled,
        note:
          copy && !copy.enabled
            ? t("workspace.disabledCopy")
            : edited
              ? t("sync.editedHere", { defaultValue: "Edited here" })
              : undefined,
        change,
      };
    }),
  });

  const rows = props.library
    ? props.library.map((skill) => row(skill.id, skill.name, skill.description, [], skill))
    : props.groups
      ? props.groups.map((group) =>
          row(
            group.id,
            group.name,
            group.description,
            group.variants,
            managedSkills.find((skill) => group.centerSkillIds.includes(skill.id)),
            group,
          ),
        )
      : (props.local ?? []).map((copy) =>
          row(
            `${copy.agent}:${copy.relative_path}`,
            copy.name,
            copy.description,
            [copy],
            managedSkills.find((skill) => skill.id === copy.center_skill_id),
          ),
        );

  return <AgentMatrix rows={rows} agents={agents} destination={destination} />;
}
