import {
  useCallback,
  useEffect,
  useEffectEvent,
  useRef,
  useState,
} from "react";

import { type CommandError, toCommandError } from "@/lib/tauri/command-error";

export type SnapshotStreamState<Snapshot> =
  | { status: "loading" }
  | { status: "error"; error: CommandError }
  | { status: "ready"; snapshot: Snapshot };

interface UseSnapshotStreamOptions<Snapshot, Event = Snapshot> {
  deps?: readonly unknown[];
  getSnapshot: () => Promise<Snapshot>;
  subscribe: (
    onEvent: (event: Event) => void,
    onError: (error: CommandError) => void,
  ) => () => Promise<void>;
  reduceEvent: (current: Snapshot | null, event: Event) => Snapshot | null;
  acceptInitial?: (
    current: Snapshot | null,
    snapshot: Snapshot,
  ) => Snapshot | null;
  keepReadyOnReload?: boolean;
  onSnapshotApplied?: (
    snapshot: Snapshot,
    source: "initial" | "stream" | "manual",
  ) => void;
  onStreamError?: (error: CommandError) => void;
}

/**
 * Loads an initial snapshot and subscribes to its ordered Tauri stream using a
 * single effect-scoped `active` guard to ignore stale responses after unmount
 * or dependency changes.
 */
export function useSnapshotStream<Snapshot, Event = Snapshot>({
  deps = [],
  getSnapshot,
  subscribe,
  reduceEvent,
  acceptInitial,
  keepReadyOnReload = false,
  onSnapshotApplied,
  onStreamError,
}: UseSnapshotStreamOptions<Snapshot, Event>) {
  const [state, setState] = useState<SnapshotStreamState<Snapshot>>({
    status: "loading",
  });
  const [streamError, setStreamError] = useState<CommandError | null>(null);
  const [refreshing, setRefreshing] = useState(true);
  const [reloadKey, setReloadKey] = useState(0);
  const snapshotRef = useRef<Snapshot | null>(null);
  const manualCallbacksRef = useRef({ acceptInitial, onSnapshotApplied });
  manualCallbacksRef.current = { acceptInitial, onSnapshotApplied };

  const loadSnapshot = useEffectEvent(getSnapshot);
  const openStream = useEffectEvent(subscribe);
  const reduceStreamEvent = useEffectEvent(reduceEvent);
  const reduceInitial = useEffectEvent(
    (current: Snapshot | null, incoming: Snapshot) =>
      acceptInitial ? acceptInitial(current, incoming) : incoming,
  );
  const notifyApplied = useEffectEvent(
    (snapshot: Snapshot, source: "initial" | "stream" | "manual") => {
      onSnapshotApplied?.(snapshot, source);
    },
  );
  const notifyStreamError = useEffectEvent((error: CommandError) => {
    onStreamError?.(error);
  });

  const applySnapshot = useCallback((snapshot: Snapshot): boolean => {
    const { acceptInitial: accept, onSnapshotApplied: onApplied } =
      manualCallbacksRef.current;
    const next = accept ? accept(snapshotRef.current, snapshot) : snapshot;
    if (next === null) return false;
    snapshotRef.current = next;
    setState({ status: "ready", snapshot: next });
    onApplied?.(next, "manual");
    return true;
  }, []);

  useEffect(() => {
    let active = true;
    let receivedInEffect = false;
    if (!keepReadyOnReload) {
      snapshotRef.current = null;
      setState({ status: "loading" });
    } else if (snapshotRef.current === null) {
      setState({ status: "loading" });
    }
    setRefreshing(true);
    setStreamError(null);

    void loadSnapshot()
      .then((snapshot) => {
        if (!active) return;
        const next = reduceInitial(snapshotRef.current, snapshot);
        if (next !== null) {
          snapshotRef.current = next;
          setState({ status: "ready", snapshot: next });
          notifyApplied(next, "initial");
        }
      })
      .catch((error: unknown) => {
        if (!active || receivedInEffect) return;
        const parsed = toCommandError(error);
        if (snapshotRef.current === null) {
          setState({ status: "error", error: parsed });
        } else {
          setStreamError(parsed);
        }
        notifyStreamError(parsed);
      })
      .finally(() => {
        if (active) setRefreshing(false);
      });

    const unsubscribe = openStream(
      (event) => {
        if (!active) return;
        const next = reduceStreamEvent(snapshotRef.current, event);
        if (next !== null) {
          receivedInEffect = true;
          snapshotRef.current = next;
          setStreamError(null);
          setState({ status: "ready", snapshot: next });
          notifyApplied(next, "stream");
        }
      },
      (error) => {
        if (!active) return;
        setStreamError(error);
        notifyStreamError(error);
      },
    );

    return () => {
      active = false;
      void unsubscribe().catch(() => undefined);
    };
    // `deps` are caller-provided primitive effect keys (such as scope).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [keepReadyOnReload, reloadKey, ...deps]);

  const retry = useCallback(() => {
    setReloadKey((value) => value + 1);
  }, []);

  const clearStreamError = useCallback(() => {
    setStreamError(null);
  }, []);

  return {
    state,
    streamError,
    refreshing,
    applySnapshot,
    clearStreamError,
    retry,
  };
}
