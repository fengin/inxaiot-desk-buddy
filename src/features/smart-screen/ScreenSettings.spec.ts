import { createPinia } from "pinia";
import { defineComponent, h, nextTick, ref } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NCheckbox, NInput, NInputNumber, NMessageProvider } from "naive-ui";
import { describe, expect, it, vi } from "vitest";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ScreenOperationInput, ScreenTask } from "@/shared/model/screen";
import { screenSettingsDefaultPatch, screenSettingsEmptyPatch, screenSettingsRetryPatch, screenSettingsUncertain, screenSettingsView, type ScreenSettingsPatch, type ScreenSettingsRead } from "@/shared/model/screenSettings";
import ScreenSettings from "./ScreenSettings.vue";

const project = "settings-ui", ids = ["platform-screen-19", "platform-screen-2"];
const input = (): ScreenOperationInput => ({ action: "screen_settings", targetIds: ids, appVersion: "", abi: "universal", reinstall: false, concurrency: 3 });
async function settled(adapter: FixtureScreenAdapter, taskId: string) {
  await vi.waitFor(async () => expect((await adapter.load(project)).tasks.find(task => task.id === taskId)?.state).toBe("succeeded"));
}
async function render(options: { satisfied?: boolean; retry?: ScreenTask; adapter?: FixtureScreenAdapter; count?: number } = {}) {
  const adapter = options.adapter ?? new FixtureScreenAdapter(localStorage, 10);
  configureScreenAdapter(adapter); configureActivityAdapter(new FixtureActivityAdapter(adapter));
  if (options.satisfied) {
    await adapter.preflightSettings(project, input(), Object.fromEntries(ids.map(id => [id, screenSettingsDefaultPatch()])));
    await settled(adapter, await adapter.execute(project, input()));
  }
  const pinia = createPinia(), store = useSmartScreensStore(pinia);
  await store.bindProject(project); store.selectedIds = options.count ? store.snapshot.screens.slice(0, options.count).map(screen => screen.id) : [...ids]; store.operation = "screen_settings";
  const step = ref(0);
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, () => h(ScreenSettings, { step: step.value, "onUpdate:step": value => { step.value = value; }, retryTask: options.retry })) }), { attachTo: document.body, global: { plugins: [pinia] } });
  await flushPromises();
  return { wrapper, store, adapter, step, close: () => { wrapper.unmount(); store.stop(); adapter.dispose(); } };
}
async function read(context: Awaited<ReturnType<typeof render>>) {
  await context.wrapper.get('[data-testid="screen-settings-read"]').trigger("click"); await flushPromises();
  await vi.waitFor(() => expect(context.step.value).toBe(1));
}
async function edit(context: Awaited<ReturnType<typeof render>>) { await read(context); }
const ntpInput = (wrapper: Awaited<ReturnType<typeof render>>["wrapper"]) => wrapper.findAllComponents(NInput).find(input => input.attributes("data-testid") === "screen-settings-ntp")!;
async function checkbox(context: Awaited<ReturnType<typeof render>>, label: string, value: boolean) {
  context.wrapper.findAllComponents(NCheckbox).find(box => box.attributes("aria-label") === (label === "修改NTP服务器" ? "NTP服务器" : label))!.vm.$emit("update:checked", value); await nextTick();
}
async function ntp(context: Awaited<ReturnType<typeof render>>, value: string) {
  await checkbox(context, "修改NTP服务器", true); ntpInput(context.wrapper).vm.$emit("update:value", value); await nextTick();
}
async function submit(context: Awaited<ReturnType<typeof render>>) {
  await context.wrapper.get('[data-testid="screen-settings-submit"]').trigger("click"); await flushPromises();
}
async function deferredReads(context: Awaited<ReturnType<typeof render>>) {
  const expected = new Map((await context.adapter.readSettings(project, context.store.selectedIds)).map(row => [row.screenId, row]));
  const pending = new Map<string, (rows: ScreenSettingsRead[]) => void>(); let peak = 0;
  const calls = vi.spyOn(context.adapter, "readSettings").mockImplementation((_project, targets) => new Promise(resolve => {
    expect(targets).toHaveLength(1); pending.set(targets[0]!, resolve); peak = Math.max(peak, pending.size);
  }));
  return { pending, calls, peak: () => peak, async complete(id = pending.keys().next().value!) {
    const resolve = pending.get(id)!; pending.delete(id); resolve([expected.get(id)!]); await flushPromises();
  } };
}

class CapabilityFixture extends FixtureScreenAdapter {
  override async readSettings(projectId: string, screenIds: string[]) {
    const rows = await super.readSettings(projectId, screenIds);
    for (const row of rows) if (row.screenId === ids[0]) { row.items.adb.state = "blocked"; row.items.adb.reason = "当前固件不支持修改持久端口"; }
    return rows;
  }
  override async preflightSettings(projectId: string, request: ScreenOperationInput, patches: Record<string, ScreenSettingsPatch>) {
    const checks = await super.preflightSettings(projectId, request, patches), rows = await this.readSettings(projectId, request.targetIds);
    return checks.map(check => { const settings = screenSettingsView(rows.find(row => row.screenId === check.screenId)!, patches[check.screenId]);
      return { ...check, settings, reason: settings.blockedCount ? Object.values(settings.items).filter(item => ["blocked", "unknown"].includes(item.state)).map(item => item.reason).join("；") : check.reason,
        state: settings.blockedCount ? "blocked" as const : settings.pendingCount ? "ready" as const : "skip" as const }; });
  }
}

describe("设置智能屏统一交互", () => {
  it.each([1, 2, 3])("首屏并发%s控制单屏worker，全部读完才自动进入第二屏", async concurrency => {
    const context = await render({ count: 5 }), { wrapper, adapter, store } = context;
    const deferred = await deferredReads(context), check = vi.spyOn(adapter, "preflightSettings").mockImplementation(async (_project, request) => request.targetIds.map(id => ({ screenId: id, name: id, ip: "192.0.2.1", state: "ready" as const, reason: "检查通过" }))), execute = vi.spyOn(adapter, "execute").mockResolvedValue("new-task");
    try {
      wrapper.findComponent(NInputNumber).vm.$emit("update:value", concurrency); await nextTick();
      expect(wrapper.get('[data-testid="screen-operation-selection-summary"]').text()).toContain("已选 5 台");
      await wrapper.get('[data-testid="screen-settings-read"]').trigger("click"); await flushPromises();
      expect(deferred.pending.size).toBe(concurrency); expect(context.step.value).toBe(0);
      expect(wrapper.findComponent(NInputNumber).props("disabled")).toBe(true);
      expect(wrapper.get('[data-testid="screen-settings-read-progress"]').text()).toContain("已完成 0/5");
      while (deferred.calls.mock.calls.length < 5 || deferred.pending.size > 1) {
        await deferred.complete(); expect(context.step.value).toBe(0);
      }
      await deferred.complete();
      expect(context.step.value).toBe(1); expect(deferred.peak()).toBe(concurrency);
      expect(wrapper.findAll('[data-screen-id]').map(row => row.attributes("data-screen-id"))).toEqual(store.selectedIds);
      await submit(context);
      expect(check.mock.calls[0]?.[1].concurrency).toBe(concurrency);
      expect(execute.mock.calls[0]?.[1].concurrency).toBe(concurrency);
    } finally { while (deferred.pending.size) await deferred.complete(); context.close(); }
  });
  it("所有读取失败留在首屏并显示原因，不进入设置或提交", async () => {
    const context = await render(), check = vi.spyOn(context.adapter, "preflightSettings"), execute = vi.spyOn(context.adapter, "execute");
    vi.spyOn(context.adapter, "readSettings").mockRejectedValue(new Error("ADB连接失败"));
    try {
      await context.wrapper.get('[data-testid="screen-settings-read"]').trigger("click"); await flushPromises();
      expect(context.step.value).toBe(0); expect(context.wrapper.text()).toContain("均未取得可靠状态");
      expect(context.wrapper.text()).toContain("ADB连接失败"); expect(context.wrapper.find('[data-testid="screen-settings-submit"]').exists()).toBe(false);
      expect(check).not.toHaveBeenCalled(); expect(execute).not.toHaveBeenCalled();
    } finally { context.close(); }
  });
  it("4/10混选读完自动进入设置页，默认三项勾选、NTP未勾选，详情颜色分明", async () => {
    const context = await render(), { wrapper, adapter } = context;
    const execute = vi.spyOn(adapter, "execute");
    try {
      await read(context);
      expect(context.step.value).toBe(1); expect(wrapper.findAll('[data-screen-id]')).toHaveLength(2); expect(execute).not.toHaveBeenCalled();
      expect(wrapper.find('[data-testid="screen-settings-next"]').exists()).toBe(false);
      expect(wrapper.find('[data-testid="screen-settings-preflight"]').exists()).toBe(false);
      expect(wrapper.findComponent(NInputNumber).exists()).toBe(false);
      for (const label of ["保持ADB端口", "校准时间", "关闭屏睡眠"]) expect(wrapper.findAllComponents(NCheckbox).find(box => box.attributes("aria-label") === label)?.props("checked")).toBe(true);
      expect(ntpInput(wrapper).props("value")).toBe("");
      expect(ntpInput(wrapper).props("disabled")).toBe(true);
      await wrapper.get('[data-screen-id="platform-screen-2"] [data-testid="screen-settings-detail"]').trigger("click"); await flushPromises();
      expect(document.body.textContent).toContain("当前 → 目标");
      expect(document.querySelector('[data-setting-key="adb"] td:nth-child(3)')?.classList.contains("screen-warning")).toBe(true);
      expect(document.querySelector('[data-setting-key="time"] td:nth-child(3)')?.classList.contains("screen-success")).toBe(true);
      expect(document.querySelector('[data-setting-key="ntp"] td:nth-child(3)')?.classList.contains("screen-muted")).toBe(true);
    } finally { context.close(); }
  });
  it("默认目标全满足仍可进入设置页填写NTP，最终全满足不创建任务", async () => {
    const context = await render({ satisfied: true }), { wrapper, adapter, store } = context;
    const execute = vi.spyOn(adapter, "execute");
    try {
      await edit(context); expect(context.step.value).toBe(1);
      await submit(context);
      expect(wrapper.get('[data-testid="screen-settings-all-satisfied"]').text()).toContain("均已满足");
      expect(wrapper.get('[data-testid="screen-settings-submit"]').attributes("disabled")).toBeDefined(); expect(execute).not.toHaveBeenCalled();
      await ntp(context, "ntp.example.test");
      expect(wrapper.find('[data-testid="screen-settings-all-satisfied"]').exists()).toBe(false);
      await submit(context);
      await vi.waitFor(() => expect(store.snapshot.tasks[0]?.state).toBe("succeeded"));
      expect(execute).toHaveBeenCalledOnce();
      expect(store.snapshot.tasks[0]?.settings?.[ids[0]]?.ntp).toEqual({ server: "ntp.example.test" });
      expect(wrapper.text()).toContain("共享重启：已确认（一次）");
    } finally { context.close(); }
  });
  it("NTP勾选且空值明确清除，取消及重新勾选保留目标、不误清", async () => {
    const context = await render(), { wrapper, adapter } = context;
    const check = vi.spyOn(adapter, "preflightSettings"), execute = vi.spyOn(adapter, "execute").mockRejectedValue(new Error("模拟队列未提交"));
    try {
      await edit(context);
      await checkbox(context, "修改NTP服务器", true);
      ntpInput(wrapper).vm.$emit("update:value", "  "); await nextTick();
      expect(wrapper.get('[data-testid="screen-settings-ntp-clear"]').text()).toContain("将清除");
      await submit(context);
      expect(check.mock.calls.at(-1)?.[2][ids[0]].ntp).toEqual({ server: "" });
      await ntp(context, "ntp.new.internal"); await checkbox(context, "修改NTP服务器", false); await submit(context);
      expect(check.mock.calls.at(-1)?.[2][ids[0]].ntp).toBeNull();
      await checkbox(context, "修改NTP服务器", true); expect(ntpInput(wrapper).props("value")).toBe("ntp.new.internal");
      await submit(context); expect(check.mock.calls.at(-1)?.[2][ids[0]].ntp).toEqual({ server: "ntp.new.internal" });
      expect(execute).toHaveBeenCalledTimes(3);
    } finally { context.close(); }
  });
  it("共同当前NTP地址只用于展示，未勾选时输入禁用且不参与修改", async () => {
    const context = await render(), original = context.adapter.readSettings.bind(context.adapter);
    vi.spyOn(context.adapter, "readSettings").mockImplementation(async (...args) => (await original(...args)).map(row => ({ ...row, items: { ...row.items, ntp: { ...row.items.ntp, current: { server: "current.internal", autoTime: true, syncConfirmed: true } } } })));
    const check = vi.spyOn(context.adapter, "preflightSettings"); vi.spyOn(context.adapter, "execute").mockResolvedValue("task");
    try {
      await edit(context); expect(ntpInput(context.wrapper).props("value")).toBe("current.internal");
      expect(ntpInput(context.wrapper).props("disabled")).toBe(true);
      ntpInput(context.wrapper).vm.$emit("update:value", "http://invalid"); await nextTick();
      expect(ntpInput(context.wrapper).props("value")).toBe("current.internal");
      await submit(context);
      expect(check.mock.calls[0]?.[2]).toEqual(Object.fromEntries(ids.map(id => [id, screenSettingsDefaultPatch()])));
    } finally { context.close(); }
  });
  it("当前NTP地址多值不擅取某屏，勾选留空明确清除且详情显示清除目标", async () => {
    const context = await render(), original = context.adapter.readSettings.bind(context.adapter);
    vi.spyOn(context.adapter, "readSettings").mockImplementation(async (...args) => (await original(...args)).map(row => ({ ...row, items: { ...row.items, ntp: { ...row.items.ntp, current: { server: row.screenId === ids[0] ? "one.internal" : "two.internal", autoTime: true, syncConfirmed: true } } } })));
    const check = vi.spyOn(context.adapter, "preflightSettings"); vi.spyOn(context.adapter, "execute").mockRejectedValue(new Error("模拟提交失败"));
    try {
      await edit(context); expect(ntpInput(context.wrapper).props("value")).toBe("");
      expect(ntpInput(context.wrapper).props("placeholder")).toContain("多个地址");
      await checkbox(context, "修改NTP服务器", true);
      expect(context.wrapper.get('[data-testid="screen-settings-ntp-clear"]').text()).toContain("清除");
      await context.wrapper.get('[data-testid="screen-settings-detail"]').trigger("click"); await flushPromises();
      expect(document.querySelector('[data-setting-key="ntp"] td:nth-child(2)')?.textContent).toContain("清除自定义地址");
      await submit(context); for (const id of ids) expect(check.mock.calls[0]?.[2][id].ntp).toEqual({ server: "" });
    } finally { context.close(); }
  });
  it("混合重试逐屏恢复NTP目标和勾选，切换不误清也不增加默认项目", async () => {
    const retry = { id: "retry-settings", projectId: project, action: "screen_settings", state: "cancelled", targets: ids.map(screenId => ({ screenId, state: "cancelled" })),
      settings: { [ids[0]!]: { ...screenSettingsEmptyPatch(), ntp: { server: "one.internal" } }, [ids[1]!]: { ...screenSettingsEmptyPatch(), ntp: { server: "two.internal" } } } } as ScreenTask;
    const context = await render({ retry }), check = vi.spyOn(context.adapter, "preflightSettings"); vi.spyOn(context.adapter, "execute").mockRejectedValue(new Error("模拟提交失败"));
    try {
      await edit(context); expect(ntpInput(context.wrapper).props("value")).toBe(""); expect(ntpInput(context.wrapper).props("placeholder")).toContain("多个目标");
      await checkbox(context, "修改NTP服务器", false); await checkbox(context, "修改NTP服务器", true);
      await submit(context); expect(check.mock.calls[0]?.[2]).toEqual(retry.settings);
      await ntp(context, "unified.internal"); await submit(context);
      for (const id of ids) expect(check.mock.calls[1]?.[2][id]).toEqual({ ...screenSettingsEmptyPatch(), ntp: { server: "unified.internal" } });
    } finally { context.close(); }
  });
  it.each(["original.internal", ""])("重试原NTP目标%s与未选屏混合时，公共开关恢复原选择", async server => {
    const original = { [ids[0]!]: { ...screenSettingsEmptyPatch(), ntp: { server } }, [ids[1]!]: { ...screenSettingsEmptyPatch(), sleepOff: true } };
    const retry = { id: "mixed-settings", projectId: project, action: "screen_settings", state: "cancelled", targets: ids.map(screenId => ({ screenId, state: "cancelled" })), settings: original } as ScreenTask;
    const context = await render({ retry }), check = vi.spyOn(context.adapter, "preflightSettings"); vi.spyOn(context.adapter, "execute").mockRejectedValue(new Error("模拟未提交"));
    try {
      await edit(context);
      expect(context.wrapper.findAllComponents(NCheckbox).find(box => box.attributes("data-testid") === "screen-settings-ntp-enabled")?.props("indeterminate")).toBe(true);
      expect(ntpInput(context.wrapper).props("value")).toBe(server);
      await checkbox(context, "修改NTP服务器", false); await checkbox(context, "修改NTP服务器", true); await submit(context);
      expect(check.mock.calls[0]?.[2]).toEqual(original);
    } finally { context.close(); }
  });
  it("勾选NTP后非法地址阻止预检，取消勾选恢复为不修改", async () => {
    const context = await render(), check = vi.spyOn(context.adapter, "preflightSettings"); vi.spyOn(context.adapter, "execute").mockResolvedValue("task");
    try {
      await edit(context); await ntp(context, "http://invalid"); await submit(context);
      expect(context.wrapper.text()).toContain("不包含协议、端口或路径"); expect(check).not.toHaveBeenCalled();
      await checkbox(context, "修改NTP服务器", false); await submit(context);
      for (const id of ids) expect(check.mock.calls[0]?.[2][id].ntp).toBeNull();
    } finally { context.close(); }
  });
  it("一次设置按钮等待最终预检，busy跨到提交且双击只建一个任务", async () => {
    const context = await render(), original = context.adapter.preflightSettings.bind(context.adapter);
    let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; });
    const check = vi.spyOn(context.adapter, "preflightSettings").mockImplementation(async (...args) => { await gate; return original(...args); });
    const execute = vi.spyOn(context.adapter, "execute").mockResolvedValue("task");
    try {
      await edit(context); await ntp(context, "new.internal");
      await context.wrapper.get('[data-testid="screen-settings-submit"]').trigger("click"); await flushPromises();
      await context.wrapper.get('[data-testid="screen-settings-submit"]').trigger("click");
      expect(check).toHaveBeenCalledOnce(); expect(execute).not.toHaveBeenCalled(); expect(context.step.value).toBe(1);
      expect(ntpInput(context.wrapper).props("disabled")).toBe(true);
      for (const label of ["保持ADB端口", "校准时间", "关闭屏睡眠"]) expect(context.wrapper.findAllComponents(NCheckbox).find(box => box.attributes("aria-label") === label)?.props("disabled")).toBe(true);
      expect(context.wrapper.text()).toContain("正在核对设置内容");
      ntpInput(context.wrapper).vm.$emit("update:value", "unexpected.internal"); await checkbox(context, "修改NTP服务器", false);
      release(); await flushPromises();
      expect(execute).toHaveBeenCalledOnce(); expect(context.step.value).toBe(2);
      expect(check.mock.calls[0]?.[2][ids[0]].ntp).toEqual({ server: "new.internal" });
    } finally { release(); context.close(); }
  });
  it("最终预检任一blocked整批停止，有ready也不提交并显示原因", async () => {
    const context = await render({ adapter: new CapabilityFixture(localStorage, 10) }), execute = vi.spyOn(context.adapter, "execute");
    try {
      await edit(context); await submit(context);
      expect(context.step.value).toBe(1); expect(context.wrapper.text()).toContain("当前固件不支持修改持久端口");
      expect(context.wrapper.text()).toContain("尚未设置任何屏"); expect(execute).not.toHaveBeenCalled();
    } finally { context.close(); }
  });
  it("最终预检ready与skip混合时只提交ready，保持冻结patch", async () => {
    const context = await render(), original = context.adapter.preflightSettings.bind(context.adapter);
    const check = vi.spyOn(context.adapter, "preflightSettings").mockImplementation(async (...args) => (await original(...args)).map((row, index) => index === 1 ? { ...row, state: "skip" as const } : { ...row, state: "ready" as const }));
    const execute = vi.spyOn(context.adapter, "execute").mockResolvedValue("task");
    try {
      await edit(context); await submit(context);
      expect(check.mock.calls[0]?.[1].targetIds).toEqual(ids);
      expect(execute).toHaveBeenCalledExactlyOnceWith(project, expect.objectContaining({ targetIds: [ids[0]] }));
    } finally { context.close(); }
  });
  it("最终预检请求失败停止，不执行旧计划或自动重试", async () => {
    const context = await render(), check = vi.spyOn(context.adapter, "preflightSettings").mockRejectedValue(new Error("最终检查连接失败")), execute = vi.spyOn(context.adapter, "execute");
    try {
      await edit(context); await submit(context);
      expect(context.wrapper.text()).toContain("最终检查连接失败"); expect(check).toHaveBeenCalledOnce(); expect(execute).not.toHaveBeenCalled();
      expect(context.step.value).toBe(1);
    } finally { context.close(); }
  });
  it.each(["project", "identity", "step"])("最终预检等待期间%s变化时不执行旧返回", async change => {
    const context = await render(), original = context.adapter.preflightSettings.bind(context.adapter);
    let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; });
    vi.spyOn(context.adapter, "preflightSettings").mockImplementation(async (...args) => { await gate; return original(...args); });
    const execute = vi.spyOn(context.adapter, "execute");
    try {
      await edit(context); await context.wrapper.get('[data-testid="screen-settings-submit"]').trigger("click"); await flushPromises();
      if (change === "project") await context.store.bindProject("other-project");
      else if (change === "identity") context.store.snapshot.screens.find(screen => screen.id === ids[0])!.mac = "00:11:22:33:44:55";
      else context.step.value = 0;
      await flushPromises(); release(); await flushPromises();
      expect(context.step.value).toBe(0); expect(execute).not.toHaveBeenCalled();
    } finally { release(); context.close(); }
  });
  it("项目或设备身份变化使读取及预检失效，不复用旧目标", async () => {
    const context = await render(), { wrapper, store } = context;
    try {
      await edit(context);
      store.snapshot.screens.find(screen => screen.id === ids[0])!.ip = "192.0.2.240"; await flushPromises();
      expect(context.step.value).toBe(0); expect(wrapper.find('[data-testid="screen-settings-submit"]').exists()).toBe(false);
      expect(wrapper.text()).toContain("身份已变化");
      await store.bindProject("other-local-project"); await flushPromises();
      expect(context.step.value).toBe(0);
    } finally { context.close(); }
  });
  it("提交发现检查过期时失效旧预检，不自动重放任务", async () => {
    const context = await render(), { wrapper, adapter } = context;
    const execute = vi.spyOn(adapter, "execute").mockRejectedValueOnce(new Error("检查记录已过期，请重新检查"));
    try {
      await edit(context); await submit(context);
      expect(wrapper.text()).toContain("检查记录已过期");
      expect(context.step.value).toBe(1);
      expect(execute).toHaveBeenCalledOnce(); expect((await adapter.load(project)).tasks).toHaveLength(0);
    } finally { context.close(); }
  });
  it("旧校时重试只选择校时，未执行组合任务保留原冻结子项", async () => {
    const retry = { id: "old-time", projectId: project, action: "time", state: "failed", targets: ids.map(screenId => ({ screenId, state: "failed" })) } as ScreenTask;
    const context = await render({ retry }), { wrapper, adapter } = context;
    const check = vi.spyOn(adapter, "preflightSettings");
    try {
      await edit(context);
      const boxes = wrapper.findAllComponents(NCheckbox);
      expect(boxes.find(box => box.attributes("aria-label") === "校准时间")?.props("checked")).toBe(true);
      expect(boxes.find(box => box.attributes("aria-label") === "保持ADB端口")?.props("checked")).toBe(false);
      expect(boxes.find(box => box.attributes("aria-label") === "关闭屏睡眠")?.props("checked")).toBe(false);
      await submit(context);
      expect(check.mock.calls[0]?.[2][ids[0]]).toEqual({ ...screenSettingsEmptyPatch(), time: true });
    } finally { context.close(); }
  });
  it("混合尺寸按单项能力显示，取消该屏不支持项目后可检查其余项目", async () => {
    const context = await render({ adapter: new CapabilityFixture(localStorage, 10) }), { wrapper, store, adapter } = context;
    store.selectedIds = [ids[0]]; await flushPromises();
    const check = vi.spyOn(adapter, "preflightSettings");
    try {
      await edit(context);
      await submit(context);
      expect(wrapper.text()).toContain("尚未设置任何屏");
      await wrapper.get('[data-testid="screen-settings-detail"]').trigger("click"); await flushPromises();
      expect(document.querySelector('[data-setting-key="adb"] td:nth-child(3)')?.classList.contains("screen-danger")).toBe(true);
      const checkbox = wrapper.findAllComponents(NCheckbox).find(box => box.attributes("aria-label") === "本屏保持ADB端口")!;
      checkbox.vm.$emit("update:checked", false); await nextTick();
      await submit(context);
      expect(check.mock.calls.at(-1)?.[2][ids[0]].adb).toBe(false);
    } finally { context.close(); }
  });
  it("延迟读取期间切换项目丢弃旧结果，不把旧屏带入新项目", async () => {
    let release!: () => void, started!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; }), began = new Promise<void>(resolve => { started = resolve; });
    const adapter = new FixtureScreenAdapter(localStorage, 10), readSettings = adapter.readSettings.bind(adapter);
    adapter.readSettings = async (...args) => { started(); await gate; return readSettings(...args); };
    const context = await render({ adapter });
    try {
      void context.wrapper.get('[data-testid="screen-settings-read"]').trigger("click"); await began;
      await context.store.bindProject("other-project"); await flushPromises(); release(); await flushPromises();
      expect(context.step.value).toBe(0);
      expect(context.wrapper.findAll('[data-screen-id]')).toHaveLength(0);
    } finally { release(); context.close(); }
  });
  it("某屏失败停止其后续项目、其他屏继续，重试只投影原失败未执行项", async () => {
    const adapter = new FixtureScreenAdapter(localStorage, 10);
    try {
      await adapter.preflightSettings(project, input(), Object.fromEntries(ids.map(id => [id, screenSettingsDefaultPatch()])));
      await adapter.setScenario(project, "partial_failure");
      const id = await adapter.execute(project, input());
      await vi.waitFor(async () => expect((await adapter.load(project)).tasks.find(task => task.id === id)?.state).toBe("partially_succeeded"));
      const task = (await adapter.load(project)).tasks.find(task => task.id === id)!;
      expect(task.targets.map(target => target.state)).toEqual(["succeeded", "failed"]);
      const patch = screenSettingsRetryPatch(task, task.targets[1]);
      expect(patch).toEqual({ adb: true, time: false, sleepOff: true, ntp: null });
      expect(task.targets[1].result?.evidence?.screenSettings).toMatchObject({ items: { adb: { state: "failed" }, sleepOff: { state: "not_started" } }, reboot: { required: true, requested: false } });
    } finally { adapter.dispose(); }
  });
  it("未知结果仅核实已执行效果，不重新派发或新增任务", async () => {
    const adapter = new FixtureScreenAdapter(localStorage, 10), execute = vi.spyOn(adapter, "execute");
    try {
      await adapter.preflightSettings(project, input(), Object.fromEntries(ids.map(id => [id, screenSettingsDefaultPatch()])));
      await adapter.setScenario(project, "needs_review"); const id = await adapter.execute(project, input());
      await vi.waitFor(async () => expect((await adapter.load(project)).tasks.find(task => task.id === id)?.state).toBe("needs_review"));
      const before = (await adapter.load(project)).tasks.find(task => task.id === id)!;
      expect(screenSettingsUncertain(before.targets[0])).toBe(true); expect(screenSettingsRetryPatch(before, before.targets[0])).toBeNull();
      await adapter.verify(project, id);
      expect((await adapter.load(project)).tasks.find(task => task.id === id)?.state).toBe("succeeded");
      expect((await adapter.load(project)).tasks).toHaveLength(1); expect(execute).toHaveBeenCalledOnce();
    } finally { adapter.dispose(); }
  });
});
