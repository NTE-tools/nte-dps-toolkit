import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { MAIN_DPS_WINDOW_LABEL } from "@/lib/tauri/window-labels";

export async function revealPrimaryWindowAfterFirstPaint(): Promise<void> {
  if (getCurrentWindow().label !== MAIN_DPS_WINDOW_LABEL) return;
  await waitForAnimationFrames(2);
  await invoke("show_main_dps_when_ready");
}

function waitForAnimationFrames(remaining: number): Promise<void> {
  if (remaining === 0) return Promise.resolve();
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      void waitForAnimationFrames(remaining - 1).then(resolve);
    });
  });
}
