import { invokeLocal } from "./hostCall";
import type { OperationSnapshot } from "./operations";

export interface ShutdownRequest {
  id: number;
  target: "install_update" | "quit" | "restart";
  mode: "cancel" | "drain" | null;
}

export interface ShutdownPrepared {
  ready: boolean;
  snapshots: OperationSnapshot[];
}

export type ShutdownAction =
  | { action: "choose"; id: number; mode: "cancel" | "drain" }
  | { action: "finish" | "keep_open" | "prepare" | "resume"; id: number }
  | { action: "inspect" | "read" };

export const shutdownRequest = <T>(request: ShutdownAction) =>
  invokeLocal<T>("shutdown_request", { request });
