import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../context/AppContext";
import { useWorkspaceChanges } from "../context/WorkspaceChangesContext";
import { getErrorMessage } from "../lib/error";
import { computePresetStatus, type PresetStatusMode } from "../lib/presetStatus";
import type { ManagedSkill, Preset } from "../lib/tauri";
import { Modal } from "./Modal";

export interface PresetBarProps {
  presets: Preset[];
  managedSkills: ManagedSkill[];
  agentKeys: string[];
  existsInWorkspace: (skill: ManagedSkill, agentKey: string) => boolean;
  onAddSkill: (skill: ManagedSkill, agentKey: string) => Promise<void>;
  onRemoveSkill: (skill: ManagedSkill, agentKey: string) => Promise<void>;
  onComplete: () => Promise<void>;
  confirmRemove?: (pairs: { skill: ManagedSkill; agentKey: string }[]) => Promise<boolean>;
  /** Whether the status badge counts agent copies or logical skills. */
  statusMode?: PresetStatusMode;
  /** Global workspace: a click switches the shared active preset (SID-98)
   *  instead of adding its skills. */
  onSwitch?: (preset: Preset) => Promise<void>;
  activePresetId?: null | string;
  projectName?: string;
}

export function PresetBar({
  presets,
  managedSkills,
  agentKeys,
  existsInWorkspace,
  onAddSkill,
  onRemoveSkill,
  onComplete,
  confirmRemove,
  statusMode = "agent-pair",
  onSwitch,
  activePresetId,
  projectName,
}: PresetBarProps) {
  const { t } = useTranslation();
  const workspaceChanges = useWorkspaceChanges();
  const { activeHost } = useApp();
  const [choosing, setChoosing] = useState(false);
  const [chosen, setChosen] = useState("");

  const [projectReview, setProjectReview] = useState<{ preset: Preset; remove: boolean } | null>(
    null,
  );

  const [loadingKey, setLoadingKey] = useState<null | string>(null);

  const statuses = useMemo(() => {
    const map = new Map<string, ReturnType<typeof computePresetStatus>>();

    for (const preset of presets) {
      map.set(
        preset.id,
        computePresetStatus(preset, managedSkills, agentKeys, existsInWorkspace, statusMode),
      );
    }

    return map;
  }, [presets, managedSkills, agentKeys, existsInWorkspace, statusMode]);

  const visiblePresets = useMemo(
    () => presets.filter((p) => statuses.get(p.id)?.status !== "empty"),
    [presets, statuses],
  );

  const handleActivate = useCallback(
    async (preset: Preset) => {
      setLoadingKey(`${preset.id}-add`);

      try {
        const presetSkills = managedSkills.filter((s) => s.preset_ids.includes(preset.id));

        let added = 0,
          skipped = 0,
          failed = 0;

        const failures: string[] = [];

        for (const skill of presetSkills) {
          for (const agentKey of agentKeys) {
            if (existsInWorkspace(skill, agentKey)) {
              skipped++;
              continue;
            }

            try {
              await onAddSkill(skill, agentKey);
              added++;
            } catch (e) {
              failed++;
              failures.push(getErrorMessage(e, t("common.error")));
            }
          }
        }

        if (added > 0) {
          toast.success(t("presetActions.addedToast", { added, skipped }));
        } else if (failed === 0) {
          toast.info(t("presetActions.nothingToAdd"));
        }

        if (failed > 0) {
          toast.error(
            [t("presetActions.partialFailedToast", { count: failed }), failures[0]]
              .filter(Boolean)
              .join(" — "),
          );
        }

        await onComplete();
      } catch (error) {
        toast.error(getErrorMessage(error, t("common.error")));
      } finally {
        setLoadingKey(null);
      }
    },
    [agentKeys, existsInWorkspace, managedSkills, onAddSkill, onComplete, t],
  );

  const handleDeactivate = useCallback(
    async (preset: Preset) => {
      setLoadingKey(`${preset.id}-remove`);

      try {
        const presetSkills = managedSkills.filter((s) => s.preset_ids.includes(preset.id));

        const pairs = presetSkills.flatMap((skill) =>
          agentKeys
            .filter((key) => existsInWorkspace(skill, key))
            .map((agentKey) => ({ skill, agentKey })),
        );

        if (confirmRemove && !(await confirmRemove(pairs))) return;

        let removed = 0,
          failed = 0;

        const failures: string[] = [];

        for (const skill of presetSkills) {
          for (const agentKey of agentKeys) {
            if (!existsInWorkspace(skill, agentKey)) continue;

            try {
              await onRemoveSkill(skill, agentKey);
              removed++;
            } catch (e) {
              failed++;
              failures.push(getErrorMessage(e, t("common.error")));
            }
          }
        }

        if (removed > 0) {
          toast.success(t("presetActions.removedToast", { removed }));
        } else if (failed === 0) {
          toast.info(t("presetActions.nothingToRemove"));
        }

        if (failed > 0) {
          toast.error(
            [t("presetActions.partialFailedToast", { count: failed }), failures[0]]
              .filter(Boolean)
              .join(" — "),
          );
        }

        await onComplete();
      } catch (error) {
        toast.error(getErrorMessage(error, t("common.error")));
      } finally {
        setLoadingKey(null);
      }
    },
    [agentKeys, existsInWorkspace, managedSkills, onComplete, onRemoveSkill, confirmRemove, t],
  );

  if (visiblePresets.length === 0) return null;
  const busy = loadingKey !== null;
  const current = presets.find((preset) => preset.id === activePresetId);
  const selected = visiblePresets.find((preset) => preset.id === chosen) ?? visiblePresets[0];

  const affected = projectReview
    ? managedSkills
        .filter((skill) => skill.preset_ids.includes(projectReview.preset.id))
        .flatMap((skill) =>
          agentKeys
            .filter((key) =>
              projectReview.remove ? existsInWorkspace(skill, key) : !existsInWorkspace(skill, key),
            )
            .map((key) => ({ skill, key })),
        )
    : [];

  return (
    <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg bg-bg-secondary p-3">
      <div className="text-[13px]">
        <p className="font-medium">
          {onSwitch
            ? t("workspace.sharedActive", { name: current?.name ?? t("common.none") })
            : t("workspace.projectDestination", { name: projectName })}
        </p>
        <p className="mt-1 text-[12px] text-muted">
          {t(onSwitch ? "presetActions.firstSwitchMessage" : "workspace.projectPresetScope")}
        </p>
      </div>
      <button
        className="app-button-secondary"
        disabled={busy}
        onClick={async () => {
          if (await workspaceChanges.confirmLeave()) {
            setChosen(current?.id ?? visiblePresets[0].id);
            setChoosing(true);
          }
        }}
      >
        {t(onSwitch ? "workspace.switchShared" : "workspace.addPresetProject")}
      </button>
      <Modal
        open={choosing}
        title={t(onSwitch ? "workspace.switchShared" : "workspace.addPresetProject")}
        onClose={() => setChoosing(false)}
      >
        <label className="block text-[13px]">
          {t("workspace.choosePreset")}
          <select
            aria-label={t("workspace.choosePreset")}
            className="app-input mt-2 w-full"
            value={selected.id}
            onChange={(event) => setChosen(event.target.value)}
          >
            {visiblePresets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.name}
              </option>
            ))}
          </select>
        </label>
        <p className="my-4 text-[13px] text-muted">{selected.description}</p>
        <div className="flex flex-wrap justify-end gap-2">
          <button className="app-button-secondary" onClick={() => setChoosing(false)}>
            {t("common.cancel")}
          </button>
          {!onSwitch && (
            <button
              className="app-button-secondary text-danger"
              onClick={() => {
                setChoosing(false);
                setProjectReview({ preset: selected, remove: true });
              }}
            >
              {t("presetActions.removeProjectNamed", { name: selected.name })}
            </button>
          )}
          <button
            className="app-button-primary"
            disabled={Boolean(onSwitch && selected.id === activePresetId)}
            onClick={() => {
              setChoosing(false);

              if (onSwitch) {
                setLoadingKey(selected.id);
                void onSwitch(selected)
                  .catch((error) => toast.error(getErrorMessage(error, t("common.error"))))
                  .finally(() => setLoadingKey(null));
              } else setProjectReview({ preset: selected, remove: false });
            }}
          >
            {t("workspace.reviewPreset")}
          </button>
        </div>
      </Modal>
      <Modal
        open={projectReview !== null}
        title={t("workspace.projectPresetReview")}
        locked={busy}
        onClose={() => setProjectReview(null)}
      >
        <p className="mb-3 text-[13px]">
          {activeHost?.name ?? t("hostSwitcher.local")} · {projectName} ·{" "}
          {projectReview?.preset.name}
        </p>
        <p className="text-[13px] text-muted">{t("workspace.projectPresetScope")}</p>
        <ul className="my-4 max-h-60 overflow-auto text-[13px]">
          {affected.map(({ skill, key }) => (
            <li className="py-1" key={`${skill.id}:${key}`}>
              {skill.name} · {key} ·{" "}
              {t(projectReview?.remove ? "workspace.removeFromAgent" : "workspace.addToAgent")}
            </li>
          ))}
        </ul>
        <div className="flex justify-end gap-2">
          <button
            className="app-button-secondary"
            disabled={busy}
            onClick={() => setProjectReview(null)}
          >
            {t("common.cancel")}
          </button>
          <button
            className="app-button-primary"
            disabled={busy || !affected.length}
            onClick={async () => {
              if (!projectReview) return;

              if (projectReview.remove) await handleDeactivate(projectReview.preset);
              else await handleActivate(projectReview.preset);
              setProjectReview(null);
            }}
          >
            {busy ? t("common.loading") : t("workspace.applyReviewed")}
          </button>
        </div>
      </Modal>
    </div>
  );
}
