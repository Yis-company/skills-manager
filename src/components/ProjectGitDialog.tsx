import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { GitBranch, GitCommit, Loader2, RefreshCw, Upload, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useApp } from "../context/AppContext";
import { getErrorMessage } from "../lib/error";
import {
  projectGitQueryOptions,
  projectGitRequest,
  type ProjectGitPrReview,
  type ProjectGitPushReview,
  type ProjectGitRequest,
  type ProjectGitStatus,
} from "../lib/projectGit";

interface Props {
  hostId: string | null;
  projectId: string;
  onClose: () => void;
}

export function ProjectGitDialog({ hostId, projectId, onClose }: Props) {
  const { t } = useTranslation();
  const { remoteHosts } = useApp();
  const hostName = hostId ? remoteHosts.find((host) => host.id === hostId)?.name ?? hostId : t("project.git.localHost");
  const dialogRef = useRef<HTMLElement>(null);
  const requestOrder = useRef(0);
  const mounted = useRef(true);
  const queryClient = useQueryClient();
  const query = useQuery(projectGitQueryOptions(hostId, projectId));
  const [status, setStatus] = useState<ProjectGitStatus | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState("");
  const [remote, setRemote] = useState("");
  const [branch, setBranch] = useState("");
  const [pushReview, setPushReview] = useState<ProjectGitPushReview | null>(null);
  const [prReview, setPrReview] = useState<ProjectGitPrReview | null>(null);
  const [prBases, setPrBases] = useState<string[]>([]);
  const [createdPrUrl, setCreatedPrUrl] = useState<string | null>(null);
  const [base, setBase] = useState("");
  const [title, setTitle] = useState("");
  const [newBranch, setNewBranch] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (!query.data || query.isFetching || query.error || status) return;
    setStatus(query.data);
    setSelected(new Set(query.data.files.filter((file) => file.skill_related && !file.blocked_reason).map((file) => file.path)));
    setRemote(query.data.upstream_remote ?? (query.data.remotes.length === 1 ? query.data.remotes[0].name : ""));
    setBranch(query.data.upstream_branch ?? query.data.branch ?? "");
  }, [query.data, query.error, query.isFetching, status]);

  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    requestAnimationFrame(() => {
      const dialog = dialogRef.current;
      const firstControl = dialog?.querySelector<HTMLElement>("button:not([disabled]), input:not([disabled])");
      (firstControl ?? dialog)?.focus();
    });
    return () => previousFocus?.focus();
  }, []);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      requestOrder.current = requestOrder.current + 1;
    };
  }, []);

  const refresh = async (preservePr = false) => {
    const order = ++requestOrder.current;
    const fresh = await queryClient.fetchQuery({ ...projectGitQueryOptions(hostId, projectId), staleTime: 0 });
    if (!mounted.current || order !== requestOrder.current) return;
    const branchChanged = status !== null && fresh.branch !== status.branch;
    setStatus(fresh);
    setSelected((current) => new Set([...current].filter((path) => fresh.files.some((file) => file.path === path && !file.blocked_reason))));
    setPushReview(null);
    if (branchChanged) {
      setBranch(fresh.upstream_branch ?? fresh.branch ?? "");
      setBase("");
      setCreatedPrUrl(null);
    }
    if (!preservePr || branchChanged) {
      setPrReview(null);
      setPrBases([]);
    }
  };

  const run = async (action: (isCurrent: () => boolean) => Promise<unknown>, after = true, success?: string, preservePr = false) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    const order = ++requestOrder.current;
    const isCurrent = () => mounted.current && order === requestOrder.current;
    try {
      await action(isCurrent);
      if (!isCurrent()) return;
      if (mounted.current && success) setNotice(success);
      if (after) {
        try {
          await refresh(preservePr);
        } catch (e) {
          if (mounted.current) setError(getErrorMessage(e, t("common.error")));
        }
      }
    } catch (e) {
      if (!isCurrent()) return;
      setError(getErrorMessage(e, t("common.error")));
      try {
        await refresh();
      } catch {
        // Keep the action error visible if the follow-up status read also fails.
      }
    } finally {
      if (mounted.current) setBusy(false);
    }
  };

  const keyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape" && !busy) onClose();
    if (event.key !== "Tab" || !dialogRef.current) return;
    const items = [...dialogRef.current.querySelectorAll<HTMLElement>("button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])")];
    if (!items.length) return;
    if (event.shiftKey && document.activeElement === items[0]) { event.preventDefault(); items[items.length - 1].focus(); }
    else if (!event.shiftKey && document.activeElement === items[items.length - 1]) { event.preventDefault(); items[0].focus(); }
  };

  if (!status && query.error) return (
    <Shell title={t("project.git.title")} onClose={onClose} dialogRef={dialogRef} onKeyDown={keyDown}>
      <p role="alert" className="text-[12px] text-red-500">{getErrorMessage(query.error, t("common.error"))}</p>
      <button className="app-button-secondary mt-3" onClick={() => void query.refetch()}>{t("common.retry")}</button>
    </Shell>
  );
  if (!status) return (
    <Shell title={t("project.git.title")} onClose={onClose} dialogRef={dialogRef} onKeyDown={keyDown}>
      <p className="flex items-center gap-2 text-[12px] text-muted">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("common.loading")}
      </p>
    </Shell>
  );

  const canCommit = !busy && !status.blocked_reason && Boolean(message.trim()) && selected.size > 0 && [...selected].every((path) => status.files.some((file) => file.path === path && !file.blocked_reason));
  const canCreateBranch = !busy && !status.blocked_reason && status.files.length === 0 && Boolean(status.branch && newBranch.trim());
  const runRequest = <R extends ProjectGitRequest,>(request: R) => projectGitRequest(hostId, projectId, request);
  const prReviewCurrent = prReview?.base === base;
  const existingUrl = prReviewCurrent ? prReview?.existing_url : null;
  const createPrAction = createdPrUrl || existingUrl
    ? () => void openUrl(createdPrUrl ?? existingUrl!).catch((e) => setError(getErrorMessage(e, t("common.error"))))
    : () => void run(async (isCurrent) => {
      const url = await runRequest({ action: "create_pr", review_id: prReview!.review_id, title: title.trim() });
      if (!isCurrent()) return;
      setCreatedPrUrl(url);
    }, true, t("project.git.prCreated"), true);

  return (
    <Shell title={t("project.git.title")} onClose={onClose} dialogRef={dialogRef} onKeyDown={keyDown} busy={busy}>
    <dl className="mb-4 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[12px]">
      <dt className="text-muted">{t("project.git.host")}</dt><dd className="truncate text-primary">{hostName}</dd>
      <dt className="text-muted">{t("project.git.root")}</dt><dd className="break-all font-mono text-primary">{status.root}</dd>
      <dt className="text-muted">{t("project.git.branch")}</dt><dd className="font-mono text-primary">{status.branch ?? "—"}</dd>
    </dl>
    {(status.blocked_reason || status.files.some((file) => file.blocked_reason)) && (
      <p role="status" className="mb-3 rounded-md border border-amber-500/30 bg-amber-500/10 p-2 text-[12px] text-secondary">
        {status.blocked_reason ?? t("project.git.someFilesBlocked")}
      </p>
    )}
    <div className="max-h-[34vh] space-y-2 overflow-y-auto pr-1">
      {status.files.length === 0 ? (
        <p className="text-[12px] text-muted">{t("project.git.clean")}</p>
      ) : status.files.map((file) => (
        <article key={file.path} className="rounded-md border border-border-subtle bg-surface p-2">
          <label className="flex items-start gap-2 text-[12px]">
            <input
              type="checkbox"
              aria-label={file.path}
              checked={selected.has(file.path)}
              disabled={busy || Boolean(file.blocked_reason) || Boolean(status.blocked_reason)}
              onChange={(event) => setSelected((current) => {
                const next = new Set(current);
                if (event.target.checked) next.add(file.path);
                else next.delete(file.path);
                return next;
              })}
            />
            <span className="min-w-0 flex-1">
              <span className="break-all font-mono text-primary">{file.path}</span>
              <span className="ml-2 text-muted">{file.status}</span>
            </span>
          </label>
          {file.blocked_reason && <p className="mt-1 text-[11px] text-amber-600">{file.blocked_reason}</p>}
          {file.diff && (
            <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded bg-surface p-2 font-mono text-[10px] text-tertiary">
              {file.diff}{file.truncated && `\n${t("project.git.diffTruncated")}`}
            </pre>
          )}
        </article>
      ))}
    </div>
    <section className="mt-4 border-t border-border-subtle pt-3">
      <h3 className="mb-2 flex items-center gap-2 text-[12px] font-semibold text-primary">
        <GitCommit className="h-3.5 w-3.5" />{t("project.git.commit")}
      </h3>
      <textarea aria-label={t("project.git.messageLabel")} value={message} onChange={(e) => setMessage(e.target.value)} disabled={busy} placeholder={t("project.git.messagePlaceholder")} className="app-input min-h-16 w-full resize-y text-[12px]" />
      <p className="mt-1 text-[11px] text-muted">{t("project.git.selectedCount", { count: selected.size })}</p>
      <button disabled={!canCommit} className="app-button-secondary mt-2 disabled:opacity-50" onClick={() => void run(() => runRequest({ action: "commit", review_id: status.review_id, paths: [...selected], message: message.trim() }), true, t("project.git.committed"))}>{t("project.git.commitSelected")}</button>
    </section>
    <section className="mt-4 border-t border-border-subtle pt-3">
      <h3 className="mb-2 flex items-center gap-2 text-[12px] font-semibold text-primary">
        <Upload className="h-3.5 w-3.5" />{t("project.git.push")}
      </h3>
      <div className="flex flex-wrap gap-2">
        <select aria-label={t("project.git.remoteLabel")} className="app-input min-w-28 flex-1 text-[12px]" value={remote} onChange={(e) => { setRemote(e.target.value); setPushReview(null); setPrReview(null); setPrBases([]); setBase(""); setCreatedPrUrl(null); }} disabled={busy}>
          <option value="">{t("project.git.chooseRemote")}</option>
          {status.remotes.map((item) => <option key={item.name} value={item.name}>{item.name} · {item.url}</option>)}
        </select>
        <input value={branch} onChange={(e) => { setBranch(e.target.value); setPushReview(null); setPrReview(null); }} disabled={busy} aria-label={t("project.git.targetBranch")} className="app-input min-w-28 flex-1 font-mono text-[12px]" />
      </div>
      <button disabled={busy || !remote || !branch} className="app-button-secondary mt-2 disabled:opacity-50" onClick={() => void run(async (isCurrent) => {
        const preview = await runRequest({ action: "push_preview", remote, branch });
        if (isCurrent()) setPushReview(preview);
      }, false)}>{t("project.git.reviewPush")}</button>
      {pushReview && (
        <div className="mt-2 rounded-md border border-border-subtle p-2 text-[11px]">
          <p className="break-all text-secondary">{pushReview.url} · {pushReview.branch}</p>
          <ul className="mt-1 max-h-28 overflow-y-auto">
            {pushReview.commits.map((commit) => (
              <li key={commit.id} className="font-mono text-tertiary">
                <span>{commit.id.slice(0, 8)} </span><span>{commit.summary}</span>
              </li>
            ))}
          </ul>
          <button disabled={busy} className="app-button-secondary mt-2 disabled:opacity-50" onClick={() => void run(() => runRequest({ action: "push", review_id: pushReview.review_id }), true, t("project.git.pushed"))}>
            {t("project.git.pushNow")}
          </button>
        </div>
      )}
    </section>
    <section className="mt-4 border-t border-border-subtle pt-3">
      <h3 className="mb-2 flex items-center gap-2 text-[12px] font-semibold text-primary">
        <GitBranch className="h-3.5 w-3.5" />{t("project.git.pullRequest")}
      </h3>
      <div className="flex flex-wrap gap-2">
        <select aria-label={t("project.git.remoteLabel")} className="app-input min-w-28 flex-1 text-[12px]" value={remote} onChange={(e) => { setRemote(e.target.value); setPushReview(null); setPrReview(null); setPrBases([]); setBase(""); setCreatedPrUrl(null); }} disabled={busy}>
          <option value="">{t("project.git.chooseRemote")}</option>
          {status.remotes.map((item) => <option key={item.name} value={item.name}>{item.name}</option>)}
        </select>
        <select aria-label={t("project.git.baseBranch")} className="app-input min-w-28 flex-1 text-[12px]" value={base} onChange={(e) => { setBase(e.target.value); setCreatedPrUrl(null); }} disabled={busy || !prBases.length}>
          <option value="">{t("project.git.chooseBase")}</option>
          {prBases.map((item) => <option key={item} value={item}>{item}</option>)}
        </select>
      </div>
      <input aria-label={t("project.git.prTitle")} className="app-input mt-2 w-full text-[12px]" value={title} onChange={(e) => setTitle(e.target.value)} disabled={busy} placeholder={t("project.git.prTitle")} />
      <button disabled={busy || !remote} className="app-button-secondary mt-2 disabled:opacity-50" onClick={() => void run(async (isCurrent) => {
        const review = await runRequest({ action: "pr_preview", remote, base: base || null });
        if (!isCurrent()) return;
        setPrReview(review);
        setPrBases(review.bases);
        setBase(review.base);
        setTitle((old) => old || review.head);
        setCreatedPrUrl(null);
      }, false)}>{t("project.git.reviewPr")}</button>
      {prReview && (
        <div className="mt-2 rounded-md border border-border-subtle p-2">
          <p className="text-[11px] text-secondary">{prReview.repository}: {prReview.head} → {base || prReview.base}</p>
          <button disabled={busy || (!createdPrUrl && (!prReviewCurrent || (!existingUrl && (!title.trim() || !base))))} className="app-button-secondary mt-2 disabled:opacity-50" onClick={createPrAction}>
            {createdPrUrl || existingUrl ? t("project.git.openPr") : t("project.git.createPr")}
          </button>
        </div>
      )}
    </section>
    {status.files.length === 0 && (
      <section className="mt-4 border-t border-border-subtle pt-3">
        <label className="text-[12px] font-semibold text-primary">{t("project.git.createBranch")}</label>
        <div className="mt-2 flex gap-2">
          <input className="app-input min-w-0 flex-1 font-mono text-[12px]" value={newBranch} onChange={(event) => setNewBranch(event.target.value)} disabled={busy || Boolean(status.blocked_reason)} placeholder={t("project.git.branchPlaceholder")} />
          <button disabled={!canCreateBranch} className="app-button-secondary disabled:opacity-50" onClick={() => void run(() => runRequest({ action: "create_branch", review_id: status.review_id, name: newBranch.trim() }), true, t("project.git.branchCreated"))}>
            {t("common.create")}
          </button>
        </div>
      </section>
    )}
    {error && <p role="alert" className="mt-3 rounded-md border border-red-500/30 bg-red-500/10 p-2 text-[12px] text-red-500">{error}</p>}
    {notice && <p role="status" className="mt-3 text-[12px] text-accent">{notice}</p>}
    {busy && (
      <p role="status" className="mt-3 flex items-center gap-2 text-[11px] text-muted">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />{t("project.git.working")}
      </p>
    )}
    <footer className="mt-4 flex justify-end gap-2 border-t border-border-subtle pt-3">
      <button className="app-button-secondary" disabled={busy} onClick={() => void run(async () => {}, true)}>
        <RefreshCw className="mr-1 inline h-3 w-3" />{t("project.git.refresh")}
      </button>
      <button className="app-button-secondary" disabled={busy} onClick={onClose}>{t("project.git.close")}</button>
    </footer>
    </Shell>
  );
}

function Shell({ title, onClose, dialogRef, onKeyDown, children, busy = false }: { title: string; onClose: () => void; dialogRef: RefObject<HTMLElement | null>; onKeyDown: (event: KeyboardEvent<HTMLElement>) => void; children: ReactNode; busy?: boolean }) {
  const { t } = useTranslation();
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center" role="presentation">
      <button aria-label={t("project.git.close")} className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={onClose} disabled={busy} />
      <section ref={dialogRef} tabIndex={-1} onKeyDown={onKeyDown} role="dialog" aria-modal="true" aria-labelledby="project-git-title" className="relative mx-3 flex max-h-[92vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-border bg-surface p-4 shadow-2xl outline-none sm:p-5">
        <header className="mb-3 flex items-center justify-between gap-3">
          <h2 id="project-git-title" className="text-[14px] font-semibold text-primary">{title}</h2>
          <button aria-label={t("project.git.close")} onClick={onClose} disabled={busy} className="rounded p-1 text-muted hover:text-secondary"><X className="h-4 w-4" /></button>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto pr-1">{children}</div>
      </section>
    </div>
  );
}
