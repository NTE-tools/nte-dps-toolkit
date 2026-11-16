import { afterEach, describe, expect, it, vi } from "vitest";

import {
  CONSOLE_PAGE_IDS,
  DEFAULT_CONSOLE_PAGE,
  adjacentConsolePage,
  consolePageActivityMode,
  isConsolePageId,
  readConsoleSidebarCollapsed,
  readConsoleFavoritePages,
  resolveConsoleShortcut,
  writeConsoleSidebarCollapsed,
  writeConsoleFavoritePages,
} from "./console-navigation";

function stubLocalStorage() {
  const values = new Map<string, string>();
  vi.stubGlobal("window", {
    localStorage: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
    },
  });
  return values;
}

describe("Console navigation", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("validates Console page identifiers", () => {
    for (const page of CONSOLE_PAGE_IDS) {
      expect(isConsolePageId(page)).toBe(true);
    }
    expect(isConsolePageId("toString")).toBe(false);
    expect(isConsolePageId("unknown-page")).toBe(false);
  });

  it("keeps exactly the active page visible", () => {
    for (const activePage of CONSOLE_PAGE_IDS) {
      expect(
        CONSOLE_PAGE_IDS.map((page) =>
          consolePageActivityMode(activePage, page),
        ).filter((mode) => mode === "visible"),
      ).toHaveLength(1);
      for (const page of CONSOLE_PAGE_IDS) {
        expect(consolePageActivityMode(activePage, page)).toBe(
          page === activePage ? "visible" : "hidden",
        );
      }
    }
  });

  it("opens Settings and cycles through the sidebar order", () => {
    expect(DEFAULT_CONSOLE_PAGE).toBe("settings");
    expect(adjacentConsolePage("history", -1)).toBe("diagnostics");
    expect(adjacentConsolePage("skills", 1)).toBe("character-data");
    expect(adjacentConsolePage("character-data", 1)).toBe("empty-curtain");
    expect(adjacentConsolePage("empty-curtain", 1)).toBe("mod-studio");
    expect(adjacentConsolePage("mod-studio", 1)).toBe("settings");
    expect(adjacentConsolePage("settings", 1)).toBe("shortcuts");
  });

  it("persists a validated and user-editable favorites list", () => {
    const values = stubLocalStorage();

    expect(readConsoleFavoritePages()).toEqual(["settings", "history"]);
    writeConsoleFavoritePages(["shortcuts"]);
    expect(readConsoleFavoritePages()).toEqual(["shortcuts"]);

    const key = [...values.keys()][0]!;
    values.set(
      key,
      JSON.stringify(["history", "missing", "history", "packets"]),
    );
    expect(readConsoleFavoritePages()).toEqual(["history", "packets"]);
    values.set(key, "not-json");
    expect(readConsoleFavoritePages()).toEqual(["settings", "history"]);
  });

  it("persists the explicit sidebar preference and falls back when storage is absent", () => {
    expect(readConsoleSidebarCollapsed()).toBe(false);
    stubLocalStorage();

    expect(readConsoleSidebarCollapsed()).toBe(false);
    writeConsoleSidebarCollapsed(true);
    expect(readConsoleSidebarCollapsed()).toBe(true);
    writeConsoleSidebarCollapsed(false);
    expect(readConsoleSidebarCollapsed()).toBe(false);
  });

  it("maps the legacy Console keyboard shortcuts and protects form controls", () => {
    const shortcut = (key: string, editable = false) =>
      resolveConsoleShortcut({
        key,
        ctrlKey: true,
        altKey: false,
        shiftKey: false,
        repeat: false,
        editable,
      });

    expect(shortcut("PageUp")).toBe("previous-page");
    expect(shortcut("PageDown")).toBe("next-page");
    expect(shortcut("PageUp", true)).toBeNull();
    expect(shortcut("k", true)).toBe("command-palette");
    expect(
      resolveConsoleShortcut({
        key: "PageDown",
        ctrlKey: true,
        altKey: false,
        shiftKey: true,
        repeat: false,
        editable: false,
      }),
    ).toBeNull();
  });
});
