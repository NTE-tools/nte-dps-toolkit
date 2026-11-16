import { useCallback, useEffect, useRef, useState } from "react";

import { useSnapshotStream } from "@/hooks/use-snapshot-stream";
import {
  emptyCurtainClient,
  type ManageItemInput,
} from "@/lib/tauri/empty-curtain-client";
import type { CharacterEquipmentAction } from "@/lib/tauri/generated/CharacterEquipmentAction";
import type { EmptyCurtainFileResult } from "@/lib/tauri/generated/EmptyCurtainFileResult";
import type { EmptyCurtainSnapshot } from "@/lib/tauri/generated/EmptyCurtainSnapshot";
import type { ItemUidSnapshot } from "@/lib/tauri/generated/ItemUidSnapshot";
import { type CommandError, toCommandError } from "@/lib/tauri/command-error";

export function useEmptyCurtain() {
  const [notice, setNotice] = useState<CommandError | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const { state, refreshing, applySnapshot, retry } =
    useSnapshotStream<EmptyCurtainSnapshot>({
      keepReadyOnReload: true,
      getSnapshot: () => emptyCurtainClient.getSnapshot(),
      subscribe: (onSnapshot, onError) =>
        emptyCurtainClient.subscribe(onSnapshot, onError),
      reduceEvent: (_current, snapshot) => snapshot,
      onSnapshotApplied: (snapshot, source) => {
        if (source !== "stream") return;
        if (!snapshot.canOperate) {
          setNotice((current) =>
            current &&
            [
              "plugin_unavailable",
              "plugin_busy",
              "plugin_unsupported",
              "empty_curtain_operation_failed",
            ].includes(current.code)
              ? null
              : current,
          );
        } else if (snapshot.operation.status === "error") {
          setNotice((current) => {
            const argumentsMatch =
              current?.messageArguments.join("\u0000") ===
              snapshot.operation.messageArguments.join("\u0000");
            if (
              current?.messageKey === snapshot.operation.messageKey &&
              argumentsMatch
            ) {
              return current;
            }
            return {
              code: "empty_curtain_operation_failed",
              messageKey: snapshot.operation.messageKey,
              messageArguments: snapshot.operation.messageArguments,
            };
          });
        } else {
          // A recovered background read must remove its own old error toast,
          // without dismissing unrelated command/file-dialog errors.
          setNotice((current) =>
            current?.code === "empty_curtain_operation_failed" ? null : current,
          );
        }
      },
      onStreamError: (error) => setNotice(error),
    });

  const runSnapshotAction = useCallback(
    async (operation: Promise<EmptyCurtainSnapshot>) => {
      setActionPending(true);
      setNotice(null);
      try {
        const snapshot = await operation;
        if (mounted.current) applySnapshot(snapshot);
      } catch (error) {
        if (mounted.current) setNotice(toCommandError(error));
      } finally {
        if (mounted.current) setActionPending(false);
      }
    },
    [applySnapshot],
  );

  const runFileAction = useCallback(
    async (operation: Promise<EmptyCurtainFileResult>) => {
      setActionPending(true);
      setNotice(null);
      try {
        const result = await operation;
        if (mounted.current) applySnapshot(result.snapshot);
      } catch (error) {
        if (mounted.current) setNotice(toCommandError(error));
      } finally {
        if (mounted.current) setActionPending(false);
      }
    },
    [applySnapshot],
  );

  const positions = useCallback(
    async (item: ItemUidSnapshot, character: ItemUidSnapshot) => {
      try {
        return await emptyCurtainClient.positions(item, character);
      } catch (error) {
        if (mounted.current) setNotice(toCommandError(error));
        return [];
      }
    },
    [],
  );

  const canOperate = state.status === "ready" && state.snapshot.canOperate;

  return {
    state,
    notice,
    actionPending:
      actionPending ||
      (state.status === "ready" &&
        state.snapshot.operation.status === "pending"),
    refreshing,
    clearNotice: () => setNotice(null),
    retry: () => {
      if (state.status === "ready" && !canOperate) return;
      setNotice(null);
      retry();
    },
    positions,
    manageItem: (input: ManageItemInput) =>
      canOperate
        ? runSnapshotAction(emptyCurtainClient.manageItem(input))
        : Promise.resolve(),
    characterAction: (
      character: ItemUidSnapshot,
      action: CharacterEquipmentAction,
    ) =>
      canOperate
        ? runSnapshotAction(
            emptyCurtainClient.characterAction(character, action),
          )
        : Promise.resolve(),
    exportInventory: () => runFileAction(emptyCurtainClient.exportInventory()),
    exportLoadout: (character: ItemUidSnapshot) =>
      runFileAction(emptyCurtainClient.exportLoadout(character)),
    importLoadout: () =>
      canOperate
        ? runFileAction(emptyCurtainClient.importLoadout())
        : Promise.resolve(),
  };
}
