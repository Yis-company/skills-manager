import { FolderOpen, Globe, Layers } from "lucide-react";
import { useTranslation } from "react-i18next";

import { useApp } from "../context/AppContext";
import { compactHomePath } from "../utils";
import { AgentIcon } from "./AgentIcon";
import type { WorkspaceScope } from "./ResourceWorkspace";

export function WorkspaceHeader({ scope }: { scope: WorkspaceScope }) {
  const { t } = useTranslation();
  const { tools, projects, activeHost, managedSkills } = useApp();
  const agent = tools.find((tool) => tool.key === scope.agentKey);
  const project = projects.find((item) => item.id === scope.projectId);

  const title =
    scope.kind === "library"
      ? t("mySkills.title")
      : scope.kind === "project"
        ? project?.name
        : (agent?.display_name ??
          t(scope.category === "lobster" ? "lobsterWorkspace.title" : "globalWorkspace.title"));

  const path = project?.path ?? agent?.skills_dir;

  return (
    <header className="app-page-header">
      <h1 aria-label={title} className="app-page-title flex items-center gap-2.5">
        {agent ? (
          <AgentIcon agentKey={agent.key} displayName={agent.display_name} className="h-6 w-6" />
        ) : scope.kind === "library" ? (
          <Layers className="h-5 w-5 text-accent" />
        ) : scope.kind === "project" ? (
          <FolderOpen className="h-5 w-5 text-accent" />
        ) : (
          <Globe className="h-5 w-5 text-accent" />
        )}
        {title}
        {scope.kind === "library" && <span className="app-badge">{managedSkills.length}</span>}
      </h1>
      <p className="app-page-subtitle break-all">
        {path ? <span>{compactHomePath(path)} · </span> : null}
        {activeHost?.name ?? t("hostSwitcher.local")} ·{" "}
        {scope.kind === "library"
          ? t("mySkills.title")
          : scope.kind === "project"
            ? t("workspace.projectDestination", { name: project?.name })
            : t("workspace.globalDestination")}
      </p>
    </header>
  );
}
