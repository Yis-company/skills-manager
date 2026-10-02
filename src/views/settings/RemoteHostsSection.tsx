import { confirm as dialogConfirm } from "@tauri-apps/plugin-dialog";
import {
  Check,
  Loader2,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  Server,
  Trash2,
  Unplug,
  X,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { useApp } from "../../context/AppContext";
import { getErrorMessage } from "../../lib/error";
import * as api from "../../lib/tauri";

interface RemoteHostForm {
  id: null | string;
  name: string;
  sshTarget: string;
  cliPath: string;
}

const EMPTY_HOST_FORM: RemoteHostForm = { id: null, name: "", sshTarget: "", cliPath: "" };

type RemoteProbeState =
  | { state: "checking" }
  | { state: "error"; message: string }
  | { state: "ok"; result: api.RemoteProbe };

export function RemoteHostsSection() {
  const { t } = useTranslation();

  const {
    remoteHosts,
    refreshRemoteHosts,
    activeHost,
    connectingHostId,
    updatingRemoteCliHostId,
    openRemoteCliUpdate,
    switchHost,
  } = useApp();

  const [form, setForm] = useState<null | RemoteHostForm>(null);
  const [saving, setSaving] = useState(false);
  const [probes, setProbes] = useState<Record<string, RemoteProbeState>>({});

  const fieldClass = "app-input bg-background";
  const actionButtonClass = "app-button-secondary gap-1.5";

  const handleSave = async () => {
    if (!form) return;
    setSaving(true);

    try {
      if (form.id) {
        await api.remoteHostUpdate(form.id, form.name, form.sshTarget, form.cliPath);
      } else {
        await api.remoteHostAdd(form.name, form.sshTarget, form.cliPath);
      }

      await refreshRemoteHosts();
      setForm(null);
      toast.success(t("remoteHosts.saved"));
    } catch (e) {
      toast.error(getErrorMessage(e, t("common.error")));
    } finally {
      setSaving(false);
    }
  };

  const handleRemove = async (host: api.RemoteHost) => {
    const confirmed = await dialogConfirm(t("remoteHosts.removeConfirm", { name: host.name }));

    if (!confirmed) return;

    try {
      if (activeHost?.id === host.id) await switchHost(null);
      await api.remoteHostRemove(host.id);
      await refreshRemoteHosts();
      toast.success(t("remoteHosts.removed"));
    } catch (e) {
      toast.error(getErrorMessage(e, t("common.error")));
    }
  };

  const handleProbe = async (host: api.RemoteHost) => {
    setProbes((prev) => ({ ...prev, [host.id]: { state: "checking" } }));

    try {
      const result = await api.remoteHostProbe(host.id);
      setProbes((prev) => ({ ...prev, [host.id]: { state: "ok", result } }));
    } catch (e) {
      const message = getErrorMessage(e, t("common.error"));
      setProbes((prev) => ({ ...prev, [host.id]: { state: "error", message } }));
    }
  };

  const renderProbe = (probe: RemoteProbeState | undefined) => {
    if (!probe) return null;

    if (probe.state === "checking") {
      return <Loader2 className="h-3 w-3 animate-spin text-muted" />;
    }

    if (probe.state === "error") {
      return <span className="text-[12px] text-red-500 dark:text-red-400">{probe.message}</span>;
    }

    const { version, compatible, app_version } = probe.result;

    return compatible ? (
      <span className="text-[12px] text-emerald-600 dark:text-emerald-400">
        {t("remoteHosts.probeOk", { version })}
      </span>
    ) : (
      <span className="text-[12px] text-amber-600 dark:text-amber-400">
        {t("remoteHosts.probeIncompatible", { version, appVersion: app_version })}
      </span>
    );
  };

  const renderUpdateAction = (host: api.RemoteHost) => {
    return (
      <button
        onClick={() => openRemoteCliUpdate(host.id)}
        disabled={
          probes[host.id]?.state === "checking" ||
          updatingRemoteCliHostId !== null ||
          connectingHostId !== null
        }
        className={actionButtonClass}
      >
        {t("remoteHosts.updateCli")}
      </button>
    );
  };

  return (
    <section>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h2 className="app-section-title">{t("remoteHosts.title")}</h2>
        <button
          onClick={() => setForm(EMPTY_HOST_FORM)}
          disabled={updatingRemoteCliHostId !== null}
          className="flex items-center gap-1 text-[13px] font-medium text-accent outline-none transition-colors hover:text-accent-light"
        >
          <Plus className="h-3.5 w-3.5" />
          {t("remoteHosts.add")}
        </button>
      </div>
      <p className="mb-3 text-[12px] text-muted">{t("remoteHosts.description")}</p>

      {form && (
        <div className="app-panel mb-3 space-y-2.5 p-4">
          <div className="flex items-center justify-between">
            <h3 className="text-[13px] font-medium text-secondary">
              {form.id ? t("remoteHosts.edit") : t("remoteHosts.add")}
            </h3>
            <button
              onClick={() => setForm(null)}
              className="text-muted outline-none hover:text-secondary"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <div>
            <label className="mb-1 block text-[12px] text-muted">
              {t("remoteHosts.form.name")}
            </label>
            <input
              type="text"
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              disabled={updatingRemoteCliHostId !== null}
              placeholder={t("remoteHosts.form.namePlaceholder")}
              className={`${fieldClass} w-full`}
            />
          </div>
          <div>
            <label className="mb-1 block text-[12px] text-muted">
              {t("remoteHosts.form.sshTarget")}
            </label>
            <input
              type="text"
              value={form.sshTarget}
              onChange={(e) => setForm({ ...form, sshTarget: e.target.value })}
              disabled={updatingRemoteCliHostId !== null}
              placeholder={t("remoteHosts.form.sshTargetPlaceholder")}
              className={`${fieldClass} w-full font-mono`}
            />
          </div>
          <div>
            <label className="mb-1 block text-[12px] text-muted">
              {t("remoteHosts.form.cliPath")}
            </label>
            <input
              type="text"
              value={form.cliPath}
              onChange={(e) => setForm({ ...form, cliPath: e.target.value })}
              disabled={updatingRemoteCliHostId !== null}
              placeholder={t("remoteHosts.form.cliPathPlaceholder")}
              className={`${fieldClass} w-full font-mono`}
            />
          </div>
          <div className="flex justify-end">
            <button
              onClick={handleSave}
              disabled={
                saving ||
                updatingRemoteCliHostId !== null ||
                !form.name.trim() ||
                !form.sshTarget.trim()
              }
              className={`${actionButtonClass} border-accent bg-accent text-white hover:opacity-90 disabled:opacity-50`}
            >
              {saving ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Check className="h-3 w-3" />
              )}
              {t("common.save")}
            </button>
          </div>
        </div>
      )}

      <div className="app-panel divide-y divide-border-faint overflow-hidden">
        {remoteHosts.length === 0 ? (
          <p className="px-4 py-3 text-[12px] text-muted">{t("remoteHosts.empty")}</p>
        ) : (
          remoteHosts.map((host) => (
            <div key={host.id} className="group flex items-center gap-3 px-4 py-3">
              <Server className="h-4 w-4 shrink-0 text-muted" />
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <span className="text-[13px] font-medium text-primary">{host.name}</span>
                  <span className="truncate font-mono text-[12px] text-muted">
                    {host.ssh_target}
                  </span>
                </div>
                <div className="mt-0.5 flex min-h-[16px] items-center gap-2">
                  {host.cli_path && (
                    <span className="truncate font-mono text-[11px] text-faint">
                      {host.cli_path}
                    </span>
                  )}
                  {renderProbe(probes[host.id])}
                </div>
              </div>
              {activeHost?.id === host.id ? (
                <button
                  onClick={() => void switchHost(null)}
                  disabled={connectingHostId !== null || updatingRemoteCliHostId !== null}
                  className={`${actionButtonClass} border-accent-border bg-accent-bg text-accent`}
                >
                  <Unplug className="h-3 w-3" />
                  {t("hostSwitcher.disconnect")}
                </button>
              ) : (
                <button
                  onClick={() => void switchHost(host.id)}
                  disabled={connectingHostId !== null || updatingRemoteCliHostId !== null}
                  className={`${actionButtonClass} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
                >
                  {connectingHostId === host.id ? (
                    <Loader2 className="h-3 w-3 animate-spin" />
                  ) : (
                    <Plug className="h-3 w-3" />
                  )}
                  {t("hostSwitcher.connect")}
                </button>
              )}
              <button
                onClick={() => handleProbe(host)}
                disabled={
                  probes[host.id]?.state === "checking" ||
                  updatingRemoteCliHostId !== null ||
                  connectingHostId !== null
                }
                className={`${actionButtonClass} border-border bg-surface-hover text-tertiary hover:bg-surface-active`}
              >
                <RefreshCw className="h-3 w-3" />
                {t("remoteHosts.probe")}
              </button>
              {renderUpdateAction(host)}
              <button
                onClick={() =>
                  setForm({
                    id: host.id,
                    name: host.name,
                    sshTarget: host.ssh_target,
                    cliPath: host.cli_path ?? "",
                  })
                }
                disabled={updatingRemoteCliHostId !== null || connectingHostId !== null}
                className="shrink-0 p-1 text-muted outline-none hover:text-accent"
                title={t("remoteHosts.edit")}
              >
                <Pencil className="h-3.5 w-3.5" />
              </button>
              <button
                onClick={() => handleRemove(host)}
                disabled={updatingRemoteCliHostId !== null}
                className="shrink-0 p-1 text-muted outline-none hover:text-red-400"
                title={t("remoteHosts.remove")}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
