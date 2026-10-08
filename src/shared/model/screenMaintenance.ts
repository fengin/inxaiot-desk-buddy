import { normalizeScreenMac } from "./screen";
import type { ScreenAction, SmartScreen } from "./screen";
import { screenPlatformDraftValues } from "./screenRegistration";
import type { ScreenPlatformDraft } from "./screenRegistration";

export const isScreenReadOnlyAction = (action: ScreenAction) => ["ping", "inspect", "mac", "diagnostics"].includes(action);

export function screenCriticalDraftFields(screen: SmartScreen, draft?: ScreenPlatformDraft): string[] {
  if (screen.source !== "platform" || !draft) return [];
  const next = screenPlatformDraftValues(screen, draft);
  return [next.ip !== screen.ip ? "IP" : "", normalizeScreenMac(next.mac) !== normalizeScreenMac(screen.mac) ? "MAC" : "", next.size !== screen.size ? "尺寸" : ""].filter(Boolean);
}

export function screenCriticalDraftWarning(screen: SmartScreen, draft?: ScreenPlatformDraft): string {
  const fields = screenCriticalDraftFields(screen, draft);
  return fields.length ? `${fields.join("、")}有待提交修改，请先核对更新平台或放弃修改；当前确认地址为 ${screen.ip}` : "";
}

export interface ScreenVersionPreviewItem {
  screenId: string;
  name: string;
  ip: string;
  platformVersion: string | null;
  deviceVersion: string | null;
  checkedAt: string;
  state: "ready" | "blocked" | "skip" | "local";
  reason: string;
}
export interface ScreenVersionPreview { id: string; projectId: string; createdAt: string; items: ScreenVersionPreviewItem[] }
export interface ScreenVersionSyncRecord { screenId: string; ip: string; before: string | null; after: string; fingerprint: string }

export const screenMaintenanceFingerprint = (screen: SmartScreen) => JSON.stringify([screen.id, screen.source, screen.ip, normalizeScreenMac(screen.mac), screen.size, screen.revision]);
