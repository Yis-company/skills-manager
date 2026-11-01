import type { SkillSourceDiffEntry } from "./tauri";

/** Host-owned evidence is returned intact with the confirmed choice. */
export interface ChangeReview {
  kind: string;
  key: string;
  tool: null | string;
  left_label: string;
  right_label: string;
  files: SkillSourceDiffEntry[];
}
