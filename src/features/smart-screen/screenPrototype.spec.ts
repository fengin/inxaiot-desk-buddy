import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { FixtureScreenAdapter, SCREEN_STORAGE_PREFIX } from "@/dev-fixtures/screenFixtureAdapter";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { isScreenLocationUncertain } from "@/shared/model/screenSpace";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { createScreenMergeChoices, effectiveScreenMac, screenMergeCandidates } from "@/shared/model/screen";
import type { LocalScreenInput, ScreenOperationInput } from "@/shared/model/screen";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { parseScreenInventory } from "./importInventory";

let adapter: FixtureScreenAdapter;
const project = "screen-prototype-test";
const input = (targetIds: string[], action: ScreenOperationInput["action"] = "ping"): ScreenOperationInput => ({ action, targetIds, appVersion: "1.6.0", abi: "universal", reinstall: false, concurrency: 1, ...(action === "install" ? { applicationId: "xiaoxin" as const, apk: { name: "xiaoxin-test.apk", size: 1024, lastModified: 0 } } : {}) });
const local = (ip: string): LocalScreenInput => ({ name: "新增屏", ip, size: "10", mac: "", building: "A座", floor: "1F", location: "入口" });
const mergeDecision = (source: "local" | "platform" = "platform", identityConfirmed = false) => ({ kind: "merge" as const, choices: createScreenMergeChoices(source), identityConfirmed });

beforeEach(() => { vi.useFakeTimers(); localStorage.clear(); adapter = new FixtureScreenAdapter(localStorage, 20); configureScreenAdapter(adapter); setActivePinia(createPinia()); });
afterEach(() => { adapter.dispose(); vi.useRealTimers(); });

describe("智能屏原型数据与操作边界", () => {
  it("本机资产跨加载保留且按项目隔离，重置不触及其他数据", async () => {
    localStorage.setItem("real-workbench-setting", "keep");
    await adapter.saveLocal(project, local("192.0.2.190"));
    expect((await new FixtureScreenAdapter(localStorage).load(project)).screens).toHaveLength(28);
    expect((await adapter.load("another-project")).screens).toHaveLength(27);
    await adapter.reset(project);
    expect((await adapter.load(project)).screens).toHaveLength(27);
    expect(localStorage.getItem("real-workbench-setting")).toBe("keep");
  });

  it("旧两级缓存按节点ID迁移，失效ID和原始位置说明不被猜测替换", async () => {
    const snapshot = createScreenSnapshot();
    for (const screen of snapshot.screens) { delete screen.spaceId; delete screen.spacePath; }
    snapshot.spaces = snapshot.spaces.filter((node) => node.kind !== "area");
    snapshot.screens[1]!.floorId = undefined;
    snapshot.screens[2]!.floorId = "deleted-floor";
    localStorage.setItem(SCREEN_STORAGE_PREFIX + project, JSON.stringify({ ...snapshot, schemaVersion: 2 }));
    const migrated = await adapter.load(project);
    expect(migrated.screens[0]!.spaceId).toBe(snapshot.screens[0]!.floorId);
    expect(migrated.screens[1]!.spaceId).toBe(snapshot.screens[1]!.buildingId);
    expect(isScreenLocationUncertain(migrated.screens[1]!, migrated.spaces)).toBe(false);
    expect(migrated.screens[2]!.spaceId).toBe("deleted-floor");
    expect(isScreenLocationUncertain(migrated.screens[2]!, migrated.spaces)).toBe(true);
    expect(migrated.screens[2]!.spacePath).toBe("A座 / 3F");
    expect(migrated.screens.find((screen) => screen.id === "local-screen-5")!.spaceId).toBeNull();
  });

  it("新建可选楼幢或深层节点，目录不可用保留原关联而不允许新分配", async () => {
    await adapter.saveLocal(project, { ...local("192.0.2.180"), spaceId: "building-a" });
    let created = (await adapter.load(project)).screens.find((screen) => screen.ip === "192.0.2.180")!;
    expect(created).toMatchObject({ spaceId: "building-a", spacePath: "A座", building: "A座", floor: "" });
    await adapter.saveLocal(project, { ...created, spaceId: "area-b-2-room" }, created.id);
    created = (await adapter.load(project)).screens.find((screen) => screen.id === created.id)!;
    expect(created.spacePath).toBe("B座/2F/东区/会议区/会议室");
    await adapter.setScenario(project, "platform_offline");
    await adapter.saveLocal(project, { ...created, name: "离线更新名称", spacePath: "不能伪造快照" }, created.id);
    expect((await adapter.load(project)).screens.find((screen) => screen.id === created.id)).toMatchObject({ name: "离线更新名称", spacePath: created.spacePath, spaceId: created.spaceId });
    await expect(adapter.saveLocal(project, { ...created, spaceId: "floor-a-1" }, created.id)).rejects.toThrow("暂不可用");
    await expect(adapter.importLocal(project, [{ ...local("192.0.2.181"), spaceId: "building-a" }])).rejects.toThrow("暂不可用");
    await adapter.saveLocal(project, { ...created, spaceId: null }, created.id);
    expect((await adapter.load(project)).screens.find((screen) => screen.id === created.id)?.spaceId).toBeNull();
  });

  it("导入空间逐条重校验，任一失效时不部分保存，也不打断其他设备任务", async () => {
    const running = await adapter.execute(project, input(["platform-screen-2"]));
    await expect(adapter.importLocal(project, [{ ...local("192.0.2.180"), spaceId: null, spacePath: "A座 / 不存在的楼层" }])).rejects.toThrow("尚未关联有效节点");
    await expect(adapter.importLocal(project, [{ ...local("192.0.2.180"), spaceId: "area-a-1-room" }, { ...local("192.0.2.181"), spaceId: "deleted-space" }])).rejects.toThrow("空间关联无效");
    expect((await adapter.load(project)).screens).toHaveLength(27);
    await adapter.importLocal(project, [{ ...local("192.0.2.180"), spaceId: "area-a-1-room" }]);
    await vi.advanceTimersByTimeAsync(100);
    expect((await adapter.load(project)).tasks.find((task) => task.id === running)?.state).toBe("succeeded");
  });

  it("合并保留旧历史关联，MAC冲突须另行核实身份", async () => {
    const snapshot = await adapter.load(project);
    const candidates = screenMergeCandidates(snapshot);
    expect(candidates).toHaveLength(3);
    const candidate = candidates.find((item) => item.matchIp && !item.conflict)!;
    const taskId = await adapter.execute(project, input([candidate.local.id]));
    await expect(adapter.merge(project, candidate, mergeDecision())).rejects.toThrow("活动任务");
    await vi.advanceTimersByTimeAsync(100);
    const refreshed = screenMergeCandidates(await adapter.load(project)).find((item) => item.key === candidate.key)!;
    await adapter.merge(project, refreshed, mergeDecision());
    const merged = await adapter.load(project);
    expect(merged.screens).toHaveLength(26);
    expect(merged.screens.find((s) => s.id === candidate.platform.id)?.aliases).toContain(candidate.local.id);
    expect(merged.screens.find((s) => s.id === candidate.platform.id)?.appVersion).toBe(candidate.platform.appVersion);
    expect(merged.screens.find((s) => s.id === candidate.platform.id)?.observedAppVersion).toBe(candidate.local.appVersion);
    expect(merged.tasks.find((t) => t.id === taskId)?.targets[0].screenId).toBe(candidate.local.id);
    const conflict = screenMergeCandidates(merged).find((item) => item.conflict)!;
    await expect(adapter.merge(project, conflict, mergeDecision())).rejects.toThrow("MAC 冲突");
    await adapter.merge(project, conflict, { kind: "ignore" });
    const checks = await adapter.preflight(project, input([conflict.local.id], "reboot"));
    expect(checks[0].state).toBe("blocked");
  });

  it("按字段混合来源更新平台记录和本机持久化视图，保留两侧原始依据", async () => {
    const candidate = screenMergeCandidates(await adapter.load(project))[0]!;
    const decision = mergeDecision();
    decision.choices.name = "local"; decision.choices.mac = "local";
    decision.choices.location = "local"; decision.choices.appVersion = "local";
    const result = await adapter.merge(project, candidate, decision);
    const merged = (await adapter.load(project)).screens.find((screen) => screen.id === candidate.platform.id)!;
    expect(result?.fields).toMatchObject({ name: candidate.local.name, mac: effectiveScreenMac(candidate.local), location: candidate.local.location, appVersion: candidate.local.appVersion });
    expect(merged).toMatchObject(result!.fields);
    expect(merged.platformStatus).toBe(candidate.platform.platformStatus);
    expect(merged.aliases).toContain(candidate.local.id);
    const restored = await new FixtureScreenAdapter(localStorage).load(project);
    expect(restored.screens.find((screen) => screen.id === candidate.platform.id)).toEqual(merged);
    expect(restored.screens.some((screen) => screen.id === candidate.local.id)).toBe(false);
    expect(restored.mergeHistory?.[0]).toMatchObject({ beforeLocal: candidate.local, beforePlatform: candidate.platform, fields: result!.fields, choices: decision.choices });
  });

  it("全部采用本机可更新平台地址，但不沿用旧地址检查结果", async () => {
    const candidate = screenMergeCandidates(await adapter.load(project)).find((item) => item.matchMac && !item.matchIp)!;
    const result = await adapter.merge(project, candidate, mergeDecision("local"));
    const merged = (await adapter.load(project)).screens.find((screen) => screen.id === candidate.platform.id)!;
    expect(merged).toMatchObject(result!.fields);
    expect(merged.ip).toBe(candidate.local.ip);
    expect(merged.ping).toBeNull(); expect(merged.checkedAt).toBeNull();
    expect(effectiveScreenMac(merged)).toBe(result!.fields.mac);
  });

  it("核实MAC冲突后可选择本机MAC合并，历史采集信息有原始记录", async () => {
    const candidate = screenMergeCandidates(await adapter.load(project)).find((item) => item.conflict)!;
    await adapter.merge(project, candidate, mergeDecision("local", true));
    const next = await adapter.load(project);
    const merged = next.screens.find((screen) => screen.id === candidate.platform.id)!;
    expect(merged.mac).toBe(effectiveScreenMac(candidate.local));
    expect(effectiveScreenMac(merged)).toBe(merged.mac);
    expect(next.mergeHistory?.[0]?.beforePlatform.mac).toBe(candidate.platform.mac);
    expect(next.screens).toHaveLength(26);
  });

  it("合并空间整体采用深层节点，未分配空间可持久化为待定", async () => {
    let candidate = screenMergeCandidates(await adapter.load(project)).find((item) => item.matchMac && !item.matchIp)!;
    await expect(adapter.saveLocal(project, { ...candidate.local, spaceId: "missing-space" }, candidate.local.id)).rejects.toThrow("空间关联无效");
    await adapter.saveLocal(project, { ...candidate.local, spaceId: "area-a-1-room" }, candidate.local.id);
    candidate = screenMergeCandidates(await adapter.load(project)).find((item) => item.key === candidate.key)!;
    const decision = mergeDecision(); decision.choices.space = "local";
    await adapter.merge(project, candidate, decision);
    expect((await adapter.load(project)).screens.find((screen) => screen.id === candidate.platform.id)).toMatchObject({ spaceId: "area-a-1-room", spacePath: "A座/1F/东区/会议区/会议室", buildingId: "building-a", floorId: "floor-a-1" });
    candidate = screenMergeCandidates(await adapter.load(project)).find((item) => !item.conflict)!;
    await adapter.saveLocal(project, { ...candidate.local, spaceId: null }, candidate.local.id);
    candidate = screenMergeCandidates(await adapter.load(project)).find((item) => item.key === candidate.key)!;
    await adapter.merge(project, candidate, mergeDecision("local"));
    const restored = (await new FixtureScreenAdapter(localStorage).load(project)).screens.find((screen) => screen.id === candidate.platform.id)!;
    expect(restored.spaceId).toBeNull(); expect(restored.buildingId).toBeUndefined(); expect(restored.floorId).toBeUndefined();
  });

  it("来源字段变动或平台拒绝写入时不执行合并", async () => {
    const candidate = screenMergeCandidates(await adapter.load(project))[0]!;
    await adapter.saveLocal(project, { ...candidate.local, name: "最新登记名称" }, candidate.local.id);
    await expect(adapter.merge(project, candidate, mergeDecision())).rejects.toThrow("字段已变化");
    const refreshed = screenMergeCandidates(await adapter.load(project))[0]!;
    const before = await adapter.load(project);
    await adapter.setScenario(project, "write_denied");
    await expect(adapter.merge(project, refreshed, mergeDecision("local"))).rejects.toThrow("拒绝字段写入");
    expect(await adapter.load(project)).toEqual(before);
  });

  it("本机保存失败不会留下半合并的内存状态", async () => {
    const failingAdapter = new FixtureScreenAdapter({ getItem: (key: string) => localStorage.getItem(key), setItem: () => { throw new Error("本机存储已满"); } } as unknown as Storage);
    const before = await failingAdapter.load(project);
    const candidate = screenMergeCandidates(before)[0]!;
    try {
      await expect(failingAdapter.merge(project, candidate, mergeDecision("local"))).rejects.toThrow("本机存储已满");
      expect(await failingAdapter.load(project)).toEqual(before);
    } finally { failingAdapter.dispose(); }
  });

  it("检查排除不适用、重复目标、同版本与降级", async () => {
    const checks = await adapter.preflight(project, input(["platform-screen-1", "local-screen-1"], "ping"));
    expect(checks.map((row) => row.state)).toEqual(["ready", "blocked"]);
    expect((await adapter.preflight(project, input(["platform-screen-19"], "adb")))[0].state).toBe("blocked");
    expect((await adapter.preflight(project, { ...input(["platform-screen-2"], "install"), abi: "arm64-v8a" }))[0].reason).toContain("不兼容");
    expect((await adapter.preflight(project, input(["platform-screen-7"], "install")))[0].state).toBe("skip");
    expect((await adapter.preflight(project, { ...input(["platform-screen-2"], "install"), appVersion: "1.4.0" }))[0].state).toBe("blocked");
    expect((await adapter.preflight(project, input(["platform-screen-9"], "install")))[0].reason).toContain("空间不足");
  });

  it("安装检查必须选择智能小新及非空APK，其他文件不能作为安装包", async () => {
    const install = input(["platform-screen-2"], "install");
    await expect(adapter.preflight(project, { ...install, apk: undefined })).rejects.toThrow("选择本地 APK");
    await expect(adapter.preflight(project, { ...install, applicationId: undefined })).rejects.toThrow("仅支持智能小新");
    await expect(adapter.preflight(project, { ...install, apk: { name: "readme.txt", size: 100, lastModified: 0 } })).rejects.toThrow(".apk");
    await expect(adapter.preflight(project, { ...install, apk: { name: "empty.apk", size: 0, lastModified: 0 } })).rejects.toThrow("空文件");
    expect((await adapter.load(project)).tasks).toHaveLength(0);
  });

  it("安装预检和执行均拒绝混合尺寸，即使包为通用架构也不能绕过", async () => {
    const mixed = input(["platform-screen-2", "platform-screen-19"], "install");
    await expect(adapter.preflight(project, mixed)).rejects.toThrow("一批只能操作同规格尺寸屏");
    await expect(adapter.execute(project, mixed)).rejects.toThrow("一批只能操作同规格尺寸屏");
    expect((await adapter.load(project)).tasks).toHaveLength(0);
    const sameSize = await adapter.preflight(project, { ...input(["platform-screen-19", "platform-screen-20"], "install"), appVersion: "" });
    expect(sameSize.every((row) => row.state === "ready")).toBe(true);
    const ping = await adapter.preflight(project, input(mixed.targetIds, "ping"));
    expect(ping.every((row) => row.state === "ready")).toBe(true);
  });

  it("未知尺寸不能安装；检查后尺寸变化也必须重新核对", async () => {
    await adapter.saveLocal(project, { ...local("192.0.2.190"), size: "unknown" });
    const unknown = (await adapter.load(project)).screens.find((screen) => screen.ip === "192.0.2.190")!;
    await expect(adapter.preflight(project, input([unknown.id], "install"))).rejects.toThrow("请先确认所选屏尺寸");
    await adapter.saveLocal(project, { ...unknown, size: "10" }, unknown.id);
    const install = { ...input([unknown.id, "platform-screen-2"], "install"), appVersion: "" };
    expect((await adapter.preflight(project, install)).every((row) => row.state === "ready")).toBe(true);
    await adapter.saveLocal(project, { ...unknown, size: "4" }, unknown.id);
    await expect(adapter.execute(project, install)).rejects.toThrow("一批只能操作同规格尺寸屏");
    expect((await adapter.load(project)).tasks).toHaveLength(0);
  });

  it("本地APK未解析时只演示流程，不将空版本当作降级或写入新观测", async () => {
    const install = { ...input(["platform-screen-2"], "install"), appVersion: "", apk: { name: "智能小新-v9.9.9.apk", size: 100, lastModified: 0 } };
    const before = (await adapter.load(project)).screens.find((screen) => screen.id === "platform-screen-2")!;
    const checks = await adapter.preflight(project, install);
    expect(checks[0].state).toBe("ready");
    expect(checks[0].reason).toContain("未解析");
    const id = await adapter.execute(project, install);
    await vi.advanceTimersByTimeAsync(100);
    const after = await adapter.load(project);
    expect(after.screens.find((screen) => screen.id === before.id)).toEqual(before);
    const task = after.tasks.find((item) => item.id === id)!;
    expect(task.targets[0].message).toContain("版本记录未更新");
    expect(task.input?.appVersion).toBe("");
    expect(task.input?.apk).toEqual(install.apk);
  });

  it("未解析APK的待核实演示也不覆盖原版本或原检查时间", async () => {
    await adapter.setScenario(project, "needs_review");
    const before = (await adapter.load(project)).screens.find((screen) => screen.id === "platform-screen-2")!;
    const id = await adapter.execute(project, { ...input([before.id], "install"), appVersion: "" });
    await vi.advanceTimersByTimeAsync(100);
    await adapter.verify(project, id);
    const after = await adapter.load(project);
    expect(after.tasks.find((task) => task.id === id)?.state).toBe("succeeded");
    expect(after.screens.find((screen) => screen.id === before.id)).toEqual(before);
  });

  it("平台不可用仍可只读检查已注册屏，设备写操作继续遵守共享条件", async () => {
    await adapter.setScenario(project, "platform_offline");
    const checks = await adapter.preflight(project, input(["local-screen-4", "platform-screen-2"]));
    expect(checks.map((row) => row.state)).toEqual(["ready", "ready"]);
    expect((await adapter.preflight(project, input(["platform-screen-2"], "time")))[0]?.state).toBe("blocked");
    const id = await adapter.execute(project, input(["local-screen-4"], "time"));
    await vi.advanceTimersByTimeAsync(100);
    expect((await adapter.load(project)).tasks.find((t) => t.id === id)?.state).toBe("succeeded");
  });

  it("IP检查独立于ADB可用性，身份冲突仍允许只读采集", async () => {
    expect((await adapter.load(project)).screens.find((s) => s.id === "platform-screen-11")?.adbAvailable).toBe(false);
    await adapter.execute(project, input(["platform-screen-11"]));
    await vi.advanceTimersByTimeAsync(100);
    expect((await adapter.load(project)).screens.find((s) => s.id === "platform-screen-11")?.ping).toBe("online");
    expect((await adapter.preflight(project, input(["local-screen-3"], "mac")))[0].state).toBe("ready");
  });

  it("取消停止排队派发，保留已开始目标结果", async () => {
    const id = await adapter.execute(project, input(["platform-screen-1", "platform-screen-2", "platform-screen-3"]));
    await vi.advanceTimersByTimeAsync(20);
    await adapter.cancel(project, id);
    await vi.advanceTimersByTimeAsync(100);
    const task = (await adapter.load(project)).tasks.find((t) => t.id === id)!;
    expect(task.state).toBe("cancelled");
    expect(task.targets.map((t) => t.state)).toEqual(["succeeded", "cancelled", "cancelled"]);
  });

  it("部分失败逐台记录，待核实先回读而非重复执行", async () => {
    await adapter.setScenario(project, "partial_failure");
    const id = await adapter.execute(project, input(["platform-screen-1", "platform-screen-2"]));
    await vi.advanceTimersByTimeAsync(160);
    expect((await adapter.load(project)).tasks.find((t) => t.id === id)?.state).toBe("partially_succeeded");
    await adapter.setScenario(project, "needs_review");
    const reviewId = await adapter.execute(project, input(["local-screen-4"], "time"));
    await vi.advanceTimersByTimeAsync(100);
    expect((await adapter.load(project)).tasks.find((t) => t.id === reviewId)?.state).toBe("needs_review");
    await adapter.verify(project, reviewId);
    const final = await adapter.load(project);
    expect(final.tasks).toHaveLength(2);
    expect(final.tasks.find((t) => t.id === reviewId)?.state).toBe("succeeded");
  });

  it("IP检查不自动覆盖平台，确认仅更新目标且拒绝旧条件", async () => {
    await adapter.execute(project, input(["platform-screen-3"]));
    await vi.advanceTimersByTimeAsync(100);
    const before = (await adapter.load(project)).screens.find((s) => s.id === "platform-screen-3")!;
    expect(before.platformStatus).toBe("online");
    expect(before.ping).toBe("offline");
    const change = { id: before.id, ip: before.ip, expected: before.platformStatus, next: before.ping!, revision: before.revision };
    await adapter.setScenario(project, "write_denied");
    expect((await adapter.coverStatus(project, [change]))[0].ok).toBe(false);
    await adapter.setScenario(project, "normal");
    expect((await adapter.coverStatus(project, [change]))[0].ok).toBe(true);
    const after = (await adapter.load(project)).screens.find((s) => s.id === before.id)!;
    expect(after.platformStatus).toBe("offline");
    expect(after.mac).toBe(before.mac); expect(after.ip).toBe(before.ip); expect(after.name).toBe(before.name);
    expect((await adapter.coverStatus(project, [change]))[0].ok).toBe(false);
  });

  it("导入验证含引用逗号、错误地址、重复行，批量保存原子失败", async () => {
    const snapshot = await adapter.load(project);
    const rows = parseScreenInventory('名称,IP,尺寸,MAC,空间路径,安装位置\n"会议室,北",192.0.2.180,10,,A座 / 1F / 东区,门口\n重复,192.0.2.180,4,,,\n错误,999.0.2.1,10,,,', snapshot.screens, snapshot.spaces);
    expect(rows[0].input.name).toBe("会议室,北");
    expect(rows[0].input.spaceId).toBe("area-a-1-east");
    expect(rows[1].error).toContain("重复"); expect(rows[2].error).toContain("IPv4");
    await expect(adapter.importLocal(project, [local("192.0.2.180"), local("999.0.2.1")])).rejects.toThrow("IPv4");
    expect((await adapter.load(project)).screens).toHaveLength(27);
  });

  it("刷新原型不会自动重放正在执行的任务", async () => {
    await adapter.execute(project, input(["platform-screen-1", "platform-screen-2"]));
    await vi.advanceTimersByTimeAsync(20); adapter.dispose();
    const restored = await new FixtureScreenAdapter(localStorage).load(project);
    expect(restored.tasks[0].state).toBe("needs_review");
    expect(restored.tasks[0].targets.map((t) => t.state)).toEqual(["needs_review", "cancelled"]);
    expect(localStorage.getItem(SCREEN_STORAGE_PREFIX + project)).toBeTruthy();
  });
});

describe("智能屏查询与项目隔离", () => {
  it("执行过程不作为资产状态，安装成功才更新最终版本", async () => {
    await adapter.setScenario(project, "partial_failure");
    const before = await adapter.load(project);
    const id = await adapter.execute(project, input(["platform-screen-2", "platform-screen-5"], "install"));
    await vi.advanceTimersByTimeAsync(20);
    const running = await adapter.load(project);
    expect(running.screens.find((s) => s.id === "platform-screen-2")?.appVersion).toBe("1.5.2");
    await vi.advanceTimersByTimeAsync(200);
    const after = await adapter.load(project);
    expect(after.tasks.find((t) => t.id === id)?.state).toBe("partially_succeeded");
    expect(after.screens.find((s) => s.id === "platform-screen-2")?.appVersion).toBe("1.6.0");
    expect(after.screens.find((s) => s.id === "platform-screen-5")?.appVersion).toBe(before.screens.find((s) => s.id === "platform-screen-5")?.appVersion);
    const saved = JSON.parse(localStorage.getItem(SCREEN_STORAGE_PREFIX + project)!);
    expect(saved).not.toHaveProperty("scenario");
    expect(saved.screens.every((s: Record<string, unknown>) => !("state" in s) && !("scenario" in s))).toBe(true);
  });
  it("组合筛选、跨页全选及本机模式计数一致", async () => {
    const store = useSmartScreensStore(); await store.bindProject(project);
    expect(store.stats.total).toBe(27); expect(store.stats.platform).toBe(21); expect(store.stats.local).toBe(6);
    store.pageSize = 10; expect(store.paged).toHaveLength(10);
    store.selectAllMatched(); expect(store.selectedIds).toHaveLength(27);
    store.filters.space = "floor-a-2"; store.filterChanged();
    expect(store.selectedIds).toHaveLength(0);
    expect(store.filtered.every((s) => s.building === "A座" && s.floor === "2F")).toBe(true);
    await adapter.setScenario(project, "platform_offline"); await store.refresh();
    expect(store.platformAvailable).toBe(false); expect(store.stats.platform).toBe(21);
    expect(store.visibleScreens).toHaveLength(27); expect(store.stats.local).toBe(6);
    await store.bindProject("second-project"); expect(store.stats.total).toBe(27); expect(store.filters.space).toBe("");
    store.stop();
  });
});
