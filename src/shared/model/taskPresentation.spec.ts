import { describe, expect, it } from "vitest";
import { taskStageLabel, taskDisplayLabels, canRetryTaskResult } from "@/app/taskPresentation";
import { registerTaskPresentation } from "./taskPresentation";
import type { ActivityTask } from "./activity";

const task = (domainType: string): ActivityTask => ({
  id: "task", projectId: "project", domainType, operationType: "install", name: "测试任务",
  state: "running", stage: "install", progress: 50, targetCount: 1, completedCount: 0,
  updatedAt: "2026-09-29", cancellable: true
});

describe("公共任务的业务展示", () => {
  it("相同阶段代码按业务显示，未知业务不套用一体机文案", () => {
    expect(taskStageLabel(task("aio"))).toBe("安装发布包");
    expect(taskStageLabel(task("test-device"))).toBe("处理中");
    const unregister = registerTaskPresentation("test-device", {
      stages: { install: "安装测试应用" }, canRetryResult: () => true
    });
    try {
      expect(taskStageLabel(task("test-device"))).toBe("安装测试应用");
      expect(canRetryTaskResult({ ...task("test-device"), state: "finalizing_failed" })).toBe(true);
      expect(taskStageLabel({ ...task("test-device"), state: "failed" })).toBe("执行失败");
    } finally { unregister(); }
    expect(canRetryTaskResult({ ...task("test-device"), state: "finalizing_failed" })).toBe(false);
  });

  it("历史注册预检只表示检查完成，不显示注册成功或业务成功颜色", () => {
    const previous = { ...task("smart_screen"), operationType: "screen_registration_preview", name: "注册/更新到平台", state: "succeeded" as const, stage: "检查完成", progress: 100 };
    expect(taskDisplayLabels(previous)).toEqual({ name: "注册/更新检查", state: "检查完成", stage: "检查完成，尚未提交注册", tone: "info" });
    expect(previous.name).toBe("注册/更新到平台");
    expect(taskStageLabel(previous)).toBe("检查完成，尚未提交注册");
  });

  it.each([
    ["queued", "等待执行", "等待提交平台"],
    ["running", "执行中", "正在提交并确认平台写入"],
    ["succeeded", "注册/更新成功", "平台写入已确认"],
    ["partially_succeeded", "部分注册/更新成功", "部分目标已写入平台，请核对其余结果"],
    ["finalizing_failed", "结果待核实", "平台写入或操作记录待核实，请勿重复提交"],
    ["failed", "注册/更新未完成", "请核对逐屏结果，重新检查后再提交"]
  ] as const)("正式注册任务%s区分提交、已确认写入与未完成结果", (state, label, stage) => {
    const value = { ...task("smart_screen"), operationType: "register", state };
    expect(taskDisplayLabels(value)).toMatchObject({ state: label, stage });
  });
});
