import { useNavigate } from "@tanstack/react-router";
import { AlertTriangle, Bot, CheckCircle2, Download, Layers, Plus } from "lucide-react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";

import { AgentControlSetupCard } from "../components/AgentControlSetupCard";
import { CreatorBadge } from "../components/CreatorBadge";
import { useApp } from "../context/AppContext";
import { skillCreator } from "../lib/skillCreator";

export function Dashboard() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { tools, projects, managedSkills, openSkillDetailById } = useApp();

  const enabledAgents = useMemo(
    () => tools.filter((tool) => tool.installed && tool.enabled),
    [tools],
  );

  const totalSkills = managedSkills.length;

  const syncedSkills = useMemo(
    () => managedSkills.filter((s) => s.targets.length > 0).length,
    [managedSkills],
  );

  const divergedCount = useMemo(
    () => projects.reduce((acc, p) => acc + p.sync_health.diverged, 0),
    [projects],
  );

  const recentSkills = useMemo(
    () => [...managedSkills].sort((a, b) => b.updated_at - a.updated_at).slice(0, 5),
    [managedSkills],
  );

  const coverageLabel = totalSkills === 0 ? "0" : `${syncedSkills}/${totalSkills}`;
  const syncCardIcon = divergedCount > 0 ? AlertTriangle : CheckCircle2;
  const syncCardColor = divergedCount > 0 ? "text-amber-400" : "text-emerald-400";
  const syncCardBg = divergedCount > 0 ? "bg-amber-500/[0.08]" : "bg-emerald-500/[0.08]";

  return (
    <div className="app-page app-page-narrow">
      <div className="app-page-header">
        <h1 className="app-page-title">{t("dashboard.greeting")}</h1>
        <p className="app-page-subtitle text-tertiary">
          {t("dashboard.summary", {
            skills: totalSkills,
            agents: enabledAgents.length,
            projects: projects.length,
          })}
        </p>
      </div>

      <AgentControlSetupCard />

      {/* Stats */}
      <div className="grid grid-cols-3 gap-3.5">
        {[
          {
            title: t("dashboard.librarySkills"),
            value: String(totalSkills),
            icon: Layers,
            color: "text-accent-light",
            bg: "bg-accent-bg",
          },
          {
            title: t("dashboard.syncCoverage"),
            value: coverageLabel,
            icon: syncCardIcon,
            color: syncCardColor,
            bg: syncCardBg,
          },
          {
            title: t("dashboard.connectedAgents"),
            value: String(enabledAgents.length),
            icon: Bot,
            color: "text-sky-400",
            bg: "bg-sky-500/[0.08]",
          },
        ].map((stat, i) => {
          const Icon = stat.icon;

          return (
            <div
              key={i}
              className="app-panel flex items-center justify-between px-4 py-4 transition-colors hover:border-border"
            >
              <div>
                <p className="app-section-title mb-1">{stat.title}</p>
                <h3 className="text-xl font-semibold leading-none text-primary">{stat.value}</h3>
              </div>
              <div
                className={`rounded-md p-2 ${stat.bg} ${stat.color} border border-border-subtle`}
              >
                <Icon className="h-4 w-4" />
              </div>
            </div>
          );
        })}
      </div>

      {/* Actions */}
      <div className="flex gap-3">
        <button
          onClick={() => navigate({ to: "/install", search: { tab: "local" } })}
          className="app-button-primary flex-1"
        >
          <Download className="h-4 w-4" />
          {t("dashboard.scanImport")}
        </button>
        <button
          onClick={() => navigate({ to: "/install" })}
          className="app-button-secondary flex-1"
        >
          <Plus className="h-4 w-4 text-tertiary" />
          {t("dashboard.installNew")}
        </button>
      </div>

      {/* Recent skills */}
      {recentSkills.length > 0 && (
        <div>
          <h2 className="app-section-title mb-2.5">{t("dashboard.recentActivity")}</h2>
          <div className="app-panel divide-y divide-border-subtle overflow-hidden">
            {recentSkills.map((skill) => (
              <div
                key={skill.id}
                role="button"
                tabIndex={0}
                onClick={() => {
                  openSkillDetailById(skill.id);
                  navigate({ to: "/my-skills" });
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    openSkillDetailById(skill.id);
                    navigate({ to: "/my-skills" });
                  }
                }}
                className="flex cursor-pointer items-center justify-between px-3.5 py-2.5 transition-colors hover:bg-surface-hover"
              >
                <div className="flex items-center gap-2.5">
                  <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded-[4px] bg-accent-bg text-[13px] font-semibold text-accent-light">
                    {skill.name.charAt(0).toUpperCase()}
                  </div>
                  <div>
                    <h4 className="flex items-center gap-1.5 text-[13px] font-medium text-secondary">
                      {skill.name}
                      <span className="rounded border border-border bg-surface-hover px-1.5 py-px text-[9px] font-normal text-muted">
                        {skill.source_type}
                      </span>
                      <CreatorBadge
                        creator={skillCreator(skill)}
                        hideLocal
                        className="font-normal"
                      />
                    </h4>
                    <p className="mt-px text-[13px] text-muted">
                      {skill.targets.length > 0
                        ? `${t("dashboard.synced")} → ${skill.targets.map((target) => target.tool).join(", ")}`
                        : t("dashboard.notSynced")}
                    </p>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
