import { createRootRoute, createRoute, createRouter, Navigate } from "@tanstack/react-router";

import { CloseActionGuard } from "./components/CloseActionGuard";
import { FirstRunRestoreDialog } from "./components/FirstRunRestoreDialog";
import { HelpDialog } from "./components/HelpDialog";
import { Layout } from "./components/Layout";
import { RemotePickerHost } from "./components/RemoteDirectoryPicker";
import { AppProvider } from "./context/AppContext";
import { Backup } from "./views/Backup";
import { Dashboard } from "./views/Dashboard";
import { parseInstallSearch } from "./views/installSearch";
import { InstallSkills } from "./views/InstallSkills";
import { MySkills } from "./views/MySkills";
import { ProjectDetail } from "./views/ProjectDetail";
import { Settings } from "./views/Settings";
import { CODING_WORKSPACE_CONFIG, LOBSTER_WORKSPACE_CONFIG } from "./views/workspaceConfigs";
import { WorkspaceView } from "./views/WorkspaceView";

const rootRoute = createRootRoute({
  component: () => (
    <AppProvider>
      <Layout />
      <HelpDialog />
      <CloseActionGuard />
      <FirstRunRestoreDialog />
      <RemotePickerHost />
    </AppProvider>
  ),
  notFoundComponent: () => <Navigate to="/" replace />,
});

// Each route is its own const: routes created inline in `addChildren` lose their
// literal paths, and every `to`, `from` and `params` then type-checks as any string.
const dashboardRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: Dashboard,
});

const parseResourceSearch = (search: {
  resource?: unknown;
}): { resource?: "instructions" | "mcps" } => {
  const { resource, ...other } = search;

  return resource === "instructions" || resource === "mcps" ? { ...other, resource } : other;
};

const mySkillsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/my-skills",
  validateSearch: parseResourceSearch,
  component: MySkills,
});

const globalWorkspaceRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/global-workspace/{-$agentKey}",
  validateSearch: parseResourceSearch,
  component: () => <WorkspaceView config={CODING_WORKSPACE_CONFIG} />,
});

const lobsterWorkspaceRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/lobster-workspace/{-$agentKey}",
  validateSearch: parseResourceSearch,
  component: () => <WorkspaceView config={LOBSTER_WORKSPACE_CONFIG} />,
});

const installRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/install",
  validateSearch: parseInstallSearch,
  component: InstallSkills,
});

const backupRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/backup",
  component: Backup,
});

const projectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/project/$id",
  validateSearch: parseResourceSearch,
  component: ProjectDetail,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings/{-$category}",
  component: Settings,
});

const routeTree = rootRoute.addChildren([
  dashboardRoute,
  mySkillsRoute,
  globalWorkspaceRoute,
  lobsterWorkspaceRoute,
  installRoute,
  backupRoute,
  projectRoute,
  settingsRoute,
]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
