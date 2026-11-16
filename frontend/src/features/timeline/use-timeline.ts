import { useCallback, useEffect, useRef, useState } from "react";

import { useSnapshotStream } from "@/hooks/use-snapshot-stream";
import { shouldAcceptDecimalVersion } from "@/lib/decimal-string";
import { timelineClient } from "@/lib/tauri/timeline-client";
import type { TimelineDpsViewMode } from "@/lib/tauri/generated/TimelineDpsViewMode";
import type { TimelineScope } from "@/lib/tauri/generated/TimelineScope";
import type { TimelineSnapshot } from "@/lib/tauri/generated/TimelineSnapshot";
import { type CommandError, toCommandError } from "@/lib/tauri/command-error";

export function useTimeline() {
  const [scope, setScope] = useState<TimelineScope>("all");
  const [pending, setPending] = useState(false);
  const [mutationError, setMutationError] = useState<CommandError | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const { state, streamError, applySnapshot, clearStreamError, retry } =
    useSnapshotStream<TimelineSnapshot>({
      deps: [scope],
      getSnapshot: () => timelineClient.getSnapshot(scope),
      subscribe: (onSnapshot, onError) =>
        timelineClient.subscribe(scope, onSnapshot, onError),
      acceptInitial: (current, snapshot) =>
        snapshot.scope === scope &&
        shouldAcceptDecimalVersion(
          current?.generation ?? null,
          snapshot.generation,
          true,
        )
          ? snapshot
          : null,
      reduceEvent: (current, snapshot) =>
        snapshot.scope === scope &&
        shouldAcceptDecimalVersion(
          current?.generation ?? null,
          snapshot.generation,
        )
          ? snapshot
          : null,
    });

  const setPreferences = useCallback(
    async (bucketSeconds: number, viewMode: TimelineDpsViewMode) => {
      if (pending) return;
      setPending(true);
      setMutationError(null);
      clearStreamError();
      try {
        const snapshot = await timelineClient.setPreferences(
          scope,
          bucketSeconds,
          viewMode,
        );
        if (mounted.current && snapshot.scope === scope) {
          applySnapshot(snapshot);
        }
      } catch (error) {
        if (mounted.current) setMutationError(toCommandError(error));
      } finally {
        if (mounted.current) setPending(false);
      }
    },
    [applySnapshot, clearStreamError, pending, scope],
  );

  return {
    state,
    scope,
    setScope,
    pending,
    mutationError: mutationError ?? streamError,
    retry: () => {
      setMutationError(null);
      retry();
    },
    setPreferences,
  };
}
