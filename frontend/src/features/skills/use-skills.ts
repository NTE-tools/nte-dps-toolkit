import { useState } from "react";

import { useSnapshotStream } from "@/hooks/use-snapshot-stream";
import { shouldAcceptDecimalVersion } from "@/lib/decimal-string";
import { skillsClient } from "@/lib/tauri/skills-client";
import type { SkillsSnapshot } from "@/lib/tauri/generated/SkillsSnapshot";
import type { TimelineScope } from "@/lib/tauri/generated/TimelineScope";

export function useSkills() {
  const [scope, setScope] = useState<TimelineScope>("all");

  const { state, streamError, clearStreamError, retry } =
    useSnapshotStream<SkillsSnapshot>({
      deps: [scope],
      getSnapshot: () => skillsClient.getSnapshot(scope),
      subscribe: (onSnapshot, onError) =>
        skillsClient.subscribe(scope, onSnapshot, onError),
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

  return {
    state,
    scope,
    setScope,
    streamError,
    clearStreamError,
    retry,
  };
}
