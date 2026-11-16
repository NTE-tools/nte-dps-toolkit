import { useSnapshotStream } from "@/hooks/use-snapshot-stream";
import { packetsClient } from "@/lib/tauri/packets-client";
import type { PacketsEvent } from "@/lib/tauri/generated/PacketsEvent";
import type { PacketsSnapshot } from "@/lib/tauri/generated/PacketsSnapshot";

import { mergePacketsEvent } from "./packets-model";

export function usePackets() {
  const { state, streamError, clearStreamError, retry } = useSnapshotStream<
    PacketsSnapshot,
    PacketsEvent
  >({
    getSnapshot: () => packetsClient.getSnapshot(),
    subscribe: (onEvent, onError) => packetsClient.subscribe(onEvent, onError),
    acceptInitial: (current, snapshot) =>
      mergePacketsEvent(current, { event: "snapshot", payload: snapshot }),
    reduceEvent: (current, event) => mergePacketsEvent(current, event),
  });

  return {
    state,
    streamError,
    clearStreamError,
    retry,
  };
}
