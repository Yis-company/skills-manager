import { FilePlus2, Loader2 } from "lucide-react";
import { useId } from "react";
import { useTranslation } from "react-i18next";

import { useApp } from "../context/AppContext";
import type { useCreateSkill } from "../hooks/useCreateSkill";
import { cn } from "../utils";
import { MarkdownEditor } from "./MarkdownEditor";
import { StatusBanner } from "./StatusBanner";

const LABEL = "mb-1 block text-[13px] font-medium text-tertiary";

const HINT = "ml-1.5 text-[12px] font-normal text-muted";

/** The install page's create tab: write a new skill into the library. */
export function CreateSkillTab({ form }: { form: ReturnType<typeof useCreateSkill> }) {
  const { t } = useTranslation();
  const { tools, presets, projects } = useApp();
  const id = useId();
  const nameErrorId = `${id}-name-error`;
  const { draft, update } = form;
  const nameError = form.nameTaken ?? (form.nameInvalid ? t("install.create.nameInvalid") : null);
  const agents = tools.filter((tool) => tool.installed && tool.enabled);
  const agentName = (key: string) => tools.find((tool) => tool.key === key)?.display_name ?? key;

  return (
    <div className="animate-in fade-in space-y-4 pb-8 duration-300">
      {form.error && (
        <div role="alert">
          <StatusBanner
            compact
            tone="danger"
            title={t("common.requestFailed")}
            description={form.error}
          />
        </div>
      )}
      {form.partial && (
        <div role="alert">
          <StatusBanner
            compact
            title={t("install.create.partialTitle", { name: form.partial.name })}
            description={form.partial.failed
              .map((target) =>
                t("install.create.failedTarget", {
                  target: target.kind === "agent" ? agentName(target.name) : target.name,
                  error: target.error,
                }),
              )
              .join(" ")}
          />
        </div>
      )}
      <form
        className="app-panel max-w-3xl p-5"
        onSubmit={(event) => {
          event.preventDefault();
          void form.submit();
        }}
      >
        <h2 className="mb-1 text-[14px] font-semibold text-primary">{t("install.create.title")}</h2>
        <p className="mb-4 text-[13px] text-muted">{t("install.create.description")}</p>

        <div className="grid gap-4 sm:grid-cols-2">
          <div>
            <label className={LABEL} htmlFor={`${id}-name`}>
              {t("install.create.name")}
            </label>
            <input
              id={`${id}-name`}
              type="text"
              value={draft.name}
              onChange={(event) => update({ name: event.target.value })}
              placeholder="release-notes"
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              aria-invalid={nameError ? true : undefined}
              aria-describedby={nameError ? nameErrorId : undefined}
              className="app-input w-full bg-background font-mono"
            />
            {nameError && (
              <p id={nameErrorId} role="alert" className="mt-1 text-[12px] text-danger">
                {nameError}
              </p>
            )}
          </div>
          <div>
            <label className={LABEL} htmlFor={`${id}-description`}>
              {t("install.create.skillDescription")}
              <span className={HINT}>{t("install.create.skillDescriptionHint")}</span>
            </label>
            <textarea
              id={`${id}-description`}
              value={draft.description}
              onChange={(event) => update({ description: event.target.value })}
              rows={2}
              maxLength={1024}
              className="app-input h-auto w-full bg-background py-2"
            />
          </div>
        </div>

        <div className="mt-4">
          <p className={cn(LABEL, "mb-0")}>{t("install.create.instructions")}</p>
          <MarkdownEditor
            value={draft.instructions}
            onChange={(instructions) => update({ instructions })}
            ariaLabel={t("install.create.instructions")}
            rows={8}
          />
        </div>

        <fieldset className="mt-4">
          <legend className={LABEL}>
            {t("install.create.addToAgents")}
            <span className={HINT}>{t("install.create.optional")}</span>
          </legend>
          {agents.length === 0 ? (
            <p className="text-[13px] text-muted">{t("install.create.noAgents")}</p>
          ) : (
            <div className="flex flex-wrap gap-2">
              {agents.map((agent) => {
                const checked = draft.agents.includes(agent.key);

                return (
                  <label
                    key={agent.key}
                    className={cn(
                      "inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-3 py-1 text-[13px]",
                      checked
                        ? "border-accent-border bg-accent-bg text-accent"
                        : "border-border-subtle text-muted",
                    )}
                  >
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={checked}
                      onChange={() =>
                        update({
                          agents: checked
                            ? draft.agents.filter((key) => key !== agent.key)
                            : [...draft.agents, agent.key],
                        })
                      }
                    />
                    {agent.display_name}
                  </label>
                );
              })}
            </div>
          )}
        </fieldset>

        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          <div>
            <label className={LABEL} htmlFor={`${id}-preset`}>
              {t("install.create.preset")}
              <span className={HINT}>{t("install.create.optional")}</span>
            </label>
            <select
              id={`${id}-preset`}
              value={draft.presetId}
              onChange={(event) => update({ presetId: event.target.value })}
              className="app-input w-full bg-background"
            >
              <option value="">{t("install.create.none")}</option>
              {presets.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.name}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label className={LABEL} htmlFor={`${id}-project`}>
              {t("install.create.project")}
              <span className={HINT}>{t("install.create.optional")}</span>
            </label>
            <select
              id={`${id}-project`}
              value={draft.projectId}
              onChange={(event) => update({ projectId: event.target.value })}
              className="app-input w-full bg-background"
            >
              <option value="">{t("install.create.none")}</option>
              {projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="mt-5 flex flex-wrap items-center justify-between gap-3 border-t border-border-subtle pt-4">
          <span className="min-w-0 break-all font-mono text-[12px] text-muted">
            {form.destination}
          </span>
          <button type="submit" disabled={!form.canSubmit} className="app-button-primary">
            {form.submitting ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <FilePlus2 className="h-4 w-4" />
            )}
            {form.submitting ? t("install.create.submitting") : t("install.create.submit")}
          </button>
        </div>
      </form>
    </div>
  );
}
