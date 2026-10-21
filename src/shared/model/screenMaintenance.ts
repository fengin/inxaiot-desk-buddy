import { normalizeScreenMac } from "./screen";
import type { ScreenAction, SmartScreen } from "./screen";
import { screenPlatformDraftValues, screenPlatformFieldEqual } from "./screenRegistration";
import type { ScreenPlatformDraft } from "./screenRegistration";

export const isScreenReadOnlyAction = (action: ScreenAction) => ["ping", "inspect", "mac", "diagnostics"].includes(action);

/** 仅提示确认地址上的版本观测差异；同步时仍须重新读取，不能直接提交此缓存。 */
export function screenNeedsVersionSync(screen: SmartScreen): boolean {
  const observed = screen.observedAppVersion?.trim();
  return screen.source === "platform" && Boolean(screen.ip.trim()) && Boolean(observed) && screen.appInstalled !== false
    && screen.versionCheckedIp === screen.ip && observed !== (screen.appVersion?.trim() || "");
}

export function screenCriticalDraftFields(screen: SmartScreen, draft?: ScreenPlatformDraft): string[] {
  if (screen.source !== "platform" || !draft) return [];
  const next = screenPlatformDraftValues(screen, draft);
  return [!screenPlatformFieldEqual("ip", next.ip, screen.ip) ? "IP" : "", !screenPlatformFieldEqual("mac", next.mac, screen.mac) ? "MAC" : "", !screenPlatformFieldEqual("size", next.size, screen.size) ? "尺寸" : ""].filter(Boolean);
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
