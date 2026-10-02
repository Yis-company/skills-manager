import type { TFunction } from "i18next";

import { getErrorKind, getErrorMessage } from "./error";

/**
 * Map a git backup error to the plain-language copy under `settings.gitError*`.
 * Shared by the Backup page and the first-run restore dialog.
 */
export function mapGitErrorMessage(cause: unknown, t: TFunction): string {
  const kind = getErrorKind(cause);
  const message = getErrorMessage(cause, "");

  if (kind === "network") return t("settings.gitErrorNetwork");

  if (
    message.includes("Authentication failed") ||
    message.includes("Permission denied") ||
    message.includes("could not read Username")
  ) {
    return t("settings.gitErrorAuth");
  }

  if (
    message.includes("Could not resolve host") ||
    message.includes("Failed to connect") ||
    message.includes("Connection timed out") ||
    /connection\s+refused/i.test(message)
  ) {
    return t("settings.gitErrorNetwork");
  }

  if (message.includes("unrelated histories") || message.includes("refusing to merge")) {
    return t("settings.gitErrorUnrelatedHistories");
  }

  if (
    message.includes("[rejected]") ||
    message.includes("non-fast-forward") ||
    message.includes("fetch first") ||
    message.includes("failed to push some refs")
  ) {
    return t("settings.gitErrorRejected");
  }

  if (message.includes("no upstream") || message.includes("has no upstream branch")) {
    return t("settings.gitErrorNoUpstream");
  }

  if (message.includes("CONFLICT") || message.includes("conflict")) {
    return t("settings.gitErrorConflict");
  }

  if (message.includes("not a git repository")) {
    return t("settings.gitErrorNotRepo");
  }

  const detail = message.trim();

  return detail && detail !== "Error"
    ? `${t("settings.gitErrorGeneric")} (${detail})`
    : t("settings.gitErrorGeneric");
}

/** A sync stopped on a merge conflict the user has to resolve. */
export function isSyncConflictError(cause: unknown): boolean {
  const message = getErrorMessage(cause, "");

  return message.includes("SYNC_CONFLICT") || message.includes("CONFLICT");
}

/** A setup or sync failure the recovery dialog can fix (diverged or missing upstream). */
export function isRecoverableSetupError(cause: unknown): boolean {
  const message = getErrorMessage(cause, "");

  return (
    message.includes("unrelated histories") ||
    message.includes("refusing to merge") ||
    message.includes("[rejected]") ||
    message.includes("non-fast-forward") ||
    message.includes("fetch first") ||
    message.includes("failed to push some refs") ||
    message.includes("no upstream") ||
    isSyncConflictError(cause)
  );
}

/** Map a GitHub connect error to its copy under `backup.github.*`, else fall back to the git copy. */
export function mapGithubErrorMessage(cause: unknown, t: TFunction): string {
  const message = getErrorMessage(cause, "");

  if (message.includes("GITHUB_TOKEN_INVALID")) return t("backup.github.errorToken");

  if (message.includes("GITHUB_SCOPE")) return t("backup.github.errorScope");

  if (message.includes("KEYCHAIN_UNAVAILABLE")) return t("backup.github.errorKeychain");

  if (message.includes("GITHUB_DEVICE_EXPIRED")) return t("backup.github.deviceExpired");

  if (message.includes("GITHUB_DEVICE_DENIED")) return t("backup.github.deviceDenied");

  if (message.includes("GITHUB_NETWORK") || getErrorKind(cause) === "network") {
    // §3.2: when github.com is unreachable, point at the PAT fallback too.
    return `${t("settings.gitErrorNetwork")} ${t("backup.github.deviceFallbackPat")}`;
  }

  return mapGitErrorMessage(cause, t);
}

/** A raw git error saying the stored credentials were revoked or expired. */
export function isAuthFailureMessage(message: string): boolean {
  return /authentication failed|401|403|invalid.{0,24}(credentials|token)|could not read username/i.test(
    message,
  );
}
