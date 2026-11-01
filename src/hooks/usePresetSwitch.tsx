import { useState } from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "../components/ConfirmDialog";
import { useApp } from "../context/AppContext";
import { getActiveHostId, invokeHost } from "../lib/hostCall";
import * as api from "../lib/tauri";

type Impact = { name: string; agent: string; path: string };

interface SwitchReview {
  left_label: null | string;
  right_label: string;
  additions: Impact[];
  removals: Impact[];
  preserved: Impact[];
}

export function usePresetSwitch(onSwitched: () => Promise<void>) {
  const { t } = useTranslation();
  const { activeHostId, activeHost, remoteHosts } = useApp();

  const [pending, setPending] = useState<{
    preset: api.Preset;
    review: SwitchReview;
    hostId: null | string;
  } | null>(null);

  const apply = async (preset: api.Preset, review: SwitchReview, hostId: null | string) => {
    try {
      await invokeHost<void>(hostId, "apply_preset_to_default", {
        id: preset.id,
        review: JSON.stringify(review),
      });
    } finally {
      if (getActiveHostId() === hostId) await onSwitched();
    }
  };

  const switchTo = async (preset: api.Preset) => {
    const hostId = activeHostId;

    const review = await api.syncCall<SwitchReview>(hostId, "sync_review", {
      kind: "switch",
      key: preset.id,
    });

    if (getActiveHostId() !== hostId) return;

    setPending({ preset, review, hostId });
  };

  const dialog = (
    <ConfirmDialog
      open={pending !== null}
      title={t("presetActions.firstSwitchTitle", { name: pending?.preset.name ?? "" })}
      message={t("presetActions.firstSwitchMessage")}
      tone="warning"
      lockWhilePending
      confirmLabel={t("presetActions.switch")}
      onClose={() => setPending(null)}
      onConfirm={async () => {
        if (pending) await apply(pending.preset, pending.review, pending.hostId);
      }}
    >
      {pending && (
        <div className="space-y-3 text-[13px] text-secondary">
          <p>{t("presetActions.host", { name: activeHost?.name ?? t("sync.thisHost") })}</p>
          <p>
            {t("presetActions.current", { name: pending.review.left_label ?? t("common.none") })}
          </p>
          {(["additions", "removals", "preserved"] as const).map((kind) => (
            <section key={kind}>
              <h3 className="font-medium">
                {t(`presetActions.${kind}`)} ({pending.review[kind].length})
              </h3>
              <ul className="mt-1 max-h-32 space-y-1 overflow-auto">
                {pending.review[kind].map((item) => (
                  <li key={`${item.agent}:${item.path}`} className="break-words">
                    {item.name} · {item.agent}
                    <span className="block font-mono text-[11px] text-muted">{item.path}</span>
                  </li>
                ))}
              </ul>
            </section>
          ))}
          <p>
            {remoteHosts.length
              ? t("presetActions.hosts", { names: remoteHosts.map((host) => host.name).join(", ") })
              : t("presetActions.noHosts")}
          </p>
        </div>
      )}
    </ConfirmDialog>
  );

  return { switchTo, dialog };
}
