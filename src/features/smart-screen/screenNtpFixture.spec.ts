import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import type { ScreenOperationInput } from "@/shared/model/screen";

const project = "ntp-fixture-result", target = "platform-screen-2", server = "ntp.internal";
let adapter: FixtureScreenAdapter;
beforeEach(() => { localStorage.clear(); vi.useFakeTimers(); adapter = new FixtureScreenAdapter(localStorage, 10); });
afterEach(() => { adapter.dispose(); vi.useRealTimers(); });
async function execute() {
  const snapshot = await adapter.load(project), screen = snapshot.screens.find(screen => screen.id === target)!;
  const input: ScreenOperationInput = { action: "ntp", targetIds: [target], appVersion: "", abi: "universal", reinstall: false, concurrency: 1, expectedTargets: { [target]: screenMaintenanceFingerprint(screen) } };
  expect((await adapter.preflightNtp(project, input, { [target]: { server } }))[0]?.state).toBe("ready");
  const id = await adapter.execute(project, input); await vi.advanceTimersByTimeAsync(100);
  return { id, task: (await adapter.load(project)).tasks.find(task => task.id === id)! };
}

describe("NTP演示结果与核实边界", () => {
  it("保存和生效确定、授时未知时任务失败并释放占用，恢复后只创建同值验证新任务", async () => {
    await adapter.setScenario(project, "ntp_sync_unconfirmed");
    const original = await execute();
    expect(original.task.state).toBe("failed"); expect(original.task.targets[0]!.state).toBe("failed");
    expect(original.task.targets[0]!.result!.evidence!.ntp).toMatchObject({ save: "succeeded", activation: "succeeded", sync: "unknown" });
    expect((await adapter.readNtp(project, [target]))[0]!.config?.server).toBe(server);
    await expect(adapter.verify(project, original.id)).rejects.toThrow("没有待核实结果");
    await adapter.setScenario(project, "normal");
    const fresh = await execute();
    expect(fresh.id).not.toBe(original.id); expect(fresh.task.state).toBe("succeeded");
    expect(fresh.task.targets[0]!.result!.evidence!.ntp).toMatchObject({ save: "unchanged", activation: "succeeded", sync: "succeeded", rebootRequired: false });
    expect((await adapter.load(project)).tasks.find(task => task.id === original.id)!.state).toBe("failed");
  });
  it("重启结果未知才保留待核实，原任务只读核实能够确认既有结果", async () => {
    await adapter.setScenario(project, "needs_review");
    const original = await execute();
    expect(original.task.state).toBe("needs_review");
    expect(original.task.targets[0]!.result!.evidence!.ntp).toMatchObject({ save: "succeeded", activation: "unknown", sync: "unknown" });
    await adapter.setScenario(project, "normal"); await adapter.verify(project, original.id);
    const task = (await adapter.load(project)).tasks.find(task => task.id === original.id)!;
    expect(task.state).toBe("succeeded"); expect(task.targets[0]!.message).toContain("未重复修改或重启");
    expect((await adapter.load(project)).tasks).toHaveLength(1);
  });
  it("只读核实确认保存和生效后，授时仍未知就结束为失败而不继续占用", async () => {
    await adapter.setScenario(project, "needs_review"); const original = await execute();
    await adapter.setScenario(project, "ntp_sync_unconfirmed"); await adapter.verify(project, original.id);
    const task = (await adapter.load(project)).tasks.find(task => task.id === original.id)!;
    expect(task.state).toBe("failed"); expect(task.targets[0]!.result!.evidence!.ntp).toMatchObject({ save: "succeeded", activation: "succeeded", sync: "unknown" });
    await expect(adapter.verify(project, original.id)).rejects.toThrow("没有待核实结果");
    await adapter.setScenario(project, "normal"); expect((await execute()).task.state).toBe("succeeded");
  });
});
