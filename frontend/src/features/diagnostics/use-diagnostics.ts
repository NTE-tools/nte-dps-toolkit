import { useCallback, useEffect, useRef, useState } from "react";

import { useSnapshotStream } from "@/hooks/use-snapshot-stream";
import { diagnosticsClient } from "@/lib/tauri/diagnostics-client";
import type { DiagnosticsSnapshot } from "@/lib/tauri/generated/DiagnosticsSnapshot";
import { type CommandError, toCommandError } from "@/lib/tauri/command-error";

import { diagnosticsSnapshotIsAtLeast } from "./diagnostics-model";

export type DiagnosticsAction =
  "run" | "import-pcapng" | "import-json" | "export-json" | "export-pcapng";

export function useDiagnostics() {
  const [actionError, setActionError] = useState<CommandError | null>(null);
  const [pendingAction, setPendingAction] = useState<DiagnosticsAction | null>(
    null,
  );
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const acceptIfAtLeast = (
    current: DiagnosticsSnapshot | null,
    incoming: DiagnosticsSnapshot,
  ) => (diagnosticsSnapshotIsAtLeast(incoming, current) ? incoming : null);

  const { state, streamError, applySnapshot, clearStreamError, retry } =
    useSnapshotStream<DiagnosticsSnapshot>({
      keepReadyOnReload: true,
      getSnapshot: () => diagnosticsClient.getSnapshot(),
      subscribe: (onSnapshot, onError) =>
        diagnosticsClient.subscribe(onSnapshot, onError),
      acceptInitial: acceptIfAtLeast,
      reduceEvent: acceptIfAtLeast,
    });

  const perform = useCallback(
    async (action: DiagnosticsAction): Promise<boolean> => {
      setPendingAction(action);
      setActionError(null);
      try {
        if (action === "run") {
          const snapshot = await diagnosticsClient.run();
          if (mounted.current) applySnapshot(snapshot);
          return true;
        }
        const result = await {
          "import-pcapng": diagnosticsClient.importPcapng,
          "import-json": diagnosticsClient.importJson,
          "export-json": diagnosticsClient.exportJson,
          "export-pcapng": diagnosticsClient.exportPcapng,
        }[action]();
        if (mounted.current) applySnapshot(result.snapshot);
        return result.performed;
      } catch (error) {
        if (mounted.current) setActionError(toCommandError(error));
        return false;
      } finally {
        if (mounted.current) setPendingAction(null);
      }
    },
    [applySnapshot],
  );

  return {
    state,
    streamError,
    actionError,
    pendingAction,
    perform,
    clearNotice: () => {
      clearStreamError();
      setActionError(null);
    },
    retry,
  };
}
