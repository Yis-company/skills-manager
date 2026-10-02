import { type InvokeArgs, type InvokeOptions, invoke as tauriInvoke } from "@tauri-apps/api/core";

import { isHostScoped } from "./hostScope";

/** The remote host the app operates on, or null for this computer. */
let activeHostId: null | string = null;

export function setActiveHostId(hostId: null | string) {
  activeHostId = hostId;
}

export function getActiveHostId(): null | string {
  return activeHostId;
}

/**
 * A check that the app is still on the host active now. A late answer from a
 * machine the app has switched away from must not replace the current
 * machine's data.
 */
export function trackHost(): () => boolean {
  const hostId = activeHostId;

  return () => activeHostId === hostId;
}

/**
 * Invoke against the host captured by the caller. This is important for
 * requests that outlive a host switch: the query key and the destination stay
 * bound to the same machine. Commands outside the host scope still run here.
 */
export function invokeHost<T>(
  hostId: null | string,
  command: string,
  args?: InvokeArgs,
  options?: InvokeOptions,
): Promise<T> {
  if (hostId !== null && isHostScoped(command, args)) {
    return tauriInvoke<T>("remote_invoke", { hostId, command, args: args ?? {} }, options);
  }

  return tauriInvoke<T>(command, args, options);
}

/**
 * `invoke` for the whole app: host-scoped commands go to the active host
 * through `remote_invoke`, which returns the same JSON the local command
 * would; everything else runs here.
 */
export function invoke<T>(command: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  return invokeHost(activeHostId, command, args, options);
}

/** `invoke` on this computer whatever host is active, for the few reads that
 *  must stay local although the same command is routed, like the proxy the
 *  app updater uses. */
export function invokeLocal<T>(
  command: string,
  args?: InvokeArgs,
  options?: InvokeOptions,
): Promise<T> {
  return tauriInvoke<T>(command, args, options);
}
