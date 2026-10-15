import { describe, expect, it } from "vitest";
import { screenHomeStageLabel, screenHomeStagesSummary, screenHomeSummary, type ScreenHomeEvidence } from "./screenHome";

const component = "chat.xiaoxin.app/chat.xiaoxin.app.MainActivity";
function evidence(): ScreenHomeEvidence {
  return { component, targetEnabled: true, save: "applied", readback: "succeeded", after: { protocolVersion: 1, packageName: "chat.xiaoxin.app", deviceOwner: true, persistentPreferred: true, resolvedHome: component }, activation: "activated", runtimeReadback: "succeeded", runtimeAfter: { homeTask: true, resumed: true, pid: 12098, taskId: 23, component } };
}

describe("固定主屏结果说明", () => {
  it("策略正确且激活动作已触发仍须实际HOME任务、进程与前台核验", () => {
    expect(screenHomeSummary(evidence())).toBe("固定小新为主屏 · 已确认");
    for (const runtime of [
      { homeTask: false }, { resumed: false }, { pid: null }, { pid: 0 }, { taskId: null }, { component: "android/com.android.internal.app.ResolverActivity" },
    ]) {
      const value = evidence(); Object.assign(value.runtimeAfter!, runtime);
      expect(screenHomeSummary(value)).toBe("固定小新为主屏 · 尚未确认");
    }
    const value = evidence(); value.runtimeReadback = "unknown";
    expect(screenHomeStagesSummary(value)).toContain("激活主屏：已触发 · 运行核验：待核实");
    expect(screenHomeSummary(value)).not.toContain(" · 已确认");
    value.runtimeReadback = "succeeded"; value.activation = "pending";
    expect(screenHomeSummary(value)).not.toContain(" · 已确认");
  });
  it("旧历史仅有持久规则证据时不冒充主屏已激活", () => {
    const value = evidence(); delete value.activation; delete value.runtimeReadback; delete value.runtimeAfter;
    expect(screenHomeSummary(value)).toBe("固定小新为主屏 · 规则已保存，激活未核验");
    expect(screenHomeStagesSummary(value)).toContain("激活主屏：未记录 · 运行核验：未记录");
  });
  it.each(["deviceOwner", "persistentPreferred", "resolvedHome"] as const)("回读缺少 %s 条件时不显示固定成功", field => {
    const value = evidence();
    if (field === "resolvedHome") value.after!.resolvedHome = "android/com.android.internal.app.ResolverActivity";
    else value.after![field] = false;
    expect(screenHomeSummary(value)).toContain("固定小新为主屏 · 尚未确认");
  });
  it("无回读保留未知，提交和已存在均不等同回读确认", () => {
    const value = evidence(); delete value.after;
    expect(screenHomeSummary(value)).toContain("固定小新为主屏 · 尚未确认");
    expect(screenHomeSummary()).toBe("未取得主屏状态");
    expect(screenHomeStageLabel("save", "applied")).toBe("已提交");
    expect(screenHomeStageLabel("save", "unchanged")).toBe("无需修改");
    expect(screenHomeStageLabel("readback", "succeeded")).toBe("已确认");
  });
  it("解除仅核实持久规则已撤销，普通主屏仍是小新也可完成", () => {
    const value = evidence(); value.targetEnabled = false;
    expect(screenHomeSummary(value)).toBe("解除主屏固定 · 尚未确认");
    value.after!.persistentPreferred = false;
    expect(screenHomeSummary(value)).toBe("解除主屏固定 · 已确认");
    value.after!.resolvedHome = "other.app/other.app.Home";
    expect(screenHomeSummary(value)).toBe("解除主屏固定 · 已确认");
  });
});
