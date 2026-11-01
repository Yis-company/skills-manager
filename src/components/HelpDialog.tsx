import {
  BookOpen,
  FolderTree,
  Globe,
  Layers3,
  Map,
  RefreshCw,
  Settings2,
  Sparkles,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { useApp } from "../context/AppContext";
import { Modal } from "./Modal";

const GUIDE_ICONS = [Map, Layers3, BookOpen, Sparkles, Globe, FolderTree, RefreshCw, Settings2];

export function HelpDialog() {
  const { t } = useTranslation();
  const { helpOpen, closeHelp } = useApp();

  if (!helpOpen) return null;

  return (
    <Modal open={helpOpen} title={t("help.title")} wide onClose={closeHelp}>
      <p className="mb-5 text-[13px] text-muted">{t("help.description")}</p>
      <div className="space-y-3">
        {(
          [
            "workflows",
            "presets",
            "install",
            "sync",
            "global",
            "projects",
            "backup",
            "settings",
          ] as const
        ).map((key, index) => {
          const Icon = GUIDE_ICONS[index];

          return (
            <div
              key={key}
              className="flex items-start gap-3 rounded-2xl border border-border-subtle bg-surface px-4 py-3"
            >
              <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-2xl bg-background text-accent">
                <Icon className="h-4 w-4" />
              </div>
              <div>
                <h3 className="text-[13px] font-semibold text-secondary">
                  {t(`help.items.${key}.title`)}
                </h3>
                <p className="mt-1 text-[13px] leading-5 text-muted">
                  {t(`help.items.${key}.description`)}
                </p>
              </div>
            </div>
          );
        })}
      </div>
    </Modal>
  );
}
