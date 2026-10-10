import { beforeEach, describe, expect, it, vi } from "vitest";
import { RealScreenAdapter, screenSnapshotFromBackend } from "./realScreenAdapter";
import type { ScreenOperationInput } from "@/shared/model/screen";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

function snapshot() {
  return {
    screens: [{ id: "platform-1", source: "platform" as const, name: "屏", ip: "192.0.2.1", mac: "", size: "4" as const, spaceId: "child", location: "门口", revision: 1, appVersion: "2.0.8", platformStatus: "online" as const, aliases: ["local-1"] }],
    spaces: [{ id: "root", name: "楼幢", kind: "building" as const }, { id: "child", parentId: "root", name: "楼层", kind: "floor" as const }],
    observations: { "local-1": [{ id: "check", observedIp: "192.0.2.1", observedAt: "2026-09-29T01:00:00Z", adbAvailable: true, abis: ["arm64-v8a"], android: "10", observedAppVersion: "2.0.9", appVersionCode: 209, appInstalled: true, errors: [] }] },
    ignoredPairs: [], platformAvailable: true
  };
}

describe("智能屏正式调用", () => {
  beforeEach(() => { invoke.mockReset(); });
  it("NTP先受控读取，再绑定逐屏预检快照执行", async () => {
    const adapter = new RealScreenAdapter();
    invoke.mockResolvedValueOnce([{ screenId: "platform-1", readAt: "2026-10-10T00:00:00Z", config: { server: "", autoTime: false, autoTimeZone: true, timeZone: "Asia/Shanghai" }, message: "已读取" }]);
    await adapter.readNtp("project", ["platform-1"]);
    expect(invoke).toHaveBeenLastCalledWith("screen_ntp_read", { localProjectId: "project", screenIds: ["platform-1"] });
    const input: ScreenOperationInput = { action: "ntp", targetIds: ["platform-1"], appVersion: "", abi: "universal", reinstall: false, concurrency: 1 };
    const patches = { "platform-1": { server: "192.168.3.142" } };
    invoke.mockResolvedValueOnce({ id: "ntp-check", items: [{ screenId: "platform-1", name: "屏", ip: "192.0.2.1", state: "ready", reason: "通过" }] });
    await adapter.preflightNtp("project", input, patches); patches["platform-1"].server = "changed.internal";
    expect(invoke).toHaveBeenLastCalledWith("screen_ntp_preflight", { localProjectId: "project", input, patches: { "platform-1": { server: "192.168.3.142" } } });
    invoke.mockResolvedValueOnce("ntp-task");
    await expect(adapter.execute("project", input)).resolves.toBe("ntp-task");
    expect(invoke).toHaveBeenLastCalledWith("screen_execute", { localProjectId: "project", preflightId: "ntp-check", input });
  });
  it("分别展示平台与设备版本，并保留注册前的实测", () => {
    const value = screenSnapshotFromBackend(snapshot());
    expect(value.screens[0]).toMatchObject({ appVersion: "2.0.8", observedAppVersion: "2.0.9", appVersionCode: 209, spacePath: "楼幢/楼层", building: "楼幢", floor: "楼层" });
    expect(value.screens[0]).not.toHaveProperty("version");
  });
  it("地址变化后不把旧地址的检查结果当作当前结果", () => {
    const raw = snapshot(); raw.screens[0]!.ip = "192.0.2.2";
    expect(screenSnapshotFromBackend(raw).screens[0]).toMatchObject({ observedAppVersion: null, adbAvailable: null, adbStatus: "uninspected", android: "尚未检查", ping: null });
  });
  it("未读取的数值和状态保留未知，不用0和失败代替", () => {
    const raw = snapshot(); raw.observations = { "local-1": [] };
    expect(screenSnapshotFromBackend(raw).screens[0]).toMatchObject({ adbAvailable: null, persistentAdb: null, freeSpaceMb: null, clockOffsetSeconds: null, appInstalled: null, appRunning: null });
  });
  it("最新Ping不覆盖先前的ADB检查结果", () => {
    const raw = snapshot();
    const previous = raw.observations['local-1'][0];
    const withPing = { ...raw, observations: { 'local-1': [previous, { id: 'ping', operationType: 'ping', observedIp: '192.0.2.1', observedAt: '2026-09-29T02:00:00Z', ping: 'online' as const, adbAvailable: false, abis: [], errors: [] }] } };
    expect(screenSnapshotFromBackend(withPing).screens[0]).toMatchObject({ adbAvailable: true, adbStatus: 'available', adbCheckedAt: previous.observedAt, ping: 'online', checkedAt: '2026-09-29T02:00:00Z' });
  });
  it("正式接口失败时返回错误，不回退模拟资产", async () => {
    invoke.mockRejectedValue({ params: { summary: "项目不存在" } });
    await expect(new RealScreenAdapter().load("missing")).rejects.toThrow("项目不存在");
    expect(invoke).toHaveBeenCalledWith("screen_load", { localProjectId: "missing", refresh: true });
  });
  it("本机编辑提交当前记录编号和修改次数", async () => {
    const adapter = new RealScreenAdapter(); invoke.mockResolvedValueOnce(snapshot()).mockResolvedValueOnce(undefined);
    await adapter.load("project");
    await adapter.saveLocal("project", { name: "屏", ip: "192.0.2.1", mac: "", size: "4", location: "门口" }, "platform-1");
    expect(invoke).toHaveBeenLastCalledWith("screen_save_local", expect.objectContaining({ localProjectId: "project", id: "platform-1", expectedRevision: 1 }));
  });
  it("登录后强制读取平台，不被此前的本机更新标记改为缓存读取，并消费旧标记", async () => {
    const adapter = new RealScreenAdapter();
    invoke.mockResolvedValueOnce({ ...snapshot(), platformAvailable: false, platformMessage: "平台会话缺失" })
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(snapshot())
      .mockResolvedValueOnce(snapshot());
    await adapter.load("project");
    await adapter.saveLocal("project", { name: "屏", ip: "192.0.2.1", mac: "", size: "4", location: "门口" });

    const refreshed = await adapter.load("project", { refreshPlatform: true });
    expect(invoke).toHaveBeenLastCalledWith("screen_load", { localProjectId: "project", refresh: true });
    expect(refreshed.platformAvailable).toBe(true);
    expect(refreshed.platformMessage).toBeUndefined();

    await adapter.load("project");
    expect(invoke).toHaveBeenLastCalledWith("screen_load", { localProjectId: "project", refresh: true });
  });
  it("普通任务或本机资料更新仍只读取本机结果，并保留已知的平台可用状态", async () => {
    const adapter = new RealScreenAdapter();
    invoke.mockResolvedValueOnce(snapshot())
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce({ ...snapshot(), platformAvailable: false });
    await adapter.load("project");
    await adapter.saveLocal("project", { name: "屏", ip: "192.0.2.1", mac: "", size: "4", location: "门口" });

    const refreshed = await adapter.load("project", { refreshPlatform: false });
    expect(invoke).toHaveBeenLastCalledWith("screen_load", { localProjectId: "project", refresh: false });
    expect(refreshed.platformAvailable).toBe(true);
  });
});
