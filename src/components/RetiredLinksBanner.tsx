import { useEffect, useState } from "react";
import { Link2Off, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import * as api from "../lib/tauri";
import { getErrorMessage } from "../lib/error";

interface RetiredLinksBannerProps {
  projectId: string;
  /** Changes whenever the project's skills reload, so the check reruns. */
  refreshKey: unknown;
}

/** Offers to remove links the app left in folders agents no longer read. */
export function RetiredLinksBanner({ projectId, refreshKey }: RetiredLinksBannerProps) {
  const { t } = useTranslation();
  const [links, setLinks] = useState<api.RetiredLink[]>([]);
  const [removing, setRemoving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    api
      .previewProjectRetiredLinks(projectId)
      .then((found) => {
        if (!cancelled) setLinks(found);
      })
      .catch((error) => console.error("Failed to check old skill links:", error));
    return () => {
      cancelled = true;
    };
  }, [projectId, refreshKey]);

  const ours = links.filter((link) => link.ours);
  if (ours.length === 0) return null;

  const remove = async () => {
    setRemoving(true);
    try {
      const left = await api.applyProjectRetiredLinks(projectId);
      const failed = left.filter((link) => link.ours).length;
      const kept = left.length - failed;
      if (ours.length > failed) {
        toast.success(t("project.retiredLinks.removed", { count: ours.length - failed }));
      }
      if (failed > 0) toast.error(t("project.retiredLinks.failed", { count: failed }));
      if (kept > 0) toast.message(t("project.retiredLinks.kept", { count: kept }));
      setLinks(left);
    } catch (error) {
      toast.error(getErrorMessage(error, t("common.error")));
    } finally {
      setRemoving(false);
    }
  };

  return (
    <section className="app-panel flex flex-wrap items-center gap-3 border-amber-500/40 bg-amber-500/5 px-3 py-2">
      <Link2Off className="h-4 w-4 shrink-0 text-amber-600 dark:text-amber-300" />
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium text-secondary">
          {t("project.retiredLinks.title", { count: ours.length })}
        </p>
        <p className="text-[12px] leading-5 text-muted" title={ours.map((link) => link.path).join("\n")}>
          {t("project.retiredLinks.hint")}
        </p>
      </div>
      <button type="button" className="app-button-secondary" onClick={remove} disabled={removing}>
        {removing && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
        {t("project.retiredLinks.remove")}
      </button>
    </section>
  );
}
