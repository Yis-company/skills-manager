import { X } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { getPresetIconOption, PRESET_ICON_OPTIONS } from "../lib/presetIcons";
import { cn } from "../utils";

interface Props {
  open: boolean;
  currentName: string;
  currentIcon?: null | string;
  currentDescription?: null | string;
  onClose: () => void;
  onRename: (newName: string, icon?: string, description?: string) => Promise<void>;
}

export function RenamePresetDialog({ open, ...props }: Props) {
  if (!open) return null;

  return <RenamePresetDialogContent {...props} />;
}

function RenamePresetDialogContent({
  currentName,
  currentIcon,
  currentDescription,
  onClose,
  onRename,
}: Omit<Props, "open">) {
  const { t } = useTranslation();

  // The icon the sidebar actually shows — inferred from the name when none is stored.
  const shownIcon = getPresetIconOption({
    name: currentName,
    description: currentDescription ?? null,
    icon: currentIcon ?? null,
  }).key;

  const [name, setName] = useState(currentName);
  const [icon, setIcon] = useState(shownIcon);
  const [description, setDescription] = useState(currentDescription || "");
  const [loading, setLoading] = useState(false);

  const unchanged =
    name.trim() === currentName &&
    icon === shownIcon &&
    description.trim() === (currentDescription || "").trim();

  const handleRename = async () => {
    if (!name.trim() || unchanged) {
      return;
    }

    setLoading(true);

    try {
      // Only send an icon the user picked: saving a name or description must
      // not pin the inferred icon of a preset that has none stored.
      await onRename(
        name.trim(),
        icon !== shownIcon ? icon : currentIcon || undefined,
        description.trim() || undefined,
      );
      onClose();
    } finally {
      setLoading(false);
    }
  };

  const inputClass =
    "w-full bg-background border border-border-subtle rounded-lg px-3 py-2 text-[13px] text-secondary focus:outline-none focus:border-border transition-all placeholder-faint";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={onClose} />
      <div className="relative w-full max-w-[400px] rounded-xl border border-border bg-surface p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-[13px] font-semibold text-primary">{t("common.edit")}</h2>
          <button
            onClick={onClose}
            className="rounded p-1 text-muted outline-none transition-colors hover:text-secondary"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-3">
          <div>
            <label className="mb-1 block text-[13px] font-medium text-tertiary">
              {t("preset.name")}
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("preset.namePlaceholder")}
              className={inputClass}
              autoFocus
              onKeyDown={(e) => e.key === "Enter" && handleRename()}
            />
          </div>
          <div>
            <label className="mb-1 block text-[13px] font-medium text-tertiary">
              {t("preset.description")}
            </label>
            <input
              type="text"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder={t("preset.descPlaceholder")}
              className={inputClass}
              onKeyDown={(e) => e.key === "Enter" && handleRename()}
            />
          </div>
          <div>
            <label className="mb-1.5 block text-[13px] font-medium text-tertiary">
              {t("preset.icon")}
            </label>
            <div className="grid max-h-[220px] grid-cols-[repeat(auto-fill,minmax(36px,1fr))] gap-1.5 overflow-y-auto pr-1">
              {PRESET_ICON_OPTIONS.map((option) => {
                const Icon = option.icon;
                const selected = option.key === icon;

                return (
                  <button
                    key={option.key}
                    type="button"
                    onClick={() => setIcon(option.key)}
                    className={cn(
                      "flex h-9 items-center justify-center rounded-lg border bg-background transition-all outline-none",
                      selected
                        ? `${option.activeClass} ${option.colorClass}`
                        : "border-border-subtle text-muted hover:border-border hover:text-secondary",
                    )}
                    title={option.label}
                  >
                    <Icon className="h-3.5 w-3.5" />
                  </button>
                );
              })}
            </div>
          </div>
          <div className="flex justify-end gap-2 pt-1">
            <button
              onClick={onClose}
              className="rounded-lg px-3 py-1.5 text-[13px] font-medium text-tertiary outline-none transition-colors hover:bg-surface-hover hover:text-secondary"
            >
              {t("common.cancel")}
            </button>
            <button
              onClick={handleRename}
              disabled={!name.trim() || unchanged || loading}
              className="rounded-lg border border-accent-border bg-accent-dark px-3 py-1.5 text-[13px] font-medium text-white outline-none transition-colors hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
            >
              {loading ? t("common.loading") : t("common.save")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
