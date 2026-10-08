import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FixtureScreenAdapter, SCREEN_STORAGE_PREFIX } from "@/dev-fixtures/screenFixtureAdapter";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { buildScreenRegistrationItems, screenPlatformDraftValues, screenPlatformFields, validRegistrationMac } from "@/shared/model/screenRegistration";
import type { ScreenRegistrationPreview, ScreenRegistrationSubmission } from "@/shared/model/screenRegistration";
import type { ScreenSnapshot, SmartScreen } from "@/shared/model/screen";

const project = "registration-project";
let adapter: FixtureScreenAdapter;
beforeEach(() => { localStorage.clear(); vi.useFakeTimers(); vi.setSystemTime(new Date("2026-09-28T08:00:00Z")); adapter = new FixtureScreenAdapter(localStorage, 10); });
afterEach(() => { adapter.dispose(); vi.useRealTimers(); });
const row = (state: ScreenSnapshot, id: string) => state.screens.find((screen) => screen.id === id)!;
function seed(change: (state: ScreenSnapshot) => void) {
  const state = createScreenSnapshot(); change(state);
  localStorage.setItem(SCREEN_STORAGE_PREFIX + project, JSON.stringify({ schemaVersion: 3, ...state }));
}
function submission(preview: ScreenRegistrationPreview): ScreenRegistrationSubmission {
  return { previewId: preview.id, screenIds: preview.items.filter((item) => item.state === "ready").map((item) => item.screenId),
    macConfirmations: Object.fromEntries(preview.items.filter((item) => item.requiredMacConfirmation).map((item) => [item.screenId, item.requiredMacConfirmation!])),
    spaceConfirmations: preview.items.filter((item) => item.needsSpaceConfirmation).map((item) => item.screenId) };
}
async function taskFor(preview: ScreenRegistrationPreview) {
  const id = await adapter.submitPlatformRegistration(project, submission(preview));
  return (await adapter.load(project)).tasks.find((task) => task.id === id)!;
}

describe("智能屏注册与平台资料草稿", () => {
  it("新登记预检采集MAC但不改资产，成功替换ID并保留别名及数量", async () => {
    await adapter.saveLocal(project, { name: "新装屏", ip: "192.0.2.211", mac: "", size: "10", spaceId: "area-a-1-room", location: "门口" });
    const before = await adapter.load(project), local = before.screens.find((screen) => screen.ip === "192.0.2.211")!;
    const preview = await adapter.previewPlatformRegistration(project, [local.id]);
    expect(preview.items[0]).toMatchObject({ state: "ready", mode: "create", macSource: "collected", requiredMacConfirmation: null });
    expect(preview.items[0]!.after.mac).toMatch(/^02:FE:/);
    expect((await adapter.load(project)).screens).toEqual(before.screens);
    const task = await taskFor(preview), after = await adapter.load(project);
    expect(task).toMatchObject({ action: "register", state: "succeeded" });
    const registered = after.screens.find((screen) => screen.aliases.includes(local.id))!;
    expect(registered).toMatchObject({ source: "platform", name: local.name, spaceId: "area-a-1-room", platformStatus: "online", ping: null, appVersion: null });
    expect(registered.id).not.toBe(local.id); expect(after.screens).toHaveLength(before.screens.length);
    expect(after.screens.some((screen) => screen.id === local.id)).toBe(false);
    expect(task.targets[0]!.screenId).toBe(local.id);
    adapter.dispose(); adapter = new FixtureScreenAdapter(localStorage, 10);
    const restored = await adapter.load(project);
    expect(restored.screens.filter((screen) => screen.aliases.includes(local.id))).toHaveLength(1);
    expect(restored.tasks[0]!.targets[0]!.screenId).toBe(local.id);
  });

  it("平台普通资料草稿独立保存；ADB不可用不阻断名称更新，状态版本不变", async () => {
    seed((state) => { row(state, "platform-screen-2").adbAvailable = false; });
    const before = await adapter.load(project), screen = row(before, "platform-screen-2");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), name: "新名称" }, screen.revision);
    const draftState = await adapter.load(project);
    expect(draftState.screens).toEqual(before.screens);
    expect(draftState.platformDrafts?.[screen.id]).toMatchObject({ base: { name: screen.name }, values: { name: "新名称" } });
    const preview = await adapter.previewPlatformRegistration(project, [screen.id]);
    expect(preview.items[0]).toMatchObject({ mode: "update", state: "ready", macSource: "unchanged", requiredMacConfirmation: null });
    expect((await taskFor(preview)).state).toBe("succeeded");
    const after = await adapter.load(project);
    expect(row(after, screen.id)).toMatchObject({ name: "新名称", platformStatus: screen.platformStatus, appVersion: screen.appVersion, aliases: screen.aliases });
    expect(after.platformDrafts?.[screen.id]).toBeUndefined(); expect(after.screens).toHaveLength(before.screens.length);
  });

  it("保存白名单字段且无变化会清除草稿，过期编辑版本被拒绝", async () => {
    const screen = row(await adapter.load(project), "platform-screen-2"), fields = screenPlatformFields(screen);
    await expect(adapter.savePlatformDraft(project, screen.id, { ...fields, name: "旧弹窗" }, screen.revision - 1)).rejects.toThrow("已变化");
    await adapter.savePlatformDraft(project, screen.id, { ...fields, name: "待改", appVersion: "伪造版本", platformStatus: "offline" } as typeof fields);
    expect((await adapter.load(project)).platformDrafts?.[screen.id]?.values).not.toHaveProperty("appVersion");
    await adapter.savePlatformDraft(project, screen.id, fields, screen.revision);
    expect((await adapter.load(project)).platformDrafts?.[screen.id]).toBeUndefined();
    expect((await adapter.previewPlatformRegistration(project, [screen.id])).items[0]!.state).toBe("skip");
  });

  it("草稿与丢弃在重载后保持独立，不产生本机资产行", async () => {
    const screen = row(await adapter.load(project), "platform-screen-2");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), location: "待改位置" });
    adapter.dispose(); adapter = new FixtureScreenAdapter(localStorage, 10);
    expect((await adapter.load(project)).platformDrafts?.[screen.id]?.values.location).toBe("待改位置");
    expect((await adapter.load(project)).screens).toHaveLength(27);
    await adapter.discardPlatformDraft(project, screen.id);
    expect((await adapter.load(project)).platformDrafts?.[screen.id]).toBeUndefined();
  });

  it("脏字段叠加最新平台值，同字段并发冲突阻断，明确重存可建立新基线", async () => {
    const original = createScreenSnapshot(), screen = row(original, "platform-screen-2");
    const base = screenPlatformFields(screen);
    const draft = { screenId: screen.id, baseRevision: screen.revision, base, values: { ...base, name: "我的名称" }, updatedAt: "earlier" };
    screen.location = "其他人修改的位置";
    expect(screenPlatformDraftValues(screen, draft)).toMatchObject({ name: "我的名称", location: "其他人修改的位置" });
    screen.name = "其他人的名称"; screen.revision++;
    original.platformDrafts = { [screen.id]: draft };
    const context = { platformAvailable: true, spacesAvailable: true, collectMac: () => null };
    expect(buildScreenRegistrationItems(original, [screen.id], context)[0]!.reason).toContain("名称已被其他操作修改");
    localStorage.setItem(SCREEN_STORAGE_PREFIX + project, JSON.stringify({ schemaVersion: 3, ...original }));
    await adapter.savePlatformDraft(project, screen.id, screenPlatformDraftValues(screen, draft), screen.revision);
    expect((await adapter.previewPlatformRegistration(project, [screen.id])).items[0]!.state).toBe("ready");
    expect((await adapter.load(project)).platformDrafts?.[screen.id]?.base.name).toBe("其他人的名称");
  });

  it("有效名称尺寸空间为提交前提，目录不可用时不能提交", async () => {
    seed((state) => { const screen = row(state, "local-screen-4"); screen.name = ""; screen.size = "unknown"; screen.spaceId = null; });
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-4"]);
    expect(preview.items[0]!.state).toBe("blocked");
    expect(preview.items[0]!.reason).toMatch(/名称不能为空.*4 寸或 10 寸.*有效空间/);
    await expect(adapter.submitPlatformRegistration(project, { previewId: preview.id, screenIds: ["local-screen-4"] })).rejects.toThrow("通过检查");
  });

  it("空间变更逐屏确认，保存草稿仍不修改平台空间", async () => {
    const screen = row(await adapter.load(project), "platform-screen-2");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), spaceId: "building-b" }, screen.revision);
    const preview = await adapter.previewPlatformRegistration(project, [screen.id]);
    expect(preview.items[0]!.needsSpaceConfirmation).toBe(true);
    await expect(adapter.submitPlatformRegistration(project, { ...submission(preview), spaceConfirmations: [] })).rejects.toThrow("空间变更");
    expect(row(await adapter.load(project), screen.id).spaceId).toBe(screen.spaceId);
    await taskFor(preview);
    expect(row(await adapter.load(project), screen.id)).toMatchObject({ spaceId: "building-b", building: "B座", floor: "" });
  });

  it("草稿保存校验IP/空间，离线可改普通资料但不能分配新的空间", async () => {
    const screen = row(await adapter.load(project), "platform-screen-2"), fields = screenPlatformFields(screen);
    await expect(adapter.savePlatformDraft(project, screen.id, { ...fields, ip: "invalid" })).rejects.toThrow("IPv4");
    await expect(adapter.savePlatformDraft(project, screen.id, { ...fields, spaceId: "missing" })).rejects.toThrow("空间关联无效");
    await adapter.setScenario(project, "platform_offline");
    await adapter.savePlatformDraft(project, screen.id, { ...fields, name: "离线待改" });
    await expect(adapter.savePlatformDraft(project, screen.id, { ...fields, spaceId: "building-b" })).rejects.toThrow("不能分配新的空间");
    expect((await adapter.previewPlatformRegistration(project, [screen.id])).items[0]!.state).toBe("blocked");
  });

  it("离线编辑保留此前草稿已选择的空间，不把保留草稿误判成新分配", async () => {
    const screen = row(await adapter.load(project), "platform-screen-2");
    const values = { ...screenPlatformFields(screen), spaceId: "building-b" };
    await adapter.savePlatformDraft(project, screen.id, values, screen.revision);
    await adapter.setScenario(project, "platform_offline");
    await adapter.savePlatformDraft(project, screen.id, { ...values, name: "离线补充名称" }, screen.revision);
    expect((await adapter.load(project)).platformDrafts?.[screen.id]?.values).toMatchObject({ spaceId: "building-b", name: "离线补充名称" });
    await expect(adapter.savePlatformDraft(project, screen.id, { ...values, spaceId: "floor-b-2" })).rejects.toThrow("不能分配新的空间");
  });
});

describe("注册身份、重复目标与提交恢复", () => {
  it("新登记重复IP或MAC阻断，不能以空值确认绕过冲突", async () => {
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-1", "local-screen-2", "local-screen-3"]);
    expect(preview.items.every((item) => item.state === "blocked" && item.duplicateIds.length)).toBe(true);
    await expect(adapter.submitPlatformRegistration(project, { previewId: preview.id, screenIds: ["local-screen-3"], macConfirmations: { "local-screen-3": "empty" } })).rejects.toThrow("通过检查");
    expect((await adapter.load(project)).screens).toHaveLength(27);
  });

  it("MAC采集失败有历史须逐屏确认历史；无历史须确认空值", async () => {
    seed((state) => { row(state, "local-screen-4").adbAvailable = false; const empty = row(state, "local-screen-6"); empty.adbAvailable = false; empty.mac = ""; empty.observedMac = ""; });
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-4", "local-screen-6"]);
    expect(preview.items.map((item) => [item.state, item.requiredMacConfirmation])).toEqual([["ready", "existing"], ["ready", "empty"]]);
    await expect(adapter.submitPlatformRegistration(project, { ...submission(preview), macConfirmations: {} })).rejects.toThrow("逐屏确认");
    const task = await taskFor(preview);
    expect(task.state).toBe("succeeded");
    expect(task.logs.some((log) => log.message.includes(preview.createdAt) && log.message.includes("已逐屏确认沿用历史 MAC"))).toBe(true);
    expect(task.logs.some((log) => log.message.includes("已逐屏确认以空 MAC 继续"))).toBe(true);
    const emptyRegistered = (await adapter.load(project)).screens.find((screen) => screen.aliases.includes("local-screen-6"))!;
    expect(emptyRegistered.mac).toBe("");
  });

  it("已知MAC冲突不会因为采集失败或空值确认变成可提交", async () => {
    seed((state) => { const screen = row(state, "local-screen-4"); screen.mac = "02:11:22:33:44:55"; screen.adbAvailable = false; });
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-4"]);
    expect(preview.items[0]).toMatchObject({ state: "blocked", macSource: "conflict" });
    await expect(adapter.submitPlatformRegistration(project, { previewId: preview.id, screenIds: ["local-screen-4"], macConfirmations: { "local-screen-4": "empty" } })).rejects.toThrow("通过检查");
  });

  it("修改IP或MAC要求身份检查，而普通名称更新不强求ADB", async () => {
    const screen = row(await adapter.load(project), "platform-screen-3");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), mac: "02:11:22:33:44:55" });
    expect((await adapter.previewPlatformRegistration(project, [screen.id])).items[0]).toMatchObject({ state: "blocked", macSource: "conflict" });
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), ip: "192.0.2.212", mac: "" }, screen.revision);
    expect((await adapter.previewPlatformRegistration(project, [screen.id])).items[0]).toMatchObject({ state: "ready", macSource: "collected" });
  });

  it("同一屏正常修改IP时模拟物理MAC保持稳定", async () => {
    const screen = row(await adapter.load(project), "platform-screen-7");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), ip: "192.0.2.223" }, screen.revision);
    const preview = await adapter.previewPlatformRegistration(project, [screen.id]);
    expect(preview.items[0]).toMatchObject({ state: "ready", macSource: "collected", after: { ip: "192.0.2.223", mac: screen.mac } });
    await taskFor(preview);
    expect(row(await adapter.load(project), screen.id)).toMatchObject({ ip: "192.0.2.223", mac: screen.mac, ping: null, checkedAt: null, appVersion: screen.appVersion });
  });

  it("同时修改IP和MAC且采集失败时不能把新MAC当历史确认", async () => {
    seed((state) => { row(state, "platform-screen-7").adbAvailable = false; });
    const screen = row(await adapter.load(project), "platform-screen-7");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), ip: "192.0.2.223", mac: "02:11:22:33:44:55" });
    const preview = await adapter.previewPlatformRegistration(project, [screen.id]);
    expect(preview.items[0]).toMatchObject({ state: "blocked", macSource: "conflict" });
    await expect(adapter.submitPlatformRegistration(project, { previewId: preview.id, screenIds: [screen.id], macConfirmations: { [screen.id]: "existing" } })).rejects.toThrow("通过检查");
  });

  it("采集成功也不能以同时改IP和MAC替换平台已有物理身份", async () => {
    seed((state) => { row(state, "platform-screen-7").observedMac = "02:11:22:33:44:55"; });
    const screen = row(await adapter.load(project), "platform-screen-7");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), ip: "192.0.2.223", mac: screen.observedMac });
    const preview = await adapter.previewPlatformRegistration(project, [screen.id]);
    expect(preview.items[0]).toMatchObject({ state: "blocked", macSource: "conflict" });
    expect(preview.items[0]!.reason).toContain("普通资料更新不能替换设备身份");
  });

  it("本机持久化失败时不留下部分注册或丢弃原资料", async () => {
    const failing = new FixtureScreenAdapter({ getItem: () => null, setItem: () => { throw new Error("本机存储已满"); } } as unknown as Storage);
    try {
      const before = await failing.load(project), preview = await failing.previewPlatformRegistration(project, ["local-screen-4"]);
      await expect(failing.submitPlatformRegistration(project, submission(preview))).rejects.toThrow("本机存储已满");
      const after = await failing.load(project);
      expect(after.screens).toEqual(before.screens); expect(after.tasks).toHaveLength(0);
    } finally { failing.dispose(); }
  });

  it("修改本机资料使旧预览失效，返回预览的篡改不能改变提交白名单", async () => {
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-4"]);
    const local = row(await adapter.load(project), "local-screen-4");
    await adapter.saveLocal(project, { ...local, name: "新名称" }, local.id);
    await expect(adapter.submitPlatformRegistration(project, submission(preview))).rejects.toThrow("已变化");
    const fresh = await adapter.previewPlatformRegistration(project, [local.id]);
    fresh.items[0]!.after.name = "篡改预览返回值";
    await taskFor(fresh);
    expect((await adapter.load(project)).screens.find((screen) => screen.aliases.includes(local.id))!.name).toBe("新名称");
  });

  it("平台离线、并发变化或跨项目预览均不产生写入", async () => {
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-4"]);
    await adapter.setScenario(project, "platform_offline");
    await expect(adapter.submitPlatformRegistration(project, submission(preview))).rejects.toThrow("平台不可用");
    await adapter.setScenario(project, "status_changed");
    await expect(adapter.submitPlatformRegistration(project, submission(preview))).rejects.toThrow("已变化");
    await expect(adapter.submitPlatformRegistration("other", submission(preview))).rejects.toThrow("不属于当前项目");
    expect((await adapter.load(project)).tasks).toHaveLength(0);
  });

  it("批次某屏被拒绝时保留该屏，其余正常登记；重复提交返回同一任务", async () => {
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-4", "local-screen-6"]);
    await adapter.setScenario(project, "partial_failure");
    const task = await taskFor(preview);
    expect(task.state).toBe("partially_succeeded");
    const next = await adapter.load(project);
    expect(next.screens.some((screen) => screen.aliases.includes("local-screen-4"))).toBe(true);
    expect(row(next, "local-screen-6").source).toBe("local");
    expect(await adapter.submitPlatformRegistration(project, submission(preview))).toBe(task.id);
    expect((await adapter.load(project)).tasks).toHaveLength(1);
  });

  it("平台拒绝资料更新时旧字段、状态版本和草稿均保留", async () => {
    const screen = row(await adapter.load(project), "platform-screen-2");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), name: "待提交" });
    const preview = await adapter.previewPlatformRegistration(project, [screen.id]);
    await adapter.setScenario(project, "write_denied");
    expect((await taskFor(preview)).state).toBe("failed");
    const next = await adapter.load(project);
    expect(row(next, screen.id)).toEqual(screen); expect(next.platformDrafts?.[screen.id]?.values.name).toBe("待提交");
  });

  it("回读失联保留受理凭据，重载后先核实且不重复创建", async () => {
    const preview = await adapter.previewPlatformRegistration(project, ["local-screen-4"]);
    await adapter.setScenario(project, "needs_review");
    const task = await taskFor(preview);
    expect(task.state).toBe("needs_review"); expect(row(await adapter.load(project), "local-screen-4").source).toBe("local");
    expect((await adapter.previewPlatformRegistration(project, ["local-screen-4"])).items[0]!.state).toBe("blocked");
    adapter.dispose(); adapter = new FixtureScreenAdapter(localStorage, 10);
    expect(await adapter.submitPlatformRegistration(project, submission(preview))).toBe(task.id);
    await adapter.verify(project, task.id);
    const restored = await adapter.load(project);
    expect(restored.tasks[0]!.state).toBe("succeeded"); expect(restored.screens).toHaveLength(27);
    expect(restored.screens.filter((screen) => screen.aliases.includes("local-screen-4"))).toHaveLength(1);
    await expect(adapter.verify(project, task.id)).rejects.toThrow("没有待核实");
  });

  it("平台注册不携带本机在线观测或小新版本到平台默认字段", async () => {
    seed((state) => { const screen = row(state, "local-screen-4"); screen.ping = "offline"; screen.appVersion = "9.9.9"; });
    await taskFor(await adapter.previewPlatformRegistration(project, ["local-screen-4"]));
    const registered = (await adapter.load(project)).screens.find((screen) => screen.aliases.includes("local-screen-4"))!;
    expect(registered).toMatchObject({ platformStatus: "online", ping: "offline", appVersion: null, observedAppVersion: "9.9.9" });
  });

  it("编辑草稿或完成注册不会让其他正在执行的任务脱离持久化", async () => {
    const id = await adapter.execute(project, { action: "ping", targetIds: ["platform-screen-2"], appVersion: "", abi: "universal", reinstall: false, concurrency: 1 });
    const screen = row(await adapter.load(project), "platform-screen-5");
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), name: "待改" });
    await taskFor(await adapter.previewPlatformRegistration(project, ["local-screen-4"]));
    await vi.advanceTimersByTimeAsync(100);
    expect((await adapter.load(project)).tasks.find((task) => task.id === id)?.state).toBe("succeeded");
  });

  it("通用设备执行通道不能绕过专用注册确认", async () => {
    await expect(adapter.execute(project, { action: "register", targetIds: ["local-screen-4"], appVersion: "", abi: "universal", reinstall: false, concurrency: 1 })).rejects.toThrow("专用资料预览");
  });
});

describe("纯领域预检", () => {
  it("注册MAC排除广播、组播、全零以及不完整地址", () => {
    for (const mac of ["FF:FF:FF:FF:FF:FF", "00:00:00:00:00:00", "01:00:5E:01:02:03", "03:11:22:33:44:55", "02:11"]) expect(validRegistrationMac(mac)).toBe(false);
    expect(validRegistrationMac("02:11:22:33:44:55")).toBe(true);
    expect(validRegistrationMac("00-11-22-33-44-55")).toBe(true);
  });
  it("两个待提交草稿变为同一地址时均阻断", () => {
    const state = createScreenSnapshot();
    state.platformDrafts = {};
    for (const id of ["platform-screen-2", "platform-screen-5"]) {
      const screen = row(state, id), base = screenPlatformFields(screen);
      state.platformDrafts[id] = { screenId: id, baseRevision: screen.revision, base, values: { ...base, ip: "192.0.2.222", mac: "" }, updatedAt: "now" };
    }
    const result = buildScreenRegistrationItems(state, Object.keys(state.platformDrafts), { platformAvailable: true, spacesAvailable: true, collectMac: () => "02:11:22:33:44:66" });
    expect(result.every((item) => item.state === "blocked" && item.reason.includes("本批次"))).toBe(true);
  });

  it("目录节点失效和目录不可用均阻断，完整深层节点可使用", () => {
    const state = createScreenSnapshot(), screen = row(state, "local-screen-4");
    screen.spaceId = "area-a-1-room";
    const context = { platformAvailable: true, spacesAvailable: true, collectMac: (value: SmartScreen) => value.observedMac };
    expect(buildScreenRegistrationItems(state, [screen.id], context)[0]!.state).toBe("ready");
    expect(buildScreenRegistrationItems(state, [screen.id], { ...context, spacesAvailable: false })[0]!.reason).toContain("空间目录不可用");
    state.spaces = state.spaces.filter((space) => space.id !== "area-a-1-room");
    expect(buildScreenRegistrationItems(state, [screen.id], context)[0]!.reason).toContain("有效空间");
  });
});
