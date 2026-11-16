import { beforeEach, describe, expect, it, vi } from "vitest";

const webview = vi.hoisted(() => ({
  label: "console",
  setBackgroundColor: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    label: webview.label,
    setBackgroundColor: webview.setBackgroundColor,
  }),
}));

import { syncConsoleWindowBackground } from "./console-window-background";

describe("Console window background", () => {
  beforeEach(() => {
    webview.label = "console";
    webview.setBackgroundColor.mockClear();
  });

  it("updates opaque desktop windows and skips transparent overlays", async () => {
    await syncConsoleWindowBackground(false);
    expect(webview.setBackgroundColor).toHaveBeenCalledWith([
      246, 247, 248, 255,
    ]);

    webview.setBackgroundColor.mockClear();
    webview.label = "main-dps";
    await syncConsoleWindowBackground(true);
    expect(webview.setBackgroundColor).toHaveBeenCalledWith([9, 9, 11, 255]);

    webview.setBackgroundColor.mockClear();
    webview.label = "character-details";
    await syncConsoleWindowBackground(true);
    expect(webview.setBackgroundColor).toHaveBeenCalledWith([9, 9, 11, 255]);

    webview.setBackgroundColor.mockClear();
    webview.label = "hud-spike";
    await syncConsoleWindowBackground(false);
    expect(webview.setBackgroundColor).not.toHaveBeenCalled();
  });
});
