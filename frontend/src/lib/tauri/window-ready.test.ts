import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  label: "main-dps",
  invoke: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: tauri.label }),
}));

import { revealPrimaryWindowAfterFirstPaint } from "./window-ready";

describe("window readiness", () => {
  beforeEach(() => {
    tauri.label = "main-dps";
    tauri.invoke.mockClear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reveals the main DPS window after two painted frames", async () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) =>
      frames.push(callback),
    );
    const pending = revealPrimaryWindowAfterFirstPaint();

    frames.shift()?.(0);
    expect(tauri.invoke).not.toHaveBeenCalled();
    frames.shift()?.(16);
    await pending;
    expect(tauri.invoke).toHaveBeenCalledOnce();
    expect(tauri.invoke).toHaveBeenCalledWith("show_main_dps_when_ready");
  });

  it("ignores non-primary windows", async () => {
    tauri.label = "console";
    await revealPrimaryWindowAfterFirstPaint();
    expect(tauri.invoke).not.toHaveBeenCalled();
  });
});
