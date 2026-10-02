import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { getErrorMessage } from "../lib/error";
import * as api from "../lib/tauri";
import type { ScanResult } from "../lib/tauri";

/**
 * Skills found in other agents' folders. The first scan runs once `active`;
 * later ones on demand, loudly (`runScan`) or silently after an install
 * (`runScanSilent`).
 */
export function useLocalScan(active: boolean) {
  const { t } = useTranslation();
  const [scanResult, setScanResult] = useState<null | ScanResult>(null);
  const [scanLoading, setScanLoading] = useState(false);
  const [localError, setLocalError] = useState<null | string>(null);
  const [firstScanSettled, setFirstScanSettled] = useState(false);

  // Updates state only once the scan settles; callers own `scanLoading`.
  const loadScan = useCallback(
    () =>
      api.scanLocalSkills().then(setScanResult, (cause: unknown) => {
        console.error(cause);
        const message = getErrorMessage(cause, t("common.error"));
        setLocalError(message);
        toast.error(message);
      }),
    [t],
  );

  const runScan = useCallback(async () => {
    setScanLoading(true);
    setLocalError(null);
    await loadScan();
    setScanLoading(false);
  }, [loadScan]);

  // Silent variant used after install/import. Never surfaces a toast or
  // new error state — failure here must not mask the install success.
  // Clears any stale localError on success so successful operations don't
  // leave previous error banners behind.
  const runScanSilent = useCallback(async () => {
    try {
      const result = await api.scanLocalSkills();
      setScanResult(result);
      setLocalError(null);
    } catch (error: unknown) {
      console.warn("silent scan failed:", error);
    }
  }, []);

  const firstScanPending = active && !scanResult && !firstScanSettled;

  useEffect(() => {
    if (!firstScanPending) return;
    void loadScan().then(() => setFirstScanSettled(true));
  }, [firstScanPending, loadScan]);

  return {
    scanResult,
    scanLoading: scanLoading || firstScanPending,
    localError,
    setLocalError,
    runScan,
    runScanSilent,
  };
}
