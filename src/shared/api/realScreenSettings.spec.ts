import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ScreenOperationInput } from "@/shared/model/screen";
import { screenSettingsDefaultPatch } from "@/shared/model/screenSettings";
const mock = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mock.invoke, isTauri: () => false }));
import { RealScreenAdapter } from "./realScreenAdapter";
const input: ScreenOperationInput = { action: "screen_settings", targetIds: ["screen"], appVersion: "", abi: "universal", reinstall: false, concurrency: 3 };
beforeEach(() => mock.invoke.mockReset());
describe("真实智能屏组合设置接口", () => {
  it("统一读取只调用只读接口，预检冻结patch并执行同一预览", async () => {
    const adapter = new RealScreenAdapter();
    mock.invoke.mockResolvedValueOnce([]);
    await adapter.readSettings("project", ["screen"]);
    expect(mock.invoke).toHaveBeenLastCalledWith("screen_settings_read", { localProjectId: "project", screenIds: ["screen"] });
    const patches = { screen: { ...screenSettingsDefaultPatch(), ntp: { server: "ntp.original.test" } } };
    mock.invoke.mockResolvedValueOnce({ id: "unified-preview", items: [] });
    await adapter.preflightSettings("project", input, patches);
    patches.screen.ntp.server = "changed.test";
    expect(mock.invoke.mock.calls[1]?.[1].patches.screen.ntp.server).toBe("ntp.original.test");
    mock.invoke.mockResolvedValueOnce("task");
    await expect(adapter.execute("project", input)).resolves.toBe("task");
    expect(mock.invoke).toHaveBeenLastCalledWith("screen_execute", { localProjectId: "project", preflightId: "unified-preview", input });
  });
  it("不同项目的组合预览隔离，不调用旧四项预检", async () => {
    const adapter = new RealScreenAdapter();
    const patches = { screen: screenSettingsDefaultPatch() };
    mock.invoke.mockResolvedValueOnce({ id: "a-preview", items: [] }).mockResolvedValueOnce({ id: "b-preview", items: [] });
    await adapter.preflightSettings("a", input, patches); await adapter.preflightSettings("b", input, patches);
    mock.invoke.mockResolvedValueOnce("a-task"); await adapter.execute("a", input);
    expect(mock.invoke).toHaveBeenLastCalledWith("screen_execute", { localProjectId: "a", preflightId: "a-preview", input });
    expect(mock.invoke.mock.calls.filter(call => String(call[0]).endsWith("_preflight")).map(call => call[0])).toEqual(["screen_settings_preflight", "screen_settings_preflight"]);
  });
});
