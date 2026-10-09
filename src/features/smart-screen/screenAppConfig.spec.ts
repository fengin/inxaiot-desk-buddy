import { defineComponent, h, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NMessageProvider, NSelect, NInput, NCheckbox, NPagination } from "naive-ui";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ScreenOperationInput } from "@/shared/model/screen";
import type { ScreenAppConfigPatch, ScreenAppConfigRead, ScreenAppConfiguration } from "@/shared/model/screenAppConfig";
import ScreenAppConfig from "./ScreenAppConfig.vue";

const originalOta = "https://config.example.org";
const originalWs = "wss://voice.example.org";
const originalH5 = "https://screen.example.org";

function sampleConfiguration(): ScreenAppConfiguration {
  const env = {
    otaUrl: originalOta, wsUrl: originalWs, h5Url: originalH5,
    h5ReadyCheckEnabled: true, otaWsUrl: null, otaH5Url: null,
    effectiveWsUrl: originalWs, effectiveH5Url: originalH5,
    wsSource: "manual" as const, h5Source: "manual" as const,
  };
  return { customDeviceName: "原名称", environments: { current: "pre", test: { ...env }, pre: { ...env }, prod: { ...env } } };
}

async function render(options: { count?: number; failFirst?: boolean; restoreDraft?: boolean; sameEnvironment?: boolean; draftSwitch?: boolean } = {}) {
  const count = options.count ?? 4;
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 20);
  const store = useSmartScreensStore(pinia);
  configureScreenAdapter(adapter);
  await store.bindProject("config-ui");
  const sample = store.snapshot.screens[0]!;
  store.snapshot.screens = Array.from({ length: count }, (_, index) => ({
    ...sample, id: `config-screen-${index + 1}`, name: `测试屏${index + 1}`, ip: `192.0.2.${index + 1}`,
  }));
  store.operation = "app_config";
  store.selectedIds = store.snapshot.screens.map(screen => screen.id);
  const ids = [...store.selectedIds];
  const read = vi.fn(async (_project: string, targets: string[]): Promise<ScreenAppConfigRead[]> => targets.map(id => {
    const config = sampleConfiguration();
    config.customDeviceName = id;
    if (id === ids.at(-1)) {
      if (!options.sameEnvironment) config.environments.current = "prod";
      config.environments.pre.otaUrl = "https://different.example.org";
    }
    const failed = options.failFirst && id === ids[0];
    return { screenId: id, readAt: "2026-10-03T10:00:00Z", config: failed ? null : config, capabilities: null, message: failed ? "请升级小新" : "已读取" };
  }));
  const preflight = vi.fn(async (_project: string, input: ScreenOperationInput, patches: Record<string, ScreenAppConfigPatch>) => input.targetIds.map(id => ({
    screenId: id, name: id, ip: "192.0.2.1", state: "ready" as const, reason: patches[id] ? "检查通过" : "缺少修改内容",
  })));
  Object.assign(adapter, { readAppConfig: read, preflightAppConfig: preflight });
  if (options.restoreDraft) {
    const rows = await read(store.projectId, ids);
    read.mockClear();
    Object.assign(adapter, {
      loadAppConfigDraft: vi.fn(async () => ({
        environment: options.draftSwitch ? "test" : "pre", switchEnvironment: Boolean(options.draftSwitch), readyMode: "keep",
        edits: { otaUrl: { mode: "keep", value: "" }, wsUrl: { mode: "set", value: "wss://draft.example.org" }, h5Url: { mode: "keep", value: "" } },
        names: Object.fromEntries(ids.map(id => [id, { mode: "keep", value: "" }])),
        targetIds: ids, includedIds: ids, rows,
      })),
      saveAppConfigDraft: vi.fn(async () => {}),
    });
  }
  const execute = vi.spyOn(adapter, "execute").mockResolvedValue("config-task");
  const step = ref(0);
  const wrapper = mount(defineComponent({
    setup: () => () => h(NMessageProvider, null, { default: () => h(ScreenAppConfig, { step: step.value, "onUpdate:step": value => step.value = value }) }),
  }), { global: { plugins: [pinia], stubs: { teleport: true } } });
  const button = (text: string) => {
    const control = wrapper.findAll("button").find(item => item.text().includes(text));
    expect(control).toBeDefined();
    return control!;
  };
  const click = async (text: string) => { await button(text).trigger("click"); await flushPromises(); };
  const clickLabel = async (label: string) => { await wrapper.get(`button[aria-label="${label}"]`).trigger("click"); await flushPromises(); };
  const select = async (label: string, value: string) => {
    const control = wrapper.findAllComponents(NSelect).find(component => component.attributes("aria-label") === label);
    expect(control).toBeDefined();
    control!.vm.$emit("update:value", value);
    await flushPromises();
  };
  const inputControl = (label: string) => {
    const control = wrapper.findAllComponents(NInput).find(component => component.attributes("aria-label") === label);
    expect(control).toBeDefined();
    return control!;
  };
  const input = async (label: string, value: string) => { inputControl(label).vm.$emit("update:value", value); await flushPromises(); };
  const checkbox = async (label: string, value: boolean) => {
    const control = wrapper.findAllComponents(NCheckbox).find(component => component.attributes("aria-label") === label || component.text() === label);
    expect(control).toBeDefined();
    control!.vm.$emit("update:checked", value);
    await flushPromises();
  };
  const changePage = async (page: number) => { wrapper.getComponent(NPagination).vm.$emit("update:page", page); await flushPromises(); };
  return {
    wrapper, store, ids, read, preflight, execute, step, button, click, clickLabel, select, input, inputControl, checkbox, changePage,
    dispose() { wrapper.unmount(); store.stop(); adapter.dispose(); },
  };
}

describe("小新配置页面", () => {
  it("默认展示三台多数值，保留每台实际值；不同屏可查看详情", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      expect(view.read).toHaveBeenCalledTimes(4);
      expect(view.inputControl("OTA 地址").props("value")).toBe(originalOta);
      expect(view.inputControl("WebSocket(可选)").props("value")).toBe(originalWs);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.wrapper.get('fieldset[aria-label="批量配置表单"]').text()).not.toContain("当前：");
      expect(view.wrapper.text()).not.toContain("同时切换到此环境");
      expect(view.wrapper.findAll('button[aria-label$="配置差异"]').length).toBe(1);
      expect(view.wrapper.find('button[aria-label="测试屏4配置差异"]').text()).toContain("2 项不同");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      expect(view.wrapper.findAll("button").some(button => button.text().includes("重新选屏"))).toBe(false);
      await view.clickLabel("测试屏4配置差异");
      expect(view.wrapper.text()).toContain("当前运行：生产环境");
      expect(view.wrapper.text()).toContain("https://different.example.org");
      expect(view.preflight).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it("直接输入只发送这个字段，名称和未修改环境不进入请求", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.input("WebSocket(可选)", "wss://new.example.org/ws");
      expect(view.inputControl("WebSocket(可选)").props("status")).toBe("warning");
      expect(view.wrapper.text()).toContain("预计重启 3 台");
      await view.click("检查修改内容");
      for (const id of view.ids) expect(view.preflight.mock.calls[0]![2][id]).toEqual({ set: { "environments.pre.wsUrl": "wss://new.example.org/ws" }, clear: [] });
      await view.click("确认修改 4 台");
      expect(view.execute).toHaveBeenCalledTimes(1);
      expect(view.step.value).toBe(2);
    } finally { view.dispose(); }
  });

  it("输入还原为读取基线后取消修改，不把少数屏自动统一", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.input("OTA 地址", "https://new.example.org");
      expect(view.wrapper.text()).toContain("本次修改 4 台");
      await view.input("OTA 地址", originalOta);
      expect(view.inputControl("OTA 地址").props("status")).toBeUndefined();
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      expect(view.preflight).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it("明确统一为多数值时，仅提交原值不同的那台", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.clickLabel("统一OTA 地址");
      expect(view.wrapper.text()).toContain("本次修改 1 台");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual([view.ids[3]]);
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[3]!]: { set: { "environments.pre.otaUrl": originalOta }, clear: [] } });
    } finally { view.dispose(); }
  });

  it("从详情直接修改设备名，只修改所选的一台", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.clickLabel("查看 测试屏2 配置详情");
      expect(view.inputControl("测试屏2设备名").props("value")).toBe(view.ids[1]);
      await view.input("测试屏2设备名", "独立名称");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual([view.ids[1]]);
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[1]!]: { set: { customDeviceName: "独立名称" }, clear: [] } });
    } finally { view.dispose(); }
  });

  it("可选地址删除表示清空，但 OTA 地址为空时不能检查提交", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.input("WebSocket(可选)", "");
      await view.input("H5地址(可选)", "");
      await view.input("OTA 地址", "");
      await view.click("检查修改内容");
      expect(view.wrapper.text()).toContain("OTA 地址不能为空");
      expect(view.preflight).not.toHaveBeenCalled();
      await view.input("OTA 地址", originalOta);
      await view.click("检查修改内容");
      for (const id of view.ids) expect(view.preflight.mock.calls[0]![2][id]).toEqual({ set: {}, clear: ["environments.pre.wsUrl", "environments.pre.h5Url"] });
    } finally { view.dispose(); }
  });

  it("主动选择环境后直接切换，并只修改该环境内明确编辑的字段", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.select("选择环境", "test");
      expect(view.wrapper.text()).toContain("本次修改 4 台");
      await view.input("WebSocket(可选)", "wss://test.example.org");
      await view.click("检查修改内容");
      expect(view.wrapper.text()).toContain("预计重启 4 台");
      for (const id of view.ids) expect(view.preflight.mock.calls[0]![2][id]).toEqual({ set: { "environments.current": "test", "environments.test.wsUrl": "wss://test.example.org" }, clear: [] });
    } finally { view.dispose(); }
  });

  it("明确选择环境后重新读取，保留切换环境及待修改值", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.select("选择环境", "test");
      await view.input("H5地址(可选)", "https://test.example.org");
      await view.click("重新读取配置");
      expect(view.read).toHaveBeenCalledTimes(8);
      expect(view.inputControl("H5地址(可选)").props("value")).toBe("https://test.example.org");
      await view.click("检查修改内容");
      for (const id of view.ids) expect(view.preflight.mock.calls[0]![2][id]).toEqual({ set: { "environments.current": "test", "environments.test.h5Url": "https://test.example.org" }, clear: [] });
    } finally { view.dispose(); }
  });

  it("选回所有屏当前运行的环境后取消切换，不影响随后单项编辑", async () => {
    const view = await render({ sameEnvironment: true });
    try {
      await view.click("读取 4 台配置");
      await view.select("选择环境", "test");
      expect(view.wrapper.text()).toContain("本次修改 4 台");
      await view.select("选择环境", "pre");
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.input("WebSocket(可选)", "wss://new.example.org");
      await view.click("检查修改内容");
      for (const id of view.ids) expect(view.preflight.mock.calls[0]![2][id]).toEqual({ set: { "environments.pre.wsUrl": "wss://new.example.org" }, clear: [] });
    } finally { view.dispose(); }
  });

  it("H5 检测开关直接编辑，还原原值不产生修改", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      expect(view.wrapper.text()).toContain("检查H5地址");
      await view.checkbox("H5检测", false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: { "environments.pre.h5ReadyCheckEnabled": false }, clear: [] });
      await view.checkbox("H5检测", true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
    } finally { view.dispose(); }
  });

  it("读取失败的屏显示原因，并排除在修改请求之外", async () => {
    const view = await render({ failFirst: true });
    try {
      await view.click("读取 4 台配置");
      expect(view.wrapper.text()).toContain("读取失败");
      expect(view.wrapper.text()).toContain("请升级小新");
      const failed = view.wrapper.findAllComponents(NCheckbox).find(control => control.attributes("aria-label") === "选择 测试屏1")!;
      expect(failed.props("disabled")).toBe(true);
      await view.input("H5地址(可选)", "https://new.example.org");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids.slice(1));
      expect(view.preflight.mock.calls[0]![2]).not.toHaveProperty(view.ids[0]!);
    } finally { view.dispose(); }
  });

  it("恢复本机草稿后必须重新读取，并保留待提交修改", async () => {
    const view = await render({ restoreDraft: true });
    try {
      await flushPromises();
      await view.click("继续本机未提交修改");
      expect(view.wrapper.text()).toContain("请重新读取屏端当前配置");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.click("重新读取配置");
      expect(view.inputControl("WebSocket(可选)").props("value")).toBe("wss://draft.example.org");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: { "environments.pre.wsUrl": "wss://draft.example.org" }, clear: [] });
    } finally { view.dispose(); }
  });

  it("恢复草稿并重新读取后，仍保留草稿中明确选择的目标环境", async () => {
    const view = await render({ restoreDraft: true, draftSwitch: true });
    try {
      await flushPromises();
      await view.click("继续本机未提交修改");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.click("重新读取配置");
      await view.click("检查修改内容");
      for (const id of view.ids) expect(view.preflight.mock.calls[0]![2][id]).toEqual({ set: { "environments.current": "test", "environments.test.wsUrl": "wss://draft.example.org" }, clear: [] });
    } finally { view.dispose(); }
  });

  it("尚未读取的提示只显示在输入占位，不另占标签下面一行", async () => {
    const view = await render();
    let finishRead: ((rows: ScreenAppConfigRead[]) => void) | undefined;
    view.read.mockImplementationOnce(() => new Promise(resolve => { finishRead = resolve; }));
    try {
      await view.click("读取 4 台配置");
      for (const label of ["OTA 地址", "WebSocket(可选)", "H5地址(可选)"]) {
        expect(view.inputControl(label).props("placeholder")).toContain("尚未读取");
        expect(view.inputControl(label).props("value")).toBe("");
      }
      const form = view.wrapper.get('fieldset[aria-label="批量配置表单"]');
      for (const label of form.findAll("label")) expect(label.element.parentElement!.textContent).not.toContain("尚未读取");
      expect(form.text()).not.toContain("当前：");
    } finally {
      finishRead?.([]);
      await flushPromises();
      view.dispose();
    }
  });

  it("取消勾选后重新读取仍保留选择，已排除屏不会重新进入批次", async () => {
    const view = await render();
    try {
      await view.click("读取 4 台配置");
      await view.checkbox("选择 测试屏3", false);
      await view.click("重新读取配置");
      expect(view.read).toHaveBeenCalledTimes(8);
      expect(view.wrapper.text()).toContain("已选 3 台");
      await view.input("WebSocket(可选)", "wss://new.example.org");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids.filter(id => id !== view.ids[2]));
    } finally { view.dispose(); }
  });

  it("分页只影响显示，跨页选择与批量修改范围保持一致", async () => {
    const view = await render({ count: 22 });
    try {
      await view.click("读取 22 台配置");
      view.wrapper.getComponent(NPagination).vm.$emit("update:page-size", 20);
      await flushPromises();
      expect(view.wrapper.find('[data-screen-id="config-screen-21"]').exists()).toBe(false);
      await view.changePage(2);
      expect(view.wrapper.find('[data-screen-id="config-screen-1"]').exists()).toBe(false);
      expect(view.wrapper.find('[data-screen-id="config-screen-21"]').exists()).toBe(true);
      await view.checkbox("选择 测试屏21", false);
      await view.input("WebSocket(可选)", "wss://batch.example.org");
      await view.changePage(1);
      expect(view.wrapper.text()).toContain("已选 21 台");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids.filter(id => id !== view.ids[20]));
      expect(Object.keys(view.preflight.mock.calls[0]![2])).toHaveLength(21);
    } finally { view.dispose(); }
  }, 15000);
});
