import { Loader2, Plug } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { getErrorMessage } from "../lib/error";
import * as api from "../lib/tauri";
import { AgentIcon } from "./AgentIcon";

const CLIENTS = [
  { key: "codex", name: "Codex" },
  { key: "claude_code", name: "Claude Code" },
] as const;

/**
 * Registers Agents Manager's MCP server with Codex or Claude Code on this
 * computer. The same core function backs `skills-manager-cli mcp register`.
 */
export function McpRegisterCard() {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<null | string>(null);

  const register = async (client: (typeof CLIENTS)[number]) => {
    if (busy) return;
    setBusy(client.key);

    try {
      const result = await api.registerMcpClient(client.key);

      const message =
        result.status === "added"
          ? t("mcpRegister.added", { agent: client.name })
          : t("mcpRegister.unchanged", { agent: client.name });

      toast.success(message, { description: result.nextStep });
    } catch (e) {
      toast.error(getErrorMessage(e, t("mcpRegister.errorGeneric")));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="app-panel p-4 transition-colors hover:border-border">
      <div className="flex items-start gap-3">
        <div className="mt-0.5 rounded-md border border-border-subtle bg-accent-bg p-2 text-accent-light">
          <Plug className="h-4 w-4" />
        </div>
        <div className="min-w-0 flex-1">
          <h3 className="text-[14px] font-semibold text-primary">{t("mcpRegister.title")}</h3>
          <p className="mt-0.5 text-[12px] leading-5 text-muted">{t("mcpRegister.body")}</p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {CLIENTS.map((client) => (
            <button
              key={client.key}
              onClick={() => register(client)}
              disabled={busy !== null}
              className="app-button-secondary h-[34px] disabled:opacity-50"
            >
              {busy === client.key ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <AgentIcon
                  agentKey={client.key}
                  displayName={client.name}
                  className="h-4 w-4 rounded-[4px]"
                />
              )}
              {client.name}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
