import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface ApplicationExitImpact {
  activeTaskCount: number;
  queuedTaskCount: number;
  runningTaskCount: number;
  waitTimeoutSeconds: number;
}

function isTauriRuntime() {
  return typeof window.__TAURI_INTERNALS__ !== "undefined";
}

export async function listenApplicationExitImpact(
  handler: (impact: ApplicationExitImpact) => void
) {
  if (!isTauriRuntime()) return () => undefined;
  return listen<ApplicationExitImpact>("application-exit-impact", (event) => {
    handler(event.payload);
  });
}

export async function confirmApplicationExit() {
  if (!isTauriRuntime()) return;
  await invoke("confirm_application_exit");
}
