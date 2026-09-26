import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, queryOptions } from "@tanstack/react-query";

const tauriInvoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauriInvoke }));

import { managedSkillsQueryOptions, refreshQuery } from "./appQueries";
import { setActiveHostId } from "./hostCall";

const clients: QueryClient[] = [];

function makeClient() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  return client;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

afterEach(() => {
  for (const client of clients.splice(0)) client.clear();
  tauriInvoke.mockReset();
  setActiveHostId(null);
});

describe("app query ownership", () => {
  it("sends a query to its captured host after the active host changes", async () => {
    const client = makeClient();
    setActiveHostId("host-new");
    tauriInvoke.mockResolvedValue([]);

    await client.fetchQuery(managedSkillsQueryOptions("host-old"));

    expect(tauriInvoke).toHaveBeenCalledWith(
      "remote_invoke",
      { hostId: "host-old", command: "get_managed_skills", args: {} },
      undefined
    );
  });
});

describe("refreshQuery", () => {
  it("cancels an older read before fetching the authoritative result", async () => {
    const client = makeClient();
    const oldRead = deferred<string[]>();
    let calls = 0;
    const options = queryOptions({
      queryKey: ["host", "host-1", "managedSkills"] as const,
      queryFn: () => {
        calls += 1;
        return calls === 1 ? oldRead.promise : Promise.resolve(["current"]);
      },
    });

    const staleRequest = client.fetchQuery(options).catch(() => undefined);
    await Promise.resolve();
    await expect(refreshQuery(client, options)).resolves.toEqual(["current"]);
    oldRead.resolve(["stale"]);
    await staleRequest;

    expect(client.getQueryData(options.queryKey)).toEqual(["current"]);
    expect(calls).toBe(2);
  });

  it("runs one trailing read when another change arrives during a refresh", async () => {
    const client = makeClient();
    const originalRead = deferred<string[]>();
    const refreshRead = deferred<string[]>();
    let calls = 0;
    let backendValue = "initial";
    const options = queryOptions({
      queryKey: ["host", "host-1", "projects"] as const,
      queryFn: () => {
        calls += 1;
        if (calls === 1) return originalRead.promise;
        if (calls === 2) return refreshRead.promise;
        return Promise.resolve([backendValue]);
      },
    });

    const initialRequest = client.fetchQuery(options).catch(() => undefined);
    await Promise.resolve();
    backendValue = "after-first-change";
    const refresh = refreshQuery(client, options);
    await vi.waitFor(() => expect(calls).toBe(2));
    backendValue = "after-second-change";
    const secondRefresh = refreshQuery(client, options);
    refreshRead.resolve(["after-first-change"]);
    await Promise.all([refresh, secondRefresh]);

    expect(client.getQueryData(options.queryKey)).toEqual(["after-second-change"]);
    expect(calls).toBe(3);
    originalRead.resolve(["initial"]);
    await initialRequest;
  });
});
