import { openUrl } from "@tauri-apps/plugin-opener";
import { Check, Copy, ExternalLink, Loader2, Terminal, X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { STEPS, type StepStatus, type useAddHost } from "../../hooks/useAddHost";
import type * as api from "../../lib/tauri";

export function AddHost({ add }: { add: ReturnType<typeof useAddHost> }) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const run = add.run;

  if (!run) {
    return (
      <form
        className="max-w-[420px] space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          void add.start(name.trim(), target.trim());
        }}
      >
        <h2 className="text-[18px] font-semibold text-primary">{t("sync.add.title")}</h2>
        <p className="text-[13px] text-muted">{t("sync.add.desc")}</p>
        <input
          className="app-input w-full"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("sync.add.name")}
          aria-label={t("sync.add.name")}
        />
        <input
          className="app-input w-full font-mono"
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          placeholder="user@host"
          aria-label={t("sync.add.target")}
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
        />
        <button
          type="submit"
          className="app-button-primary w-full"
          disabled={!name.trim() || !target.trim()}
        >
          {t("sync.add.start")}
        </button>
      </form>
    );
  }

  const done = STEPS.filter((key) => run.steps[key].status === "done").length;
  const blocked = STEPS.some((key) => run.steps[key].status === "blocked");

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <Terminal className="h-4 w-4 text-muted" />
        <h2 className="text-[16px] font-semibold text-primary">
          {t("sync.add.settingUp", { name: run.name })}
        </h2>
        <span className="font-mono text-[12px] text-faint">{run.target}</span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-surface-active">
        <div
          className="h-full bg-accent transition-all"
          style={{ width: `${(done / STEPS.length) * 100}%` }}
        />
      </div>
      <ol
        role="log"
        className="space-y-2 rounded-lg bg-zinc-950 p-4 font-mono text-[12px] leading-5 text-zinc-300"
      >
        {STEPS.flatMap((key) => {
          const current = run.steps[key];

          if (current.status === "todo") return [];

          return (
            <li key={key}>
              <div className="flex items-start gap-2">
                <StepIcon status={current.status} />
                <span>
                  <span className="text-zinc-100">{t(`sync.add.steps.${key}`)}</span>
                  {current.note && <span className="block text-zinc-400">{current.note}</span>}
                </span>
              </div>
              {key === "access" && current.status === "waiting" && run.paste && (
                <PasteKey paste={run.paste} onAdded={add.pasted} />
              )}
              {key === "preview" && current.status === "waiting" && run.preview && (
                <div className="ml-6 mt-2 rounded-md bg-surface p-3 font-sans">
                  <JoinPreviewList preview={run.preview} />
                  <div className="mt-3 flex gap-2">
                    <button type="button" className="app-button-primary !py-1.5" onClick={add.join}>
                      {t("sync.add.join")}
                    </button>
                    <button
                      type="button"
                      className="app-button-secondary !py-1.5"
                      onClick={add.reset}
                    >
                      {t("common.cancel")}
                    </button>
                  </div>
                </div>
              )}
            </li>
          );
        })}
      </ol>
      {(blocked || run.done) && (
        <button type="button" className="app-button-secondary" onClick={add.reset}>
          {run.done ? t("sync.add.another") : t("sync.add.startOver")}
        </button>
      )}
    </div>
  );
}

function StepIcon({ status }: { status: StepStatus }) {
  if (status === "running")
    return <Loader2 className="mt-0.5 h-3.5 w-3.5 animate-spin text-accent" />;

  if (status === "blocked") return <X className="mt-0.5 h-3.5 w-3.5 text-red-400" />;

  if (status === "waiting") return <span className="mt-1 h-2.5 w-2.5 rounded-full bg-amber-400" />;

  return <Check className="mt-0.5 h-3.5 w-3.5 text-emerald-400" />;
}

function PasteKey({ paste, onAdded }: { paste: api.HostSyncPasteKey; onAdded: () => void }) {
  const { t } = useTranslation();

  return (
    <div className="ml-6 mt-2 space-y-2 rounded-md bg-surface p-3 font-sans text-[13px] text-secondary">
      <p>{t("sync.add.pasteKey", { title: paste.title })}</p>
      <pre className="overflow-x-auto rounded bg-bg-secondary p-2 font-mono text-[11px] text-tertiary">
        {paste.key}
      </pre>
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          className="app-button-secondary !py-1.5"
          onClick={() => void navigator.clipboard.writeText(paste.key)}
        >
          <Copy className="h-3.5 w-3.5" />
          {t("sync.add.copyKey")}
        </button>
        <button
          type="button"
          className="app-button-secondary !py-1.5"
          onClick={() => void openUrl(paste.settings_url)}
        >
          <ExternalLink className="h-3.5 w-3.5" />
          {t("sync.add.openDeployKeys")}
        </button>
        <button type="button" className="app-button-primary !py-1.5" onClick={onAdded}>
          {t("sync.add.added")}
        </button>
      </div>
    </div>
  );
}

/** What joining changes (SID-100): adds, replaced by Sync's version, the active preset. */
export function JoinPreviewList({ preview }: { preview: api.JoinPreview }) {
  const { t } = useTranslation();

  const items = (list: api.JoinItem[]) =>
    list.map((item) => `${t(`sync.kinds.${item.kind}`)} ${item.name}`).join(", ");

  return (
    <div className="space-y-1.5 text-[13px] text-secondary">
      {preview.remote_empty && <p>{t("sync.preview.remoteEmpty")}</p>}
      {preview.local_empty && <p>{t("sync.preview.localEmpty")}</p>}
      {preview.adds.length > 0 && <p>{t("sync.preview.adds", { items: items(preview.adds) })}</p>}
      {preview.replaced.length > 0 && (
        <p>{t("sync.preview.replaced", { items: items(preview.replaced) })}</p>
      )}
      {preview.active_preset && (
        <p>{t("sync.preview.activePreset", { preset: preview.active_preset })}</p>
      )}
      <p className="text-[12px] text-muted">{t("sync.preview.undo")}</p>
    </div>
  );
}
