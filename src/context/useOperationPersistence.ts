import type { QueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";

import { invokeLocal } from "../lib/hostCall";
import {
  mergeOperations,
  operationNeedsSession,
  operationQueryKey,
  type OperationSnapshot,
} from "../lib/operations";

export interface SavedOperationResults {
  version: number;
  results: OperationSnapshot[];
}

export type OperationResultsRequest =
  | { action: "read" }
  | { action: "write"; version: number; results: OperationSnapshot[] };

/** One serialized writer for the existing contextual cache. Failed saves retain memory.
 * flush() includes changes arriving during a save and rejects until all are durable.
 */
export function useOperationPersistence(
  client: QueryClient,
  observe: (host: null | string) => void,
) {
  const [status, setStatus] = useState<"error" | "loading" | "saved" | "saving">("loading");
  const [observationReady, setObservationReady] = useState(false);

  const state = useRef<{
    version: number;
    saved: string;
    failed: null | string;
    ready: boolean;
    loading: null | Promise<void>;
    saving: null | Promise<void>;
  }>({ version: 0, saved: "", failed: null, ready: false, loading: null, saving: null });

  const collect = useCallback(
    () =>
      client
        .getQueriesData<OperationSnapshot[]>({
          predicate: (q) =>
            q.queryKey.length === 3 && q.queryKey[0] === "host" && q.queryKey[2] === "operations",
        })
        .flatMap(([, records]) => records ?? [])
        .filter(operationNeedsSession)
        .map(({ restored: _restored, ...operation }) => operation),
    [client],
  );

  const load = useCallback(() => {
    if (state.current.ready) return Promise.resolve();

    if (state.current.loading) return state.current.loading;
    state.current.loading = invokeLocal<SavedOperationResults>("operation_results_request", {
      request: { action: "read" },
    })
      .then((saved) => {
        state.current.version = saved.version;

        for (const result of saved.results) {
          const origin = result.host_id;
          client.setQueryData<OperationSnapshot[]>(operationQueryKey(origin), (current = []) =>
            mergeOperations([{ ...result, restored: true }], current, origin),
          );
          observe(origin);
        }

        state.current.ready = true;
        setObservationReady(true);
        state.current.saved = JSON.stringify(saved.results);
        setStatus("saved");
      })
      .catch(() => {
        // Current observations remain usable; no save is allowed until the
        // original document can be loaded and conservatively merged.
        setObservationReady(true);
        setStatus("error");
        throw new Error("Could not load saved operation results");
      })
      .finally(() => {
        state.current.loading = null;
      });

    return state.current.loading;
  }, [client, observe]);

  const flush = useCallback(
    async function flush() {
      await load();

      if (state.current.saving) {
        await state.current.saving;

        return flush();
      }

      const save = async () => {
        try {
          for (;;) {
            const results = collect();
            const serialized = JSON.stringify(results);

            if (serialized === state.current.saved) break;
            setStatus("saving");

            const saved = await invokeLocal<SavedOperationResults>("operation_results_request", {
              request: { action: "write", version: state.current.version, results },
            });

            state.current.version = saved.version;
            state.current.saved = serialized;
          }

          state.current.failed = null;
          setStatus("saved");
        } catch {
          state.current.failed = JSON.stringify(collect());
          setStatus("error");
          throw new Error("Could not save operation results; retry saving before closing the app");
        }
      };

      state.current.saving = save().finally(() => {
        state.current.saving = null;
      });

      return state.current.saving;
    },
    [collect, load],
  );

  useEffect(() => {
    void flush().catch(() => {});

    return client.getQueryCache().subscribe((event) => {
      if (
        state.current.ready &&
        event.type === "updated" &&
        event.action.type === "success" &&
        event.query.queryKey[2] === "operations" &&
        JSON.stringify(collect()) !== state.current.failed
      )
        void flush().catch(() => {});
    });
  }, [client, collect, flush]);

  return { persistenceStatus: status, observationReady, flush };
}
