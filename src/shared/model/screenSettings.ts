import type { ScreenTask, ScreenTaskTarget } from "./screen";
import { ntpServerDisplay, validateNtpServer } from "./screenNtp";

export type ScreenSettingsKey = "adb" | "ntp" | "time" | "sleepOff";
export type ScreenSettingsState = "ready" | "skip" | "blocked" | "unknown" | "not_selected";
export interface ScreenSettingsPatch { adb: boolean; time: boolean; sleepOff: boolean; ntp: { server: string } | null }
export interface ScreenSettingsItem {
  state: ScreenSettingsState; current: unknown; target: unknown; currentDisplay: string; targetDisplay: string;
  reason: string; rebootRequired: boolean;
}
export interface ScreenSettingsRead {
  screenId: string; readAt: string; items: Record<ScreenSettingsKey, ScreenSettingsItem>;
  pendingCount: number; blockedCount: number; error?: string;
}
export interface ScreenSettingsStepResult {
  state: "succeeded" | "skipped" | "failed" | "unknown" | "not_started" | "not_selected" | "pending";
  message: string; before?: unknown; target?: unknown; after?: unknown; evidence?: unknown;
}
export interface ScreenSettingsEvidence {
  patch: ScreenSettingsPatch; items: Record<ScreenSettingsKey, ScreenSettingsStepResult>;
  reboot: { requested: boolean; required?: boolean; beforeBootId?: string; afterBootId?: string; reasons?: string[] };
}
export const screenSettingsFields: { key: ScreenSettingsKey; label: string }[] = [
  { key: "adb", label: "保持ADB端口" }, { key: "ntp", label: "NTP服务器" },
  { key: "time", label: "校准时间" }, { key: "sleepOff", label: "关闭屏睡眠" }
];
export const screenSettingsDefaultPatch = (): ScreenSettingsPatch => ({ adb: true, time: true, sleepOff: true, ntp: null });
export const screenSettingsEmptyPatch = (): ScreenSettingsPatch => ({ adb: false, time: false, sleepOff: false, ntp: null });
export const screenSettingsSelected = (patch: ScreenSettingsPatch, key: ScreenSettingsKey) => key === "ntp" ? patch.ntp !== null : patch[key];
export const screenSettingsStateLabel = (state: ScreenSettingsState) => ({ ready: "待处理", skip: "已满足", blocked: "不可执行", unknown: "未确认", not_selected: "未选" })[state];
export const screenSettingsStateClass = (state: ScreenSettingsState) => state === "ready" ? "screen-warning" : state === "skip" ? "screen-success" : ["blocked", "unknown"].includes(state) ? "screen-danger" : "screen-muted";
export function screenSettingsRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}
export function screenSettingsReadFailure(screenId: string, reason: string): ScreenSettingsRead {
  const item = (): ScreenSettingsItem => ({ state: "unknown", current: null, target: null, currentDisplay: "未读取", targetDisplay: "未确认", reason, rebootRequired: false });
  return { screenId, readAt: new Date().toISOString(), items: { adb: item(), ntp: item(), time: item(), sleepOff: item() }, pendingCount: 0, blockedCount: 3, error: reason };
}

/** 使用本次只读事实刷新表单意图；最终能力、写权限及计划仍由后端预检确认。 */
export function screenSettingsView(row: ScreenSettingsRead, patch: ScreenSettingsPatch): ScreenSettingsRead {
  const items = {} as Record<ScreenSettingsKey, ScreenSettingsItem>;
  for (const { key } of screenSettingsFields) {
    const before = row.items[key];
    if (!screenSettingsSelected(patch, key)) {
      items[key] = { ...before, state: "not_selected", target: null, targetDisplay: "保留原设置", reason: key === "ntp" ? "未勾选NTP，保留原设置" : "本次未选择", rebootRequired: false };
      continue;
    }
    if (key !== "ntp") { items[key] = { ...before }; continue; }
    const current = screenSettingsRecord(before.current), server = patch.ntp!.server;
    const error = validateNtpServer(server);
    const readable = typeof current.server === "string" && typeof current.autoTime === "boolean";
    const matched = readable && current.server === server && current.autoTime === true && current.syncConfirmed === true;
    const state: ScreenSettingsState = error ? "blocked" : ["blocked", "unknown"].includes(before.state) || !readable ? "unknown" : matched ? "skip" : "ready";
    items.ntp = { ...before, state, target: { server }, targetDisplay: server ? ntpServerDisplay(server) : "清除自定义地址（固件默认）", rebootRequired: state === "ready",
      reason: error || (state === "unknown" ? before.reason || "尚未确认当前NTP设置或能力" : matched ? "NTP目标及生效状态已满足，无需重复设置" : "将设置并确认NTP生效") };
  }
  return { ...row, items, pendingCount: Object.values(items).filter(item => item.state === "ready").length,
    blockedCount: Object.values(items).filter(item => ["blocked", "unknown"].includes(item.state)).length };
}
export function screenSettingsNeedsReboot(row: ScreenSettingsRead): boolean {
  return ["adb", "ntp"].some(key => row.items[key as ScreenSettingsKey].state === "ready" && row.items[key as ScreenSettingsKey].rebootRequired);
}
export function screenSettingsEvidence(target: ScreenTaskTarget): ScreenSettingsEvidence | undefined {
  const value = screenSettingsRecord(target.result?.evidence?.screenSettings);
  return value.patch && value.items ? value as unknown as ScreenSettingsEvidence : undefined;
}
export function screenSettingsUncertain(target: ScreenTaskTarget): boolean {
  const evidence = screenSettingsEvidence(target);
  return target.state === "needs_review" || target.result?.device === "unknown" || !!evidence && Object.values(evidence.items).some(item => item.state === "unknown");
}
export function screenSettingsConfirmed(target: ScreenTaskTarget): boolean {
  const evidence = screenSettingsEvidence(target);
  if (!evidence || screenSettingsFields.some(({ key }) => screenSettingsSelected(evidence.patch, key) && !["succeeded", "skipped"].includes(evidence.items[key]?.state))) return false;
  if (!evidence.reboot?.required && !evidence.reboot?.reasons?.length) return true;
  return evidence.reboot.requested === true && !!evidence.reboot.beforeBootId && !!evidence.reboot.afterBootId && evidence.reboot.beforeBootId !== evidence.reboot.afterBootId;
}
export function screenSettingsRetryPatch(task: ScreenTask, target: ScreenTaskTarget): ScreenSettingsPatch | null {
  if (screenSettingsUncertain(target)) return null;
  const evidence = screenSettingsEvidence(target);
  const original = task.settings?.[target.screenId] ?? evidence?.patch;
  if (task.action === "screen_settings") {
    if (!original) return null;
    const patch: ScreenSettingsPatch = { adb: original.adb, time: original.time, sleepOff: original.sleepOff, ntp: original.ntp ? { server: original.ntp.server } : null };
    for (const { key } of screenSettingsFields) if (["succeeded", "skipped"].includes(evidence?.items[key]?.state ?? "")) {
      if (key === "ntp") patch.ntp = null; else patch[key] = false;
    }
    return patch;
  }
  const patch = screenSettingsEmptyPatch();
  if (["succeeded", "skipped"].includes(target.result?.device ?? "")) return patch;
  if (task.action === "time") patch.time = true;
  if (task.action === "adb") patch.adb = true;
  if (task.action === "sleep_off") patch.sleepOff = true;
  if (task.action === "ntp") {
    const server = original?.ntp?.server ?? screenSettingsRecord(target.result?.evidence?.ntp).targetServer;
    if (typeof server === "string") patch.ntp = { server };
  }
  return patch;
}
export const screenSettingsResultLabel = (state: ScreenSettingsStepResult["state"]) => ({ succeeded: "已完成", skipped: "无需执行", failed: "未完成", unknown: "待核实", not_started: "未开始", not_selected: "未选", pending: "等待处理" })[state] ?? "待核实";
export const screenSettingsResultClass = (state: ScreenSettingsStepResult["state"]) => ["succeeded", "skipped"].includes(state) ? "screen-success" : ["failed", "unknown"].includes(state) ? "screen-danger" : state === "pending" ? "screen-warning" : "screen-muted";
export function screenSettingsRebootLabel(evidence: ScreenSettingsEvidence): string {
  if (!evidence.reboot?.requested) return evidence.reboot?.required || evidence.reboot?.reasons?.length ? "共享重启：未执行，后续已停止" : "共享重启：无需执行";
  return evidence.reboot.beforeBootId && evidence.reboot.afterBootId && evidence.reboot.beforeBootId !== evidence.reboot.afterBootId ? "共享重启：已确认（一次）" : "共享重启：待核实";
}
