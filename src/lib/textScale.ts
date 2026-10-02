export const TEXT_SIZE_SCALE_MAP = new Map(
  Object.entries({
    small: "0.9",
    default: "1",
    large: "1.1",
    xlarge: "1.2",
  }),
);

export function applyTextSize(size: string) {
  const scale = TEXT_SIZE_SCALE_MAP.get(size) ?? "1";
  document.documentElement.style.zoom = scale;
  document.documentElement.style.setProperty("--app-scale", scale);
}
