import { describe, expect, it } from "vitest";
import { reactive } from "vue";
import { screenActions, screenActionLabel, type ScreenTask, type ScreenTaskTarget } from "./screen";
import { screenSettingsDefaultPatch, screenSettingsEmptyPatch, screenSettingsFields, screenSettingsRetryPatch, screenSettingsView,
  screenSettingsRebootLabel, type ScreenSettingsEvidence, type ScreenSettingsItem, type ScreenSettingsPatch, type ScreenSettingsRead } from "./screenSettings";

const item = (key: string): ScreenSettingsItem => ({ state: key === "ntp" ? "not_selected" : "ready", current: key === "ntp" ? { server: "old.internal", autoTime: true, activationConfirmed: true, syncConfirmed: true } : {}, target: true,
  currentDisplay: "当前未满足", targetDisplay: "目标已满足", reason: "需处理", rebootRequired: ["adb", "ntp"].includes(key) });
const row = (): ScreenSettingsRead => ({ screenId: "screen", readAt: "2026-11-03", pendingCount: 3, blockedCount: 0,
  items: { adb: item("adb"), time: item("time"), sleepOff: item("sleepOff"), ntp: item("ntp") } });
const combinations = Array.from({ length: 16 }, (_, mask) => ({ mask, patch: { adb: !!(mask & 1), time: !!(mask & 2), sleepOff: !!(mask & 4), ntp: mask & 8 ? { server: "new.internal" } : null } satisfies ScreenSettingsPatch }));

describe("智能屏组合设置的目标与历史投影", () => {
  it.each(combinations)("组合$mask只统计明确选中的实际待处理项目", ({ patch }) => {
    const view = screenSettingsView(row(), patch);
    expect(view.pendingCount).toBe(Number(patch.adb) + Number(patch.time) + Number(patch.sleepOff) + Number(patch.ntp !== null));
    expect(view.blockedCount).toBe(0);
    for (const { key } of screenSettingsFields) expect(view.items[key].state).toBe((key === "ntp" ? patch.ntp !== null : patch[key]) ? "ready" : "not_selected");
  });
  it("NTP未勾选不计数，同值须同步已确认才满足，勾选且空server明确清除", () => {
    const before = row();
    expect(screenSettingsView(before, screenSettingsDefaultPatch()).items.ntp).toMatchObject({ state: "not_selected", targetDisplay: "保留原设置", reason: "未勾选NTP，保留原设置" });
    expect(screenSettingsView(before, { ...screenSettingsEmptyPatch(), ntp: { server: "old.internal" } }).items.ntp.state).toBe("skip");
    (before.items.ntp.current as Record<string, unknown>).syncConfirmed = false;
    expect(screenSettingsView(before, { ...screenSettingsEmptyPatch(), ntp: { server: "old.internal" } }).items.ntp.state).toBe("ready");
    const restore = screenSettingsView(before, { ...screenSettingsEmptyPatch(), ntp: { server: "" } });
    expect(restore.items.ntp.state).toBe("ready"); expect(restore.items.ntp.targetDisplay).toBe("清除自定义地址（固件默认）");
  });
  it("不可执行单独计数，未选后不阻断其他项目，未知不当作满足", () => {
    const before = row(); before.items.adb.state = "blocked"; before.items.time.state = "unknown";
    expect(screenSettingsView(before, screenSettingsDefaultPatch())).toMatchObject({ pendingCount: 1, blockedCount: 2 });
    expect(screenSettingsView(before, { ...screenSettingsDefaultPatch(), adb: false, time: false })).toMatchObject({ pendingCount: 1, blockedCount: 0 });
  });
  it("主菜单只保留设置智能屏，旧独立动作仍能正确显示历史名称", () => {
    expect(screenActions.filter(action => ["time", "ntp", "adb", "sleep_off"].includes(action.value))).toHaveLength(0);
    expect(screenActions.filter(action => action.value === "screen_settings")).toHaveLength(1);
    expect(screenActions.some(action => action.value === "app_config")).toBe(true);
    expect(["time", "ntp", "adb", "sleep_off"].map(action => screenActionLabel(action as ScreenTask["action"]))).toEqual(["校准时间", "设置NTP服务器", "保持ADB端口", "关闭屏睡眠"]);
  });
  it("重试只带原失败或未开始子项，未知结果要求先核实", () => {
    const evidence: ScreenSettingsEvidence = { patch: { ...screenSettingsDefaultPatch(), ntp: { server: "original.internal" } },
      items: { adb: { state: "succeeded", message: "完成" }, ntp: { state: "skipped", message: "相同" }, time: { state: "failed", message: "失败" }, sleepOff: { state: "not_started", message: "未开始" } }, reboot: { requested: false } };
    const target: ScreenTaskTarget = { screenId: "screen", name: "屏", ip: "192.0.2.1", state: "failed", progress: 30, message: "停止", result: { device: "failed", business: "not_required", shared: "not_required", evidence: { screenSettings: evidence } } };
    const task: ScreenTask = { id: "task", projectId: "project", action: "screen_settings", state: "failed", createdAt: "2026-11-03", updatedAt: "2026-11-03", targets: [target], logs: [], settings: { screen: evidence.patch } };
    expect(screenSettingsRetryPatch(reactive(task), reactive(target))).toEqual({ adb: false, time: true, sleepOff: true, ntp: null });
    evidence.items.time.state = "unknown"; expect(screenSettingsRetryPatch(task, target)).toBeNull();
  });
  it("旧单项任务重试不带入默认三项，未开始组合任务使用冻结原patch", () => {
    const target = { screenId: "screen", state: "cancelled" } as ScreenTaskTarget;
    expect(screenSettingsRetryPatch({ action: "time" } as ScreenTask, target)).toEqual({ ...screenSettingsEmptyPatch(), time: true });
    const patch = { ...screenSettingsEmptyPatch(), sleepOff: true };
    expect(screenSettingsRetryPatch({ action: "screen_settings", settings: { screen: patch } } as unknown as ScreenTask, target)).toEqual(patch);
  });
  it("需要但未发送的共享重启不能显示无需执行", () => {
    const evidence = { reboot: { requested: false, required: true, reasons: ["adb"] } } as ScreenSettingsEvidence;
    expect(screenSettingsRebootLabel(evidence)).toContain("未执行");
    evidence.reboot.required = false; evidence.reboot.reasons = []; expect(screenSettingsRebootLabel(evidence)).toContain("无需执行");
    evidence.reboot = { requested: true, required: true, beforeBootId: "before", afterBootId: "after" };
    expect(screenSettingsRebootLabel(evidence)).toContain("已确认（一次）");
  });
});
