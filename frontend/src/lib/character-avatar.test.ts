import { afterEach, describe, expect, it, vi } from "vitest";
import characters from "@res/data/characters/characters.json";
import { panelImage } from "@/features/character-data/character-build-model";

const getSnapshot = vi.hoisted(() =>
  vi.fn<() => Promise<{ records: Array<{ id: number; avatar: string }> }>>(),
);

vi.mock("@/lib/tauri/character-data-client", () => ({
  characterDataClient: { getSnapshot },
}));

import {
  bootstrapCharacterAvatarCatalog,
  resolveCharacterAvatar,
} from "./character-avatar";

async function loadAvatars(records: Array<{ id: number; avatar: string }>) {
  getSnapshot.mockResolvedValueOnce({ records });
  await bootstrapCharacterAvatarCatalog();
}

afterEach(async () => {
  await loadAvatars([]);
});

describe("runtime character avatar catalog", () => {
  it.each(["1074", "1090"] as const)(
    "resolves appearance-named avatars for character %i in both views",
    async (id) => {
      const avatar = characters.characters[id].avatar;
      const file = avatar.split("/").at(-1)!;
      await loadAvatars([{ id: Number(id), avatar }]);
      expect(resolveCharacterAvatar(Number(id))).toContain(file);
      expect(panelImage("avatar", String(id))).toContain(file);
    },
  );

  it("replaces an older mapping instead of retaining the compiled catalog", async () => {
    await loadAvatars([
      {
        id: 1076,
        avatar: "res/images/characters/player_zhenhong.png",
      },
    ]);
    expect(resolveCharacterAvatar(1076)).toContain("player_zhenhong.png");

    await loadAvatars([
      {
        id: 1003,
        avatar: "res/images/characters/player_003.png",
      },
    ]);
    expect(resolveCharacterAvatar(1076)).toBeNull();
    expect(resolveCharacterAvatar(1003)).toContain("player_003.png");
  });

  it("ignores empty avatar paths", async () => {
    await loadAvatars([{ id: 1076, avatar: "  " }]);
    expect(resolveCharacterAvatar(1076)).toBeNull();
  });
});
