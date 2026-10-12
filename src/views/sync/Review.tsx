import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { getErrorMessage } from "../../lib/error";
import * as api from "../../lib/tauri";

/**
 * Skills, instructions and MCPs changed on two hosts (SID-108) and presets
 * with the same name from two hosts (SID-104). Choices are recorded in Sync,
 * and each host applies them on its next pull.
 */
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
  const [busy, setBusy] = useState<null | string>(null);

  if (conflicts.length === 0 && resources.length === 0 && presets.length === 0) {
    return <div className="py-16 text-center text-[13px] text-muted">{t("sync.review.empty")}</div>;
  }

  const resolve = async (key: string, command: string, args: api.SyncCallArgs) => {
    setBusy(key);

    try {
      await api.syncCall(null, command, args);
      toast.success(t("backup.conflicts.resolved"));
      await onResolved();
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    } finally {
      setBusy(null);
    }
  };

  const choice = (key: string, label: string, onClick: () => void) => (
    <button
      key={key}
      type="button"
      disabled={busy !== null}
      className="app-button-secondary !py-1.5"
      onClick={onClick}
    >
      {label}
    </button>
  );

  return (
    <div className="space-y-3">
      <p className="text-[12px] text-muted">{t("sync.review.desc")}</p>
      {conflicts.map((conflict) => {
        const keep = (host: null | string) =>
          resolve(conflict.skill_id, "sync_resolve", {
            skill: conflict.skill_id,
            keep: host,
            both: host === null,
          });

        return (
          <ReviewCard
            key={conflict.skill_id}
            kind={t("sync.review.skill")}
            name={conflict.name}
            detail={t("sync.review.changedOnBoth", {
              one: conflict.this_host,
              other: conflict.other_host || t("sync.review.anotherHost"),
            })}
          >
            {[conflict.this_host, conflict.other_host].flatMap((host) =>
              host ? [choice(host, t("sync.review.keep", { host }), () => keep(host))] : [],
            )}
            {choice("both", t("sync.review.keepBoth"), () => keep(null))}
          </ReviewCard>
        );
      })}
      {resources.map((resource) => (
        <ReviewCard
          key={resource.key}
          kind={t(`sync.review.${resource.kind}`)}
          name={resource.name}
          detail={t("sync.review.changedOnBoth", {
            one: resource.this_host,
            other: resource.other_host || t("sync.review.anotherHost"),
          })}
        >
          {[resource.this_host, resource.other_host].map((host, index) =>
            choice(host || String(index), t("sync.review.keep", { host }), () =>
              resolve(resource.key, "sync_resolve_resource", {
                key: resource.key,
                other: index === 1,
              }),
            ),
          )}
        </ReviewCard>
      ))}
      {presets.map((preset) => {
        const settle = (args: api.SyncCallArgs) =>
          resolve(preset.preset_id, "sync_resolve_preset", { preset: preset.preset_id, ...args });

        return (
          <ReviewCard
            key={preset.preset_id}
            kind={t("sync.review.preset")}
            name={preset.name}
            detail={t("sync.review.sameName", { other: preset.other_name })}
          >
            {choice("both", t("sync.review.keepBoth"), () => settle({}))}
            {choice("combine", t("sync.review.combine", { other: preset.other_name }), () =>
              settle({ combine: true }),
            )}
            {choice("other", t("sync.review.keepOnly", { name: preset.other_name }), () =>
              settle({ keep: preset.other_id }),
            )}
            {choice("this", t("sync.review.keepOnly", { name: preset.name }), () =>
              settle({ keep: preset.preset_id }),
            )}
          </ReviewCard>
        );
      })}
    </div>
  );
}

function ReviewCard({
  kind,
  name,
  detail,
  children,
}: {
  kind: string;
  name: string;
  detail: string;
  children: ReactNode;
}) {
  return (
    <div className="rounded-lg border border-border-subtle p-4">
      <div className="text-[11px] font-semibold uppercase tracking-wide text-amber-600 dark:text-amber-400">
        {kind}
      </div>
      <div className="mt-0.5 text-[14px] font-medium text-primary">{name}</div>
      <div className="text-[12px] text-muted">{detail}</div>
      <div className="mt-3 flex flex-wrap gap-2">{children}</div>
    </div>
  );
}
