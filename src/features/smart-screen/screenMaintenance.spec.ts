import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FixtureScreenAdapter, SCREEN_STORAGE_PREFIX } from "@/dev-fixtures/screenFixtureAdapter";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import type { ScreenOperationInput, ScreenSnapshot } from "@/shared/model/screen";

const project = "maintenance-review";
let adapter: FixtureScreenAdapter;
beforeEach(() => { localStorage.clear(); vi.useFakeTimers(); adapter = new FixtureScreenAdapter(localStorage, 10); });
afterEach(() => { adapter.dispose(); vi.useRealTimers(); });
const input = (action: ScreenOperationInput["action"], id = "platform-screen-2"): ScreenOperationInput => ({ action, targetIds: [id], appVersion: "", abi: "universal", reinstall: false, concurrency: 1 });
const screen = (snapshot: ScreenSnapshot, id = "platform-screen-2") => snapshot.screens.find((s) => s.id === id)!;
const liveState = () => (adapter as unknown as { snapshots: Map<string, ScreenSnapshot> }).snapshots.get(project)!;

describe("版本同步、离线检查与实际目标", () => {
  it("先安装的未注册屏登记后可独立读取版本、同步平台且不重复安装", async () => {
    const seed = createScreenSnapshot();
    screen(seed, "local-screen-4").appVersion = "1.6.2";
    localStorage.setItem(SCREEN_STORAGE_PREFIX + project, JSON.stringify({ schemaVersion: 3, ...seed }));
    const registration = await adapter.previewPlatformRegistration(project, ["local-screen-4"]);
    await adapter.submitPlatformRegistration(project, { previewId: registration.id, screenIds: ["local-screen-4"] });
    const registered = (await adapter.load(project)).screens.find((s) => s.aliases.includes("local-screen-4"))!;
    expect(registered.appVersion).toBeNull();
    const preview = await adapter.previewVersionSync(project, ["local-screen-4"]);
    expect(preview.items[0]).toMatchObject({ screenId: registered.id, deviceVersion: "1.6.2", platformVersion: null, state: "ready" });
    const id = await adapter.submitVersionSync(project, preview.id, [registered.id]);
    const after = await adapter.load(project);
    expect(screen(after, registered.id)).toMatchObject({ appVersion: "1.6.2", ip: registered.ip, mac: registered.mac, platformStatus: registered.platformStatus, spaceId: registered.spaceId });
    expect(after.tasks.find((task) => task.id === id)?.action).toBe("version_sync");
    expect(after.tasks.some((task) => task.action === "install")).toBe(false);
    expect(await adapter.submitVersionSync(project, preview.id, [registered.id])).toBe(id);
    adapter.dispose(); adapter = new FixtureScreenAdapter(localStorage, 10);
    expect(screen(await adapter.load(project), registered.id).appVersion).toBe("1.6.2");
  });

  it("重新读取独立设备状态，不把平台旧版本或本机旧观测当成本次读取", async () => {
    await adapter.load(project);
    screen(liveState()).appVersion = "0.9.0"; screen(liveState()).observedAppVersion = "99.0.0";
    const preview = await adapter.previewVersionSync(project, ["platform-screen-2"]);
    expect(preview.items[0]).toMatchObject({ platformVersion: "0.9.0", deviceVersion: "1.5.2", state: "ready" });
    await adapter.submitVersionSync(project, preview.id, ["platform-screen-2"]);
    expect(screen(await adapter.load(project)).appVersion).toBe("1.5.2");
    expect((await adapter.previewVersionSync(project, ["platform-screen-2"])).items[0]?.state).toBe("skip");
  });

  it("连接失败或未安装时不清空平台版本、不使用历史观测提交", async () => {
    await adapter.load(project);
    screen(liveState()).adbAvailable = false;
    const failed = await adapter.previewVersionSync(project, ["platform-screen-2"]);
    expect(failed.items[0]).toMatchObject({ state: "blocked", deviceVersion: null });
    await expect(adapter.submitVersionSync(project, failed.id, ["platform-screen-2"])).rejects.toThrow("读取成功");
    const absent = await adapter.previewVersionSync(project, ["platform-screen-1"]);
    expect(absent.items[0]?.state).toBe("blocked");
    expect(screen(await adapter.load(project)).appVersion).toBe("1.5.2");
  });

  it("平台不可用时已注册屏四种只读操作可执行，写操作及版本同步仍阻断", async () => {
    const before = screen(await adapter.load(project));
    await adapter.setScenario(project, "platform_offline");
    for (const action of ["ping", "inspect", "mac", "diagnostics"] as const) {
      expect((await adapter.preflight(project, input(action)))[0]?.state).toBe("ready");
      await adapter.execute(project, input(action)); await vi.advanceTimersByTimeAsync(100);
    }
    expect((await adapter.preflight(project, input("reboot")))[0]?.state).toBe("blocked");
    const preview = await adapter.previewVersionSync(project, ["platform-screen-2"]);
    expect(preview.items[0]).toMatchObject({ deviceVersion: "1.5.2", state: "blocked" });
    await expect(adapter.submitVersionSync(project, preview.id, ["platform-screen-2"])).rejects.toThrow("平台不可用");
    const after = screen(await adapter.load(project));
    expect(after.appVersion).toBe(before.appVersion); expect(after.platformStatus).toBe(before.platformStatus);
    expect(after.versionCheckedIp).toBe(before.ip);
  });

  it.each(["ip", "mac", "size"] as const)("关键草稿%s阻断设备写入，只读使用当前确认地址", async (field) => {
    const current = screen(await adapter.load(project));
    const values = { ...screenPlatformFields(current), [field]: field === "ip" ? "192.0.2.210" : field === "mac" ? "02:AA:BB:CC:DD:EE" : "4" };
    await adapter.savePlatformDraft(project, current.id, values);
    expect((await adapter.preflight(project, input("time")))[0]?.reason).toContain("待提交修改");
    expect((await adapter.preflight(project, input("ping")))[0]).toMatchObject({ state: "ready", ip: current.ip });
    await expect(adapter.execute(project, input("time"))).rejects.toThrow("目标条件已变化");
    const appVersion = await adapter.previewVersionSync(project, [current.id]);
    expect(appVersion.items[0]).toMatchObject({ ip: current.ip, state: "blocked" });
    await adapter.discardPlatformDraft(project, current.id);
    expect((await adapter.preflight(project, input("time")))[0]?.state).toBe("ready");
  });

  it("名称位置草稿不阻断设备写操作，已确认地址变化使旧执行确认失效", async () => {
    const current = screen(await adapter.load(project));
    await adapter.savePlatformDraft(project, current.id, { ...screenPlatformFields(current), name: "新名字", location: "新位置" });
    expect((await adapter.preflight(project, input("time")))[0]?.state).toBe("ready");
    const prepared = { ...input("time"), expectedTargets: { [current.id]: screenMaintenanceFingerprint(current) } };
    screen(liveState()).ip = "192.0.2.211";
    await expect(adapter.execute(project, prepared)).rejects.toThrow("目标条件已变化");
  });

  it("过期预览、关键草稿、平台写拒绝都保留平台版本", async () => {
    await adapter.load(project); screen(liveState()).appVersion = "0.9.0";
    const preview = await adapter.previewVersionSync(project, ["platform-screen-2"]);
    screen(liveState()).appVersion = "1.4.0";
    const id = await adapter.submitVersionSync(project, preview.id, ["platform-screen-2"]);
    expect((await adapter.load(project)).tasks.find((t) => t.id === id)?.state).toBe("failed");
    expect(screen(await adapter.load(project)).appVersion).toBe("1.4.0");
    const again = await adapter.previewVersionSync(project, ["platform-screen-2"]);
    await adapter.setScenario(project, "write_denied");
    const denied = await adapter.submitVersionSync(project, again.id, ["platform-screen-2"]);
    expect((await adapter.load(project)).tasks.find((t) => t.id === denied)?.state).toBe("failed");
    expect(screen(await adapter.load(project)).appVersion).toBe("1.4.0");
    await expect(adapter.submitVersionSync("another-project", again.id, ["platform-screen-2"])).rejects.toThrow("失效");
  });

  it("回读中断重开后核实原同步，不重复安装或创建任务", async () => {
    await adapter.load(project); screen(liveState()).appVersion = "0.9.0";
    const preview = await adapter.previewVersionSync(project, ["platform-screen-2"]);
    await adapter.setScenario(project, "needs_review");
    const id = await adapter.submitVersionSync(project, preview.id, ["platform-screen-2"]);
    expect(screen(await adapter.load(project)).appVersion).toBe("0.9.0");
    adapter.dispose(); adapter = new FixtureScreenAdapter(localStorage, 10);
    await adapter.verify(project, id);
    const next = await adapter.load(project);
    expect(screen(next).appVersion).toBe("1.5.2");
    expect(next.tasks.find((t) => t.id === id)?.state).toBe("succeeded");
    expect(next.tasks).toHaveLength(1);
  });

  it("部分目标地址变化只拒绝该台，采集MAC冲突不写平台版本", async () => {
    await adapter.load(project);
    screen(liveState()).appVersion = "0.9.0"; screen(liveState(), "platform-screen-3").appVersion = "0.8.0";
    const preview = await adapter.previewVersionSync(project, ["platform-screen-2", "platform-screen-3"]);
    screen(liveState(), "platform-screen-3").ip = "192.0.2.222";
    const id = await adapter.submitVersionSync(project, preview.id, ["platform-screen-2", "platform-screen-3"]);
    const after = await adapter.load(project);
    expect(after.tasks.find((task) => task.id === id)?.state).toBe("partially_succeeded");
    expect(screen(after).appVersion).toBe("1.5.2"); expect(screen(after, "platform-screen-3").appVersion).toBe("0.8.0");
    screen(liveState()).mac = "02:11:22:33:44:55"; screen(liveState()).observedMac = "02:11:22:33:44:66";
    expect((await adapter.previewVersionSync(project, ["platform-screen-2"])).items[0]?.reason).toContain("MAC 不一致");
  });
});
