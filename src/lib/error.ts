import { isString } from "../utils";

/** Error kinds matching the Rust `AppError` enum. */
export const ERROR_KINDS = [
  "database",
  "io",
  "network",
  "git",
  "not_found",
  "invalid_input",
  "cancelled",
  "internal",
  "target_conflict",
  "remote_version_mismatch",
] as const;

export type ErrorKind = (typeof ERROR_KINDS)[number];

/** A deployment target that is not ours to replace (#363). Nothing at these
 *  paths was touched. */
export interface TargetConflictDetail {
  path: string;
  reason: string;
}

export interface TargetConflictDetails {
  conflicts: TargetConflictDetail[];
}

/** Structured error returned by Tauri commands. */
export interface AppError {
  kind: ErrorKind;
  message: string;
  /** Present only for kinds that carry machine-readable specifics. */
  details?: TargetConflictDetails;
}

/** Type-guard: check if a caught error is a structured `AppError`. */
export function isAppError(cause: unknown): cause is AppError {
  return (
    cause instanceof Object &&
    "kind" in cause &&
    "message" in cause &&
    ERROR_KINDS.some((kind) => kind === cause.kind) &&
    typeof cause.message === "string"
  );
}

/**
 * Extract a human-readable message from any caught error.
 * Handles structured `AppError`, plain strings, and `Error` instances.
 */
export function getErrorMessage(cause: unknown, fallback: string): string {
  if (isAppError(cause)) return cause.message;

  if (cause instanceof Error && cause.message) return cause.message;

  // A rejected Tauri command can carry a bare string.
  if (isString(cause) && cause) return cause;

  return fallback;
}

/** Extract the error kind (or `undefined` for non-structured errors). */
export function getErrorKind(cause: unknown): ErrorKind | undefined {
  if (isAppError(cause)) return cause.kind;

  return undefined;
}
