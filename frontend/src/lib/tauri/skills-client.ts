import { invokeCommand, type CommandError } from "@/lib/tauri/command-error";
import type { SkillsEvent } from "@/lib/tauri/generated/SkillsEvent";
import type { SkillsSnapshot } from "@/lib/tauri/generated/SkillsSnapshot";
import type { TimelineScope } from "@/lib/tauri/generated/TimelineScope";
import { subscribeStream } from "@/lib/tauri/stream-client";

type SkillsScope = TimelineScope;

export const skillsClient = {
  getSnapshot: (scope: SkillsScope) =>
    invokeCommand<SkillsSnapshot>("get_skills_snapshot", { scope }),
  subscribe: (
    scope: SkillsScope,
    onSnapshot: (snapshot: SkillsSnapshot) => void,
    onError: (error: CommandError) => void,
  ) =>
    subscribeStream<SkillsEvent>({
      streamKind: "skills",
      subscribeCommand: "subscribe_skills",
      unsubscribeCommand: "unsubscribe_skills",
      subscribeArguments: { scope },
      onEvent: (event) => onSnapshot(event.payload),
      onError,
    }),
};
