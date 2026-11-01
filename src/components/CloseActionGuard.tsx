import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

import { useWorkspaceChanges } from "../context/WorkspaceChangesContext";
import * as api from "../lib/tauri";
import { CloseActionDialog } from "./CloseActionDialog";

export function CloseActionGuard() {
  const { confirmLeave } = useWorkspaceChanges();
  const [dialogOpen, setDialogOpen] = useState(false);

  useEffect(() => {
    const unlisten = listen("window-close-requested", async () => {
      const tray = await api.getSettings("show_tray_icon");

      const trayEnabled = (() => {
        const normalized = (tray ?? "true").trim().toLowerCase();

        return !(
          normalized === "false" ||
          normalized === "0" ||
          normalized === "no" ||
          normalized === "off"
        );
      })();

      if (!trayEnabled) {
        if (await confirmLeave()) api.appExit();

        return;
      }

      const pref = await api.getSettings("close_action");

      if (pref === "close") {
        if (await confirmLeave()) api.appExit();
      } else if (pref === "hide") {
        await api.hideToTray();
      } else {
        setDialogOpen(true);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [confirmLeave]);

  const handleClose = async (remember: boolean) => {
    if (!(await confirmLeave())) return;
    setDialogOpen(false);

    if (remember) await api.setSettings("close_action", "close");
    api.appExit();
  };

  const handleHide = async (remember: boolean) => {
    setDialogOpen(false);

    if (remember) await api.setSettings("close_action", "hide");
    await api.hideToTray();
  };

  return (
    <CloseActionDialog
      open={dialogOpen}
      onCancel={() => setDialogOpen(false)}
      onClose={handleClose}
      onHide={handleHide}
    />
  );
}
