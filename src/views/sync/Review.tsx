import { useState } from "react";
import { useTranslation } from "react-i18next";

import {
  ChangeReviewDialog,
  type ChangeReviewTarget,
  type ReviewChoice,
} from "../../components/ChangeReviewDialog";
import type * as api from "../../lib/tauri";

/** Decisions stay in review until both versions and the chosen outcome are visible. */
export function Review({
  conflicts,
  resources,
  presets,
  onResolved,
}: {
  conflicts: api.SyncConflict[];
  resources: api.ResourceConflict[];
  presets: api.SameNamePreset[];
  onResolved: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<ChangeReviewTarget | null>(null);

  const targets: ChangeReviewTarget[] = [
    ...conflicts.map((conflict): ChangeReviewTarget => ({
      kind: "skill",
      key: conflict.skill_id,
      name: conflict.name,
      description: t("sync.review.changedOnBoth", {
        one: conflict.this_host,
        other: conflict.other_host,
      }),
      choices: [
        ...[conflict.this_host, conflict.other_host].flatMap((host): ReviewChoice[] =>
          host
            ? [
                {
                  label: t("sync.review.keep", { host }),
                  command: "sync_resolve",
                  args: { skill: conflict.skill_id, keep: host, both: false },
                },
              ]
            : [],
        ),
        {
          label: t("sync.review.keepBoth"),
          command: "sync_resolve",
          args: { skill: conflict.skill_id, keep: null, both: true },
        },
      ],
    })),
    ...resources.map((resource): ChangeReviewTarget => ({
      kind: "resource",
      key: resource.key,
      name: resource.name,
      description: t(`sync.review.${resource.kind}`),
      choices: [resource.this_host, resource.other_host].map((host, index) => ({
        label: t("sync.review.keep", { host }),
        command: "sync_resolve_resource",
        args: { key: resource.key, other: index === 1 },
      })),
    })),
    ...presets.map((preset): ChangeReviewTarget => ({
      kind: "preset",
      key: preset.preset_id,
      name: preset.name,
      description: t("sync.review.sameName", { other: preset.other_name }),
      choices: [
        {
          label: t("sync.review.keepBoth"),
          command: "sync_resolve_preset",
          args: { preset: preset.preset_id },
        },
        {
          label: t("sync.review.combine", { other: preset.other_name }),
          command: "sync_resolve_preset",
          args: { preset: preset.preset_id, combine: true },
        },
        ...[
          { id: preset.other_id, name: preset.other_name },
          { id: preset.preset_id, name: preset.name },
        ].map((item) => ({
          label: t("sync.review.keepOnly", { name: item.name }),
          command: "sync_resolve_preset",
          args: { preset: preset.preset_id, keep: item.id },
        })),
      ],
    })),
  ];

  if (targets.length === 0)
    return <div className="py-16 text-center text-[13px] text-muted">{t("sync.review.empty")}</div>;

  return (
    <div className="space-y-3">
      <p className="text-[13px] text-muted">{t("sync.review.desc")}</p>
      {targets.map((item) => (
        <section
          key={`${item.kind}:${item.key}`}
          className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border-subtle p-4"
        >
          <div className="min-w-0">
            <h2 className="break-words text-[14px] font-medium">{item.name}</h2>
            <p className="text-[12px] text-muted">{item.description}</p>
          </div>
          <button
            type="button"
            className="app-button-secondary"
            aria-label={t("review.compareNamed", { name: item.name })}
            onClick={() => setTarget(item)}
          >
            {t("review.compare")}
          </button>
        </section>
      ))}
      {target && (
        <ChangeReviewDialog
          hostId={null}
          target={target}
          onClose={() => setTarget(null)}
          onApplied={onResolved}
        />
      )}
    </div>
  );
}
