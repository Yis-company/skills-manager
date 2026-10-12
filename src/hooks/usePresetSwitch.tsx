import { useState } from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "../components/ConfirmDialog";
import * as api from "../lib/tauri";

const PREVIEWED_SETTING = "preset_switch_previewed";

/**
 * Switch the shared active preset (SID-98). The first switch shows what it
 * takes off the agents, skills added directly included; later ones just
 * switch.
 */
export function usePresetSwitch(onSwitched: () => Promise<void>) {
  const { t } = useTranslation();
  const [pending, setPending] = useState<{ preset: api.Preset; removals: string[] } | null>(null);

  const apply = async (preset: api.Preset) => {
    await api.applyPresetToDefault(preset.id);
    await onSwitched();
  };

  const switchTo = async (preset: api.Preset) => {
    const previewed = (await api.getSettings(PREVIEWED_SETTING).catch(() => null)) === "1";

    if (!previewed) {
      const removals = await api.previewPresetSwitch(preset.id);

      if (removals.length > 0) {
        setPending({
          preset,
          removals: removals.map(([skill, agent]) => `${skill} · ${agent}`),
        });

        return;
      }
    }

    await apply(preset);
  };

  const dialog = (
    <ConfirmDialog
      open={pending !== null}
      title={t("presetActions.firstSwitchTitle", { name: pending?.preset.name ?? "" })}
      message={t("presetActions.firstSwitchMessage")}
      details={pending?.removals}
      tone="warning"
      confirmLabel={t("presetActions.switch")}
      onClose={() => setPending(null)}
      onConfirm={async () => {
        if (!pending) return;
        await api.setSettings(PREVIEWED_SETTING, "1");
        await apply(pending.preset);
      }}
    />
  );

  return { switchTo, dialog };
}
