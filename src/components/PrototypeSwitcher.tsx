// PROTOTYPE: throwaway variant switcher for UI prototypes. Dev builds only.
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect } from "react";

interface PrototypeSwitcherProps {
  variants: { key: string; name: string }[];
  current: string;
  onChange: (key: string) => void;
}

export function PrototypeSwitcher({ variants, current, onChange }: PrototypeSwitcherProps) {
  const index = Math.max(
    0,
    variants.findIndex((v) => v.key === current),
  );
  const step = (delta: number) =>
    onChange(variants[(index + delta + variants.length) % variants.length].key);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;

      if (el?.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "ArrowLeft") step(-1);
      if (e.key === "ArrowRight") step(1);
    };

    window.addEventListener("keydown", onKey);

    return () => window.removeEventListener("keydown", onKey);
  });

  if (!import.meta.env.DEV) return null;

  const v = variants[index];

  return (
    <div className="fixed bottom-5 left-1/2 z-[100] flex -translate-x-1/2 items-center gap-1 rounded-full bg-black px-1.5 py-1.5 text-white shadow-2xl ring-1 ring-white/20">
      <button
        type="button"
        aria-label="Previous variant"
        onClick={() => step(-1)}
        className="rounded-full p-1.5 hover:bg-white/15"
      >
        <ChevronLeft className="h-4 w-4" />
      </button>
      <span className="min-w-[220px] px-2 text-center text-[12px] font-medium">
        PROTOTYPE · {v.key} ({v.name}) · {index + 1}/{variants.length}
      </span>
      <button
        type="button"
        aria-label="Next variant"
        onClick={() => step(1)}
        className="rounded-full p-1.5 hover:bg-white/15"
      >
        <ChevronRight className="h-4 w-4" />
      </button>
    </div>
  );
}
