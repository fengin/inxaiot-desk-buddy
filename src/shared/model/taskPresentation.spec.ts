import { describe, expect, it } from "vitest";
import { taskStageLabel, canRetryTaskResult } from "@/app/taskPresentation";
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
});
