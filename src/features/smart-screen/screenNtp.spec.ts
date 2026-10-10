import { defineComponent, h, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NCheckbox, NInput, NMessageProvider, NPagination, NSelect } from "naive-ui";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { screenActions, type ScreenOperationInput, type ScreenTask } from "@/shared/model/screen";
import { validateNtpServer, type ScreenNtpRead, type ScreenNtpPatch } from "@/shared/model/screenNtp";
import ScreenNtpConfig from "./ScreenNtpConfig.vue";
import ScreenOperations from "./ScreenOperations.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";

async function render(options: { count?: number; mixed?: boolean; failFirst?: boolean; autoTime?: boolean; operations?: boolean } = {}) {
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 20), store = useSmartScreensStore(pinia);
  configureScreenAdapter(adapter); await store.bindProject("ntp-ui");
  const sample = store.snapshot.screens[0]!;
  store.snapshot.screens = Array.from({ length: options.count ?? 2 }, (_, index) => ({ ...sample, id: `ntp-screen-${index + 1}`, name: `测试屏${index + 1}`, ip: `192.0.2.${index + 1}`, size: index % 2 ? "10" : "4" }));
  store.selectedIds = store.snapshot.screens.map(screen => screen.id); store.operation = "ntp";
  const ids = [...store.selectedIds];
  const read = vi.fn(async (_project: string, targets: string[]): Promise<ScreenNtpRead[]> => targets.map(id => ({
    screenId: id, readAt: "2026-10-10T01:00:00Z", config: options.failFirst && id === ids[0] ? null : { server: options.mixed && id !== ids[0] ? "ntp.internal" : "", autoTime: options.autoTime ?? true, autoTimeZone: false, timeZone: "Asia/Shanghai" },
    capabilities: { activation: "reboot", rebootRequired: true }, message: options.failFirst && id === ids[0] ? "ADB 读取失败" : "已读取",
  })));
  const preflight = vi.fn(async (_project: string, input: ScreenOperationInput, patches: Record<string, ScreenNtpPatch>) => input.targetIds.map(id => ({ screenId: id, name: id, ip: "192.0.2.1", state: "ready" as const, reason: patches[id] ? "检查通过" : "缺少设置" })));
  Object.assign(adapter, { readNtp: read, preflightNtp: preflight });
  const execute = vi.spyOn(adapter, "execute").mockResolvedValue("ntp-task"), step = ref(0), historyOpen = ref(false);
  if (options.operations) vi.spyOn(adapter, "load").mockImplementation(async () => JSON.parse(JSON.stringify(store.snapshot)));
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, { default: () => options.operations ? h(ScreenOperations, { historyOpen: historyOpen.value, "onUpdate:historyOpen": value => historyOpen.value = value }) : h(ScreenNtpConfig, { step: step.value, "onUpdate:step": value => step.value = value }) }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
  const click = async (text: string) => { await wrapper.findAll("button").find(button => button.text().includes(text))!.trigger("click"); await flushPromises(); };
  const input = async (value: string) => { wrapper.findAllComponents(NInput).find(control => control.attributes("aria-label") === "NTP服务器地址")!.vm.$emit("update:value", value); await flushPromises(); };
  const select = async (value: string) => { wrapper.findAllComponents(NSelect).find(control => control.attributes("aria-label") === "NTP地址处理")!.vm.$emit("update:value", value); await flushPromises(); };
  const checkbox = async (id: string, value: boolean) => { wrapper.findAllComponents(NCheckbox).find(control => control.attributes("aria-label") === `选择 ${store.snapshot.screens.find(screen => screen.id === id)!.name}`)!.vm.$emit("update:checked", value); await flushPromises(); };
  return { wrapper, store, adapter, ids, read, preflight, execute, step, historyOpen, click, input, select, checkbox, dispose() { wrapper.unmount(); store.stop(); adapter.dispose(); } };
}

describe("NTP 地址与操作菜单", () => {
  it("配置、校时与 NTP 连续排列，共用菜单供操作页和详情使用", () => { expect(screenActions.map(action => action.value).slice(2, 5)).toEqual(["app_config", "time", "ntp"]); });
  it.each(["", "192.168.3.142", "ntp.internal", "ntp-1.example.com", "ntp", "::1", "2001:db8::1"])("接受服务器地址 %s", value => { expect(validateNtpServer(value)).toBe(""); });
  it.each(["http://192.168.3.142", "192.168.3.142:123", "ntp.internal/path", "192.168.3.999", "ntp server", "-ntp.local"])("拒绝不受支持的地址 %s", value => { expect(validateNtpServer(value)).not.toBe(""); });
});

describe("先读取再修改 NTP", () => {
  it("未设置显示空地址，同时支持 4 寸和 10 寸屏", async () => {
    const view = await render(); try { await view.click("读取 2 台 NTP 设置"); expect(view.wrapper.get('[aria-label="NTP服务器地址"] input').element.getAttribute("value") || "").toBe(""); expect(view.wrapper.text()).toContain("未设置自定义地址"); expect(view.wrapper.text()).toContain("同值验证 2 台"); expect(view.preflight).not.toHaveBeenCalled(); } finally { view.dispose(); }
  });
  it("修改后带逐屏地址与身份预检，确认重启后才执行", async () => {
    const view = await render(); try {
      await view.click("读取 2 台 NTP 设置"); await view.input("192.168.3.142"); await view.click("检查设置");
      expect(view.preflight.mock.calls[0]![1]).toMatchObject({ action: "ntp", targetIds: view.ids, expectedTargets: expect.any(Object) });
      expect(view.preflight.mock.calls[0]![2]).toEqual(Object.fromEntries(view.ids.map(id => [id, { server: "192.168.3.142" }])));
      await view.click("保存并生效 2 台"); expect(view.execute).not.toHaveBeenCalled(); expect(view.wrapper.text()).toContain("可能需要重启系统");
      await view.click("确认设置并按需重启"); expect(view.execute).toHaveBeenCalledExactlyOnceWith("ntp-ui", expect.objectContaining({ action: "ntp", targetIds: view.ids }));
    } finally { view.dispose(); }
  });
  it("混合原值留空不表示统一清空，显式恢复默认才提交空地址", async () => {
    const view = await render({ mixed: true }); try {
      await view.click("读取 2 台 NTP 设置"); await view.click("检查设置");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[0]!]: { server: "" }, [view.ids[1]!]: { server: "ntp.internal" } });
      await view.select("default"); await view.click("检查设置");
      expect(view.preflight.mock.calls[1]![2]).toEqual(Object.fromEntries(view.ids.map(id => [id, { server: "" }])));
      expect(view.wrapper.text()).toContain("恢复固件默认会移除自定义地址");
    } finally { view.dispose(); }
  });
  it("同值允许验证授时，保存会开启原来关闭的自动校时", async () => {
    const view = await render({ autoTime: false }); try { await view.click("读取 2 台 NTP 设置"); expect(view.wrapper.text()).toContain("本次修改 2 台"); await view.click("检查设置"); expect(view.preflight).toHaveBeenCalledOnce(); } finally { view.dispose(); }
  });
  it("非法服务器地址不产生预检或写入", async () => {
    const view = await render(); try { await view.click("读取 2 台 NTP 设置"); await view.input("http://192.168.3.142:123"); await view.click("检查设置"); expect(view.wrapper.text()).toContain("不包含协议、端口或路径"); expect(view.preflight).not.toHaveBeenCalled(); expect(view.execute).not.toHaveBeenCalled(); } finally { view.dispose(); }
  });
  it("读取失败不进入修改范围；修改输入使旧预检失效", async () => {
    const view = await render({ failFirst: true }); try {
      await view.click("读取 2 台 NTP 设置"); expect(view.wrapper.text()).toContain("读取失败"); await view.input("192.168.3.142"); await view.click("检查设置"); expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids.slice(1));
      await view.input("192.168.3.143"); expect(view.wrapper.findAll("button").some(button => button.text().includes("保存并生效"))).toBe(false); expect(view.execute).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });
  it("重新读取保留输入和排除范围，跨页选择仍提交全部已选目标", async () => {
    const view = await render({ count: 22 }); try {
      await view.click("读取 22 台 NTP 设置"); await view.input("ntp.internal"); view.wrapper.getComponent(NPagination).vm.$emit("update:page-size", 20); await flushPromises(); view.wrapper.getComponent(NPagination).vm.$emit("update:page", 2); await flushPromises(); await view.checkbox(view.ids[20]!, false); await view.click("重新读取设置");
      expect(view.wrapper.get('[aria-label="NTP服务器地址"] input').element.getAttribute("value")).toBe("ntp.internal"); await view.click("检查设置"); expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids.filter(id => id !== view.ids[20]));
    } finally { view.dispose(); }
  }, 15000);
  it("切换项目会丢弃旧读取响应，不填入新项目", async () => {
    const view = await render(); let finish: ((rows: ScreenNtpRead[]) => void) | undefined;
    view.read.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    try {
      await view.click("读取 2 台 NTP 设置"); view.store.projectId = "ntp-other"; await flushPromises(); finish?.([{ screenId: view.ids[0]!, readAt: "2026-10-10T01:00:00Z", config: { server: "old.internal", autoTime: true, autoTimeZone: false, timeZone: "Asia/Shanghai" }, message: "旧响应" }]); await flushPromises(); expect(view.step.value).toBe(0); expect(view.wrapper.text()).not.toContain("old.internal"); expect(view.preflight).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });
});

function failedNtpTask(view: Awaited<ReturnType<typeof render>>, id = "original-ntp-operation"): ScreenTask {
  return { id, projectId: view.store.projectId, action: "ntp", state: "failed", mode: "real", createdAt: "2026-10-10T01:00:00Z", updatedAt: "2026-10-10T01:00:00Z", logs: [],
    input: { action: "ntp", retryOfOperationId: "older-ntp-operation", targetIds: [...view.ids], appVersion: "", abi: "universal", reinstall: false, concurrency: 1 },
    targets: view.store.selectedScreens.map(screen => ({ screenId: screen.id, name: screen.name, ip: screen.ip, state: "failed", progress: 100, message: "服务器不可达，授时失败" })) };
}

describe("NTP重试操作关联", () => {
  it("当前结果重试重新读取后，预检与提交关联本次原操作编号", async () => {
    const view = await render({ operations: true }), original = failedNtpTask(view);
    view.execute.mockImplementationOnce(async () => { view.store.snapshot.tasks = [original]; return original.id; });
    try {
      await view.click("读取 2 台 NTP 设置"); await view.input("192.168.3.142"); await view.click("检查设置"); await view.click("保存并生效 2 台"); await view.click("确认设置并按需重启");
      expect(view.wrapper.getComponent(ScreenTaskResults).props("task").id).toBe(original.id);
      await view.click("重试 2 台失败 / 未执行设备"); expect(view.execute).toHaveBeenCalledOnce();
      await view.click("读取 2 台 NTP 设置"); await view.input("192.168.3.142"); await view.click("检查设置");
      expect(view.preflight.mock.calls[1]![1].retryOfOperationId).toBe(original.id);
      await view.click("重新读取设置"); await view.click("检查设置"); expect(view.preflight.mock.calls[2]![1].retryOfOperationId).toBe(original.id);
      await view.click("保存并生效 2 台"); await view.click("确认设置并按需重启");
      expect(view.execute.mock.calls[1]![1]).toMatchObject({ retryOfOperationId: original.id, action: "ntp", targetIds: view.ids });
      expect(view.execute.mock.calls[1]![1].retryOfOperationId).not.toBe(original.input!.retryOfOperationId);
    } finally { view.dispose(); }
  });
  it("历史结果重试映射旧屏标识，并在读取、预检和提交中保留原操作关联", async () => {
    const view = await render({ operations: true }), original = failedNtpTask(view, "history-ntp-operation");
    view.store.snapshot.screens[0]!.aliases = ["old-local-screen"];
    original.targets[0]!.screenId = "old-local-screen"; original.input!.targetIds[0] = "old-local-screen";
    view.store.snapshot.tasks = [original]; view.historyOpen.value = true;
    try {
      await flushPromises(); view.wrapper.getComponent(ScreenTaskResults).vm.$emit("retry", ["old-local-screen", view.ids[1]]); await flushPromises();
      expect(view.historyOpen.value).toBe(false); expect(view.store.selectedIds).toEqual(view.ids);
      expect(view.read).not.toHaveBeenCalled(); expect(view.execute).not.toHaveBeenCalled();
      await view.click("读取 2 台 NTP 设置"); await view.input("ntp.internal"); await view.click("检查设置");
      expect(view.preflight.mock.calls[0]![1]).toMatchObject({ retryOfOperationId: original.id, targetIds: view.ids });
      await view.click("保存并生效 2 台"); await view.click("确认设置并按需重启");
      expect(view.execute.mock.calls[0]![1]).toMatchObject({ retryOfOperationId: original.id, targetIds: view.ids });
    } finally { view.dispose(); }
  });
  it("从重试转为新的智能屏操作会清除旧操作关联", async () => {
    const view = await render({ operations: true }), original = failedNtpTask(view);
    view.store.snapshot.tasks = [original]; view.historyOpen.value = true;
    try {
      await flushPromises(); view.wrapper.getComponent(ScreenTaskResults).vm.$emit("retry", view.ids); await flushPromises();
      await view.click("读取 2 台 NTP 设置"); await view.input("ntp.internal"); await view.click("检查设置"); expect(view.preflight.mock.calls[0]![1].retryOfOperationId).toBe(original.id);
      await view.click("重新选屏"); await view.click("读取 2 台 NTP 设置"); await view.input("ntp.internal"); await view.click("检查设置");
      expect(view.preflight.mock.calls[1]![1].retryOfOperationId).toBeUndefined(); expect(view.execute).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });
});
