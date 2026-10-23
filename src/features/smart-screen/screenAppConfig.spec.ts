import { defineComponent, h, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NMessageProvider, NSelect, NInput, NInputNumber, NCheckbox, NPagination, NModal, NButton } from "naive-ui";
import { readFileSync } from "node:fs";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ScreenOperationInput, ScreenSize } from "@/shared/model/screen";
import type { AppConfigFieldDefinition, AppConfigVoiceEdit, ScreenAppConfigDraft, ScreenAppConfigPatch, ScreenAppConfigRead, ScreenAppConfiguration } from "@/shared/model/screenAppConfig";
import ScreenAppConfig from "./ScreenAppConfig.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import { screenActions } from "@/shared/model/screen";

const originalOta = "https://config.example.org";
const originalWs = "wss://voice.example.org";
const originalH5 = "https://screen.example.org";
const fixtureJson = (name: string) => JSON.parse(readFileSync(`test/fixtures/app-config-schema-v1/${name}.json`, "utf8")).data;
const nativeFixture = { default: { capabilities: fixtureJson("schema-capabilities-default"), config: fixtureJson("schema-read-default") }, small: { capabilities: fixtureJson("schema-capabilities-small"), config: fixtureJson("schema-read-small") } };

function sampleConfiguration(): ScreenAppConfiguration {
  const env = {
    otaUrl: originalOta, wsUrl: originalWs, h5Url: originalH5,
    h5ReadyCheckEnabled: true, otaWsUrl: null, otaH5Url: null,
    effectiveWsUrl: originalWs, effectiveH5Url: originalH5,
    wsSource: "manual" as const, h5Source: "manual" as const,
  };
  return { customDeviceName: "原名称", environments: { current: "pre", test: { ...env }, pre: { ...env }, prod: { ...env } } };
}

async function render(options: { count?: number; sizes?: ScreenSize[]; failFirst?: boolean; restoreDraft?: boolean; sameEnvironment?: boolean; draftSwitch?: boolean; homeValues?: (boolean | "unsupported" | "unknown")[]; homeActiveValues?: (boolean | undefined)[]; draftHomeMode?: "keep" | "on" | "off"; draftHomeActivation?: boolean; ownerValues?: ("self" | "none" | "other" | "unknown" | "unsupported")[]; kioskValues?: (boolean | "missing" | "unknown" | "unsupported")[]; kioskActiveValues?: (boolean | null | undefined)[]; draftEnsureDeviceOwner?: true; draftKioskMode?: "keep" | "on" | "off"; readyValues?: boolean[]; wsValues?: (string | null)[]; voiceValues?: (boolean | "unsupported" | "missing" | "invalid")[]; draftVoiceMode?: "keep" | "on" | "off"; schemas?: ("default" | "small" | "legacy")[]; audioOverrides?: Record<string, number>[]; definitions?: AppConfigFieldDefinition[]; draftVoiceEdits?: Record<string, AppConfigVoiceEdit> } = {}) {
  const count = options.count ?? 4;
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 20);
  const store = useSmartScreensStore(pinia);
  configureScreenAdapter(adapter);
  await store.bindProject("config-ui");
  const sample = store.snapshot.screens[0]!;
  store.snapshot.screens = Array.from({ length: count }, (_, index) => ({
    ...sample, id: `config-screen-${index + 1}`, name: `测试屏${index + 1}`, ip: `192.0.2.${index + 1}`, size: options.sizes?.[index] ?? "10",
  }));
  store.operation = "app_config";
  store.selectedIds = store.snapshot.screens.map(screen => screen.id);
  const ids = [...store.selectedIds];
  const read = vi.fn(async (_project: string, targets: string[]): Promise<ScreenAppConfigRead[]> => targets.map(id => {
    let config = sampleConfiguration();
    config.customDeviceName = id;
    if (id === ids.at(-1)) {
      if (!options.sameEnvironment) config.environments.current = "prod";
      config.environments.pre.otaUrl = "https://different.example.org";
    }
    const failed = options.failFirst && id === ids[0];
    if (options.readyValues) config.environments.pre.h5ReadyCheckEnabled = options.readyValues[ids.indexOf(id)]!;
    if (options.wsValues) config.environments.pre.wsUrl = options.wsValues[ids.indexOf(id)] ?? null;
    const home = options.homeValues?.[ids.indexOf(id)];
    const owner = options.ownerValues?.[ids.indexOf(id)] ?? "self";
    const kiosk = options.kioskValues?.[ids.indexOf(id)];
    const voice = options.voiceValues?.[ids.indexOf(id)];
    if (typeof voice === "boolean") config.autoStartVoiceEngine = voice;
    else if (voice === "unsupported") config.autoStartVoiceEngine = true;
    else if (voice === "invalid") config.autoStartVoiceEngine = "false" as unknown as boolean;
    let capabilities: ScreenAppConfigRead["capabilities"] = voice === undefined ? null : { appVersionName: "1.6.0", appVersionCode: 30, protocolVersion: 1, supportedFields: voice === "unsupported" ? [] : ["autoStartVoiceEngine"], ready: true };
    const kind = options.schemas?.[ids.indexOf(id)];
    if (kind && kind !== "legacy") {
      config = JSON.parse(JSON.stringify(nativeFixture[kind].config)) as ScreenAppConfiguration;
      config.customDeviceName = id;
      capabilities = JSON.parse(JSON.stringify(nativeFixture[kind].capabilities)) as ScreenAppConfigRead["capabilities"];
      Object.assign(config.audioConfig!, options.audioOverrides?.[ids.indexOf(id)] ?? {});
      if (options.definitions) {
        capabilities!.configSchema!.fields.push(...JSON.parse(JSON.stringify(options.definitions)));
        capabilities!.supportedFields.push(...options.definitions.map(field => field.key));
      }
    }
    return { screenId: id, readAt: "2026-10-03T10:00:00Z", config: failed ? null : config, capabilities, message: failed ? "请升级小新" : "已读取",
      ...(owner === "unsupported" ? {} : { owner: { state: owner, message: owner === "other" ? "设备OWNER由其他应用持有" : owner === "unknown" ? "设备OWNER状态未确认" : "已核对OWNER" } }),
      ...(kiosk === undefined ? {} : { kiosk: kiosk === "unsupported" ? { supported: false, message: "当前小新不支持应用锁定，请升级" }
        : { supported: true, configured: kiosk !== "missing", ...(typeof kiosk === "boolean" ? { enabled: kiosk } : {}), active: options.kioskActiveValues ? options.kioskActiveValues[ids.indexOf(id)] : typeof kiosk === "boolean" ? kiosk : kiosk === "missing" ? false : undefined, activityReady: true, message: "已读取锁定设置" } }),
      ...(home === undefined ? {} : { home: typeof home === "boolean" ? { supported: true, enabled: home, active: options.homeActiveValues ? options.homeActiveValues[ids.indexOf(id)] : home } : { supported: home === "unknown", message: home === "unsupported" ? "当前小新版本不支持主屏设置" : "未读取到持久主屏规则" } }) };
  }));
  const preflight = vi.fn(async (_project: string, input: ScreenOperationInput, patches: Record<string, ScreenAppConfigPatch>) => input.targetIds.map(id => ({
    screenId: id, name: id, ip: "192.0.2.1", state: "ready" as const, reason: patches[id] ? "检查通过" : "缺少修改内容",
  })));
  const saveDraft = vi.fn<(projectId: string, draft: ScreenAppConfigDraft | null) => Promise<void>>(async () => {});
  let sourceDraft: ScreenAppConfigDraft | undefined;
  Object.assign(adapter, { readAppConfig: read, preflightAppConfig: preflight, saveAppConfigDraft: saveDraft });
  if (options.restoreDraft) {
    const rows = await read(store.projectId, ids);
    read.mockClear();
    sourceDraft = {
        environment: options.draftSwitch ? "test" : "pre", switchEnvironment: Boolean(options.draftSwitch), readyMode: "keep",
        homeMode: options.draftHomeMode,
        homeActivation: options.draftHomeActivation,
        ensureDeviceOwner: options.draftEnsureDeviceOwner,
        kioskMode: options.draftKioskMode,
        voiceMode: options.draftVoiceMode,
        voiceEdits: options.draftVoiceEdits,
        edits: { otaUrl: { mode: "keep", value: "" }, wsUrl: { mode: "set", value: "wss://draft.example.org" }, h5Url: { mode: "keep", value: "" } },
        names: Object.fromEntries(ids.map(id => [id, { mode: "keep", value: "" }])),
        targetIds: ids, includedIds: ids, rows,
    };
    Object.assign(adapter, { loadAppConfigDraft: vi.fn(async () => sourceDraft) });
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
  const closeVoice = async () => {
    if (wrapper.findAllComponents(NModal).some(modal => modal.props("title") === "语音配置" && modal.props("show"))) { await button("完成").trigger("click"); await flushPromises(); }
  };
  const openVoice = async () => {
    if (!wrapper.findAllComponents(NModal).some(modal => modal.props("title") === "语音配置" && modal.props("show"))) { await wrapper.get('button[aria-label="语音配置"]').trigger("click"); await flushPromises(); }
  };
  const closeDetail = async () => {
    const modal = wrapper.findAllComponents(NModal).find(item => String(item.vm.$attrs['aria-label']).includes("配置详情") && item.props("show"));
    if (modal) { modal.vm.$emit("update:show", false); await flushPromises(); }
  };
  const click = async (text: string) => { if (text !== "完成") await closeVoice(); await button(text).trigger("click"); await flushPromises(); };
  const clickLabel = async (label: string) => { if (label !== "语音配置") await closeVoice(); await wrapper.get(`button[aria-label="${label}"]`).trigger("click"); await flushPromises(); };
  const select = async (label: string, value: string | number) => {
    if (label === "选择环境") await closeVoice(); else await openVoice();
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
  const input = async (label: string, value: string) => { await closeVoice(); inputControl(label).vm.$emit("update:value", value); await flushPromises(); };
  const number = async (label: string, value: number | null) => {
    await openVoice();
    const control = wrapper.findAllComponents(NInputNumber).find(component => component.attributes("aria-label") === label);
    expect(control).toBeDefined(); control!.vm.$emit("update:value", value); await flushPromises();
  };
  const checkbox = async (label: string, value: boolean) => {
    if (label === "语音引擎随应用启动") await openVoice(); else await closeVoice();
    const control = wrapper.findAllComponents(NCheckbox).find(component => component.attributes("aria-label") === label || component.text() === label);
    expect(control).toBeDefined();
    control!.vm.$emit("update:checked", value);
    await flushPromises();
  };
  const changePage = async (page: number) => { wrapper.getComponent(NPagination).vm.$emit("update:page", page); await flushPromises(); };
  return {
    wrapper, store, ids, read, preflight, execute, saveDraft, sourceDraft, step, button, click, clickLabel, select, input, inputControl, checkbox, number, openVoice, closeVoice, closeDetail, changePage,
    dispose() { wrapper.unmount(); store.stop(); adapter.dispose(); },
  };
}

async function deferConfigReads(view: Awaited<ReturnType<typeof render>>) {
  const original = view.read.getMockImplementation()!;
  const expected = new Map(await Promise.all(view.ids.map(async id => [id, await original(view.store.projectId, [id])] as const)));
  const pending = new Map<string, { resolve: (rows: ScreenAppConfigRead[]) => void; reject: (error: Error) => void }>();
  let peak = 0;
  view.read.mockImplementation((_project, targets) => new Promise((resolve, reject) => {
    expect(targets).toHaveLength(1);
    pending.set(targets[0]!, { resolve, reject }); peak = Math.max(peak, pending.size);
  }));
  const complete = async (id: string, rows = expected.get(id)!) => {
    const request = pending.get(id); expect(request).toBeDefined(); pending.delete(id); request!.resolve(rows); await flushPromises();
  };
  return {
    pending, expected, complete, peak: () => peak,
    useOriginal: () => view.read.mockImplementation(original),
    async fail(id: string, message: string) { const request = pending.get(id); expect(request).toBeDefined(); pending.delete(id); request!.reject(new Error(message)); await flushPromises(); },
    async finish() { while (pending.size) await complete(pending.keys().next().value!); },
  };
}

describe("小新配置并发读取", () => {
  it("最多三个单屏请求，乱序完成逐台更新且按原选屏顺序提交，完成前不能检查", async () => {
    const view = await render({ count: 6 }), reads = await deferConfigReads(view);
    try {
      await view.click("读取 6 台配置");
      expect(view.read.mock.calls.map(call => call[1])).toEqual(view.ids.slice(0, 3).map(id => [id]));
      expect(view.wrapper.get('.screen-config-read-progress').text()).toContain("正在读取 3 台已完成 0/6 台");
      expect(view.wrapper.findAll('.screen-config-status').filter(row => row.text() === '正在读取')).toHaveLength(3);
      await reads.complete(view.ids[2]!);
      expect(view.read.mock.calls.at(-1)![1]).toEqual([view.ids[3]]);
      expect(view.wrapper.get(`[data-screen-id="${view.ids[2]}"]`).text()).toContain("读取成功");
      expect(view.wrapper.get('.screen-config-read-progress').text()).toContain("已完成 1/6 台");
      expect(view.button("检查修改内容").attributes('disabled')).toBeDefined();
      for (const index of [1, 3, 5, 0, 4]) await reads.complete(view.ids[index]!);
      expect(reads.peak()).toBe(3);
      expect(view.read.mock.calls.map(call => call[1])).toEqual(view.ids.map(id => [id]));
      expect(view.wrapper.findAll('[data-screen-id]').map(row => row.attributes('data-screen-id'))).toEqual(view.ids);
      expect(view.wrapper.findAll('.screen-config-status').filter(row => row.text() === '读取成功')).toHaveLength(6);
      expect(view.wrapper.find('.screen-config-read-progress').exists()).toBe(false);
      await view.input("WebSocket(可选)", "wss://new.example.org"); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids);
      expect(view.execute).not.toHaveBeenCalled();
    } finally { await reads.finish(); view.dispose(); }
  });

  it("单台抛错、失败返回或错屏结果各自归位，不阻断后续读取或混入修改目标", async () => {
    const view = await render({ count: 5 }), reads = await deferConfigReads(view);
    try {
      await view.click("读取 5 台配置");
      await reads.fail(view.ids[0]!, "第一台连接超时");
      await reads.complete(view.ids[1]!, [{ ...reads.expected.get(view.ids[1]!)![0]!, config: null, message: "第二台版本不支持" }]);
      await reads.complete(view.ids[2]!, reads.expected.get(view.ids[4]!)!);
      await reads.complete(view.ids[4]!); await reads.complete(view.ids[3]!);
      expect(view.read).toHaveBeenCalledTimes(5); expect(reads.peak()).toBe(3);
      for (const [index, message] of ["第一台连接超时", "第二台版本不支持", "读取结果与目标屏不一致"].entries()) {
        expect(view.wrapper.get(`[data-screen-id="${view.ids[index]}"]`).text()).toContain(message);
      }
      expect(view.wrapper.findAll('.screen-config-status').filter(row => row.text().includes('读取失败'))).toHaveLength(3);
      await view.input("WebSocket(可选)", "wss://new.example.org"); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids.slice(3));
      expect(view.execute).not.toHaveBeenCalled();
    } finally { await reads.finish(); view.dispose(); }
  });

  it("切项目后停止旧批次派发，迟到响应不能回填或结束新项目读取状态", async () => {
    const view = await render({ count: 6 }), reads = await deferConfigReads(view);
    try {
      await view.click("读取 6 台配置");
      await view.store.bindProject("config-ui-next"); await flushPromises();
      expect(view.step.value).toBe(0);
      const newId = view.store.snapshot.screens[0]!.id;
      view.store.selectedIds = [newId]; reads.useOriginal(); await flushPromises();
      await view.click("读取 1 台配置");
      await reads.finish();
      expect(view.read).toHaveBeenCalledTimes(4);
      expect(view.wrapper.findAll('[data-screen-id]').map(row => row.attributes('data-screen-id'))).toEqual([newId]);
      expect(view.wrapper.get(`[data-screen-id="${newId}"]`).text()).toContain("读取成功");
      expect(view.wrapper.text()).not.toContain("第一台连接超时");
    } finally { await reads.finish(); view.dispose(); }
  });

  it("重新选屏后旧批次不再派发或回填，新批次只读取重新选择的目标", async () => {
    const view = await render({ count: 6 }), reads = await deferConfigReads(view);
    try {
      await view.click("读取 6 台配置");
      view.store.snapshot.screens[5]!.size = "4"; await flushPromises();
      view.wrapper.findAllComponents(NButton).find(button => button.text() === '重新选屏')!.vm.$emit('click'); await flushPromises();
      expect(view.step.value).toBe(0);
      view.store.selectedIds = [view.ids[5]!]; reads.useOriginal(); await flushPromises();
      await view.click("读取 1 台配置"); await reads.finish();
      expect(view.read).toHaveBeenCalledTimes(4);
      expect(view.read.mock.calls.at(-1)![1]).toEqual([view.ids[5]]);
      expect(view.wrapper.findAll('[data-screen-id]').map(row => row.attributes('data-screen-id'))).toEqual([view.ids[5]]);
      expect(view.wrapper.get(`[data-screen-id="${view.ids[5]}"]`).text()).toContain("读取成功");
    } finally { await reads.finish(); view.dispose(); }
  });

  it("卸载后只等待已发出的三个请求结束，不派发剩余目标或保存迟到草稿", async () => {
    const view = await render({ count: 6 }), reads = await deferConfigReads(view);
    await view.click("读取 6 台配置"); view.dispose();
    await reads.finish();
    expect(view.read).toHaveBeenCalledTimes(3);
    expect(view.saveDraft).not.toHaveBeenCalled();
  });
});

describe("OWNER与应用锁定", () => {
  const control = (view: Awaited<ReturnType<typeof render>>, label: string) => view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === label)!;

  it("已具备OWNER只读展示，布局保持H5、OWNER、主屏、锁定、语音及右侧重读", async () => {
    const view = await render({ count: 1, homeValues: [false], kioskValues: [false] });
    try {
      await view.click("读取 1 台配置");
      expect(view.wrapper.get('[data-testid="screen-config-owner"]').text()).toContain("已具备设备OWNER权限");
      expect(control(view, "设置小新为设备OWNER")).toBeUndefined();
      const text = view.wrapper.get('.screen-config-checks').text();
      const positions = ["检查H5地址", "已具备设备OWNER权限", "固定小新为主屏", "锁定应用", "语音配置"].map(label => text.indexOf(label));
      expect(positions.every((value, index) => value >= 0 && (index === 0 || value > positions[index - 1]!))).toBe(true);
      expect(view.wrapper.get('.screen-config-checks').element.nextElementSibling!.textContent).toContain("重新读取配置");
      expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });

  it("无OWNER时只登记单向设置意图，之后允许选择主屏和锁定并在详情计入三项", async () => {
    const view = await render({ count: 1, ownerValues: ["none"], homeValues: [false], kioskValues: [false] });
    try {
      await view.click("读取 1 台配置");
      expect(control(view, "固定小新为主屏").props('disabled')).toBe(true);
      expect(control(view, "锁定应用").props('disabled')).toBe(true);
      await view.checkbox("设置小新为设备OWNER", true);
      expect(view.execute).not.toHaveBeenCalled();
      expect(control(view, "固定小新为主屏").props('disabled')).toBe(false);
      await view.checkbox("固定小新为主屏", true); await view.checkbox("锁定应用", true);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[0]!]: { set: {}, clear: [], ensureDeviceOwner: true, homeEnabled: true, kioskEnabled: true } });
      expect(view.execute).not.toHaveBeenCalled();
      await view.clickLabel("查看 测试屏1 配置详情");
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe("待修改 3 项");
      expect(view.wrapper.get('[data-config-field="ensureDeviceOwner"]').text()).toBe("尚未设置设备OWNER → 设置小新为设备OWNER");
      expect(view.wrapper.get('[data-config-field="kioskEnabled"]').text()).toBe("关闭 → 开启并锁定");
    } finally { view.dispose(); }
  });

  it("取消未提交OWNER意图只移除字段，不发送false或依赖动作", async () => {
    const view = await render({ count: 1, sameEnvironment: true, ownerValues: ["none"], homeValues: [false], kioskValues: [false] });
    try {
      await view.click("读取 1 台配置"); await view.checkbox("设置小新为设备OWNER", true);
      await view.checkbox("固定小新为主屏", true); await view.checkbox("锁定应用", true);
      await view.checkbox("设置小新为设备OWNER", false);
      await view.input("WebSocket(可选)", "wss://new.example.org"); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: { 'environments.pre.wsUrl': 'wss://new.example.org' }, clear: [] });
    } finally { view.dispose(); }
  });

  it("混合已具备和未设置OWNER时仅对未设置屏发true，取消后已有权限仍半选保留", async () => {
    const view = await render({ count: 2, ownerValues: ["self", "none"], homeValues: [false, false], kioskValues: [false, false] });
    try {
      await view.click("读取 2 台配置");
      expect(control(view, "设置小新为设备OWNER").props('indeterminate')).toBe(true);
      await view.checkbox("设置小新为设备OWNER", true); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[1]!]: { set: {}, clear: [], ensureDeviceOwner: true } });
      await view.checkbox("设置小新为设备OWNER", false);
      expect(control(view, "设置小新为设备OWNER").props('indeterminate')).toBe(true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });

  it.each(["other", "unknown", "unsupported"] as const)("OWNER为%s时禁用依赖项，普通配置仍可修改", async owner => {
    const view = await render({ count: 1, ownerValues: [owner], homeValues: [false], kioskValues: [false] });
    try {
      await view.click("读取 1 台配置");
      for (const label of ["设置小新为设备OWNER", "固定小新为主屏", "锁定应用"]) expect(control(view, label).props('disabled')).toBe(true);
      expect(view.wrapper.get('[data-testid="screen-config-owner"]').attributes('title')).toMatch(/其他应用|未确认|升级/);
      await view.checkbox("设置小新为设备OWNER", true); await view.checkbox("锁定应用", true);
      await view.input("WebSocket(可选)", "wss://new.example.org"); await view.click("检查修改内容");
      const patch = view.preflight.mock.calls[0]![2][view.ids[0]!]!;
      expect(patch).not.toHaveProperty('ensureDeviceOwner'); expect(patch).not.toHaveProperty('kioskEnabled');
    } finally { view.dispose(); }
  });

  it("明确缺少锁定保存键时默认提出开启目标，显示橙色差异且单独设置不重启", async () => {
    const view = await render({ count: 1, kioskValues: ["missing"] });
    try {
      await view.click("读取 1 台配置");
      expect(control(view, "锁定应用").props('checked')).toBe(true);
      expect(view.wrapper.get('[data-testid="screen-config-kiosk"]').classes()).toContain('is-edited');
      expect(view.wrapper.text()).toContain("本次修改 1 台 · 预计重启 0 台");
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[0]!]: { set: {}, clear: [], kioskEnabled: true } });
      await view.clickLabel("查看 测试屏1 配置详情");
      expect(view.wrapper.get('[data-config-field="kioskEnabled"]').text()).toBe("尚未保存 → 开启并锁定");
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe("待修改 1 项");
    } finally { view.dispose(); }
  });

  it("恢复草稿中的旧缺键快照不能在重新读取前假定默认开启", async () => {
    const view = await render({ count: 1, kioskValues: ["missing"], restoreDraft: true });
    try {
      await flushPromises(); await view.click("继续本机未提交修改");
      expect(control(view, "锁定应用").props('checked')).toBe(false);
      expect(control(view, "锁定应用").props('indeterminate')).toBe(true);
      expect(control(view, "锁定应用").props('disabled')).toBe(true);
      expect(view.wrapper.get('[data-testid="screen-config-kiosk"]').classes()).not.toContain('is-edited');
      await view.click("重新读取配置");
      expect(control(view, "锁定应用").props('checked')).toBe(true);
      expect(view.wrapper.get('[data-testid="screen-config-kiosk"]').classes()).toContain('is-edited');
      expect(view.execute).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it.each([true, false])("锁定已有保存值%s且运行一致时照实显示，初读不自动提交", async enabled => {
    const view = await render({ count: 1, kioskValues: [enabled] });
    try {
      await view.click("读取 1 台配置");
      expect(control(view, "锁定应用").props('checked')).toBe(enabled);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.wrapper.get('[data-testid="screen-config-kiosk"]').classes()).not.toContain('is-edited');
    } finally { view.dispose(); }
  });

  it.each([true, false])("锁定已保存%s但运行相反时确认目标使其一致，读回一致后消除差异", async enabled => {
    const active = [!enabled];
    const view = await render({ count: 1, kioskValues: [enabled], kioskActiveValues: active });
    try {
      await view.click("读取 1 台配置");
      expect(view.execute).not.toHaveBeenCalled();
      expect(control(view, "锁定应用").props('checked')).toBe(enabled);
      expect(view.wrapper.text()).toContain("本次修改 1 台 · 预计重启 0 台");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: {}, clear: [], kioskEnabled: enabled });
      await view.clickLabel("查看 测试屏1 配置详情");
      expect(view.wrapper.get('[data-config-field="kioskEnabled"]').text()).toBe(enabled ? "已开启，未锁定 → 开启并锁定" : "已关闭，仍锁定 → 关闭并解锁");
      await view.closeDetail(); active[0] = enabled; await view.click("重新读取配置");
      expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });

  it.each(["unknown", "unsupported"] as const)("锁定能力%s不能假设默认开启，运行未知也不伪装成未锁定", async value => {
    const view = await render({ count: 1, kioskValues: [value] });
    try {
      await view.click("读取 1 台配置");
      expect(control(view, "锁定应用").props('disabled')).toBe(true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.wrapper.get('[data-testid="screen-config-kiosk"]').attributes('title')).toMatch(/升级|未确认/);
    } finally { view.dispose(); }
  });

  it("锁定保存值已知但active未知时不产生自动目标", async () => {
    const view = await render({ count: 1, kioskValues: [true], kioskActiveValues: [null] });
    try {
      await view.click("读取 1 台配置");
      expect(control(view, "锁定应用").props('checked')).toBe(true);
      expect(control(view, "锁定应用").props('disabled')).toBe(true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });

  it("锁定混合值半选保留、全部开、全部关循环只提交实际差异", async () => {
    const view = await render({ count: 2, kioskValues: [true, false] });
    try {
      await view.click("读取 2 台配置"); expect(control(view, "锁定应用").props('indeterminate')).toBe(true);
      await view.checkbox("锁定应用", true); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[1]!]: { set: {}, clear: [], kioskEnabled: true } });
      await view.checkbox("锁定应用", false); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[1]![2]).toEqual({ [view.ids[0]!]: { set: {}, clear: [], kioskEnabled: false } });
      await view.checkbox("锁定应用", true);
      expect(control(view, "锁定应用").props('indeterminate')).toBe(true); expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });

  it("当前已锁定时激活HOME须明确关闭锁定，允许同批先解锁后激活", async () => {
    const view = await render({ count: 1, homeValues: [false], kioskValues: [true] });
    try {
      await view.click("读取 1 台配置"); await view.checkbox("固定小新为主屏", true);
      expect(view.button("检查修改内容").attributes('disabled')).toBeDefined();
      expect(view.wrapper.get('[data-testid="screen-config-unlock-required"]').text()).toContain("后台会先解锁再激活主屏");
      await view.checkbox("锁定应用", false);
      expect(view.wrapper.find('[data-testid="screen-config-unlock-required"]').exists()).toBe(false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[0]!]: { set: {}, clear: [], homeEnabled: true, kioskEnabled: false } });
    } finally { view.dispose(); }
  });

  it("保存为开启但当前未锁定时激活HOME同样须明确选择关闭", async () => {
    const view = await render({ count: 1, homeValues: [false], kioskValues: [true], kioskActiveValues: [false] });
    try {
      await view.click("读取 1 台配置"); await view.checkbox("固定小新为主屏", true);
      expect(view.button("检查修改内容").attributes('disabled')).toBeDefined();
      await view.checkbox("锁定应用", false); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: {}, clear: [], homeEnabled: true, kioskEnabled: false });
    } finally { view.dispose(); }
  });

  it("没有HOME动作的普通环境配置重启不被锁定状态阻断", async () => {
    const view = await render({ count: 1, sameEnvironment: true, homeValues: [true], homeActiveValues: [false], kioskValues: [true] });
    try {
      await view.click("读取 1 台配置"); await view.input("WebSocket(可选)", "wss://new.example.org");
      expect(view.wrapper.find('[data-testid="screen-config-unlock-required"]').exists()).toBe(false);
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: { 'environments.pre.wsUrl': 'wss://new.example.org' }, clear: [] });
      expect(view.wrapper.text()).toContain("预计重启 1 台");
    } finally { view.dispose(); }
  });

  it("已作为当前HOME但尚未持久固定时只设置规则，不因锁定要求重复激活", async () => {
    const view = await render({ count: 1, homeValues: [false], homeActiveValues: [true], kioskValues: [true] });
    try {
      await view.click("读取 1 台配置"); await view.checkbox("固定小新为主屏", true);
      expect(view.wrapper.find('[data-testid="screen-config-unlock-required"]').exists()).toBe(false);
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: {}, clear: [], homeEnabled: true });
    } finally { view.dispose(); }
  });

  it("固定当前HOME并同时修改需重启配置时提示页面重开，但不额外增加重启数量", async () => {
    const view = await render({ count: 1, sameEnvironment: true, homeValues: [false], homeActiveValues: [true], kioskValues: [false] });
    try {
      await view.click("读取 1 台配置"); await view.checkbox("固定小新为主屏", true);
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(false);
      await view.input("WebSocket(可选)", "wss://new.example.org");
      expect(view.wrapper.get('[data-testid="screen-config-home-impact"]').text()).toBe("切到小新主屏可能重新打开页面，当前语音对话会结束。");
      expect(view.wrapper.text()).toContain("本次修改 1 台 · 预计重启 1 台");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: { 'environments.pre.wsUrl': 'wss://new.example.org' }, clear: [], homeEnabled: true });
      expect(view.execute).not.toHaveBeenCalled();
      await view.checkbox("固定小新为主屏", false);
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(false);
      expect(view.wrapper.text()).toContain("预计重启 1 台");
    } finally { view.dispose(); }
  });

  it("OWNER和锁定草稿必须重读，已取得OWNER后不再发设置意图，切项目清除旧目标", async () => {
    const owners: ("self" | "none")[] = ["none"];
    const view = await render({ count: 1, ownerValues: owners, kioskValues: [true], restoreDraft: true, draftEnsureDeviceOwner: true, draftKioskMode: "off" });
    try {
      await flushPromises(); await view.click("继续本机未提交修改");
      expect(control(view, "锁定应用").props('disabled')).toBe(true); expect(view.button("检查修改内容").attributes('disabled')).toBeDefined();
      await view.click("重新读取配置"); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toMatchObject({ ensureDeviceOwner: true, kioskEnabled: false });
      owners[0] = "self"; await view.click("重新读取配置"); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[1]![2][view.ids[0]!]).not.toHaveProperty('ensureDeviceOwner');
      expect(view.wrapper.get('[data-testid="screen-config-owner"]').text()).toContain("已具备设备OWNER权限");
      await view.store.bindProject("config-owner-next"); await flushPromises(); expect(view.step.value).toBe(0);
      view.store.selectedIds = [view.store.snapshot.screens[0]!.id]; await flushPromises();
      view.read.mockImplementationOnce(async (_project, targets) => targets.map(screenId => ({ screenId, readAt: '2026-10-23T00:00:00Z', config: sampleConfiguration(), capabilities: null, message: '已读取', owner: { state: 'none', message: '尚未设置' }, kiosk: { supported: true, configured: true, enabled: true, active: true, message: '已读取' } })));
      await view.click("读取 1 台配置");
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(control(view, "设置小新为设备OWNER").props('checked')).toBe(false);
      await view.checkbox("设置小新为设备OWNER", true); await view.click("检查修改内容");
      expect(Object.values(view.preflight.mock.calls.at(-1)![2])).toEqual([{ set: {}, clear: [], ensureDeviceOwner: true }]);
    } finally { view.dispose(); }
  });
});

describe("小新配置页面", () => {
  it.each(['4', '10'] as const)("同为%s寸的两台屏可以完整读取", async size => {
    const view = await render({ count: 2, sizes: [size, size] });
    try {
      expect(view.button('读取 2 台配置').attributes('disabled')).toBeUndefined();
      await view.click('读取 2 台配置');
      expect(view.read.mock.calls.map(call => call[1])).toEqual(view.ids.map(id => [id]));
      expect(view.step.value).toBe(1);
      expect(view.wrapper.find('[data-testid="screen-config-size-warning"]').exists()).toBe(false);
    } finally { view.dispose(); }
  });

  it.each([
    { sizes: ['4', '10'] as ScreenSize[], message: '4 寸屏和 10 寸屏不能同时修改配置，请分批选择。' },
    { sizes: ['10', 'unknown'] as ScreenSize[], message: '请先确认所选屏尺寸' },
  ])("尺寸组合$sizes禁用读取，直接触发处理函数也不能绕过", async ({ sizes, message }) => {
    const view = await render({ count: 2, sizes });
    try {
      expect(view.wrapper.get('[data-testid="screen-config-size-warning"]').text()).toBe(message);
      expect(view.button('读取 2 台配置').attributes('disabled')).toBeDefined();
      view.wrapper.findAllComponents(NButton).find(button => button.text() === '读取 2 台配置')!.vm.$emit('click');
      await flushPromises();
      expect(view.read).not.toHaveBeenCalled(); expect(view.step.value).toBe(0);
      expect(view.saveDraft).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it("筛选隐藏不同尺寸不能绕过，清空后只选同尺寸恢复读取", async () => {
    const view = await render({ count: 2, sizes: ['10', '4'] });
    try {
      const picker = view.wrapper.getComponent(ScreenTargetPicker);
      picker.findAllComponents(NSelect).find(select => select.attributes('aria-label') === '运维屏尺寸')!.vm.$emit('update:value', '10');
      await flushPromises();
      expect(picker.findAll('.screen-target-row.selected')).toHaveLength(1);
      expect(view.store.selectedIds).toEqual(view.ids);
      expect(view.wrapper.get('[data-testid="screen-config-size-warning"]').text()).toContain('不能同时修改配置');
      expect(view.button('读取 2 台配置').attributes('disabled')).toBeDefined();
      await view.click('清空选择');
      expect(view.wrapper.find('[data-testid="screen-config-size-warning"]').exists()).toBe(false);
      expect(view.button('读取 0 台配置').attributes('disabled')).toBeDefined();
      await view.checkbox('运维选择 测试屏1', true);
      expect(view.button('读取 1 台配置').attributes('disabled')).toBeUndefined();
      await view.click('读取 1 台配置');
      expect(view.read).toHaveBeenCalledExactlyOnceWith(view.store.projectId, [view.ids[0]]);
    } finally { view.dispose(); }
  });

  it("目标记录缺失按尺寸未确认处理，不把找不到的已选屏忽略", async () => {
    const view = await render({ count: 2 });
    try {
      view.store.snapshot.screens = view.store.snapshot.screens.slice(0, 1); await flushPromises();
      expect(view.store.selectedIds).toHaveLength(2);
      expect(view.wrapper.get('[data-testid="screen-config-size-warning"]').text()).toBe('请先确认所选屏尺寸');
      expect(view.button('读取 2 台配置').attributes('disabled')).toBeDefined();
      expect(view.read).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it.each([['4', '10'], ['10', 'unknown']] as ScreenSize[][])("旧尺寸不合法草稿%s不能恢复，本次操作保留旧稿全部编辑", async (first, second) => {
    const voiceEdits: Record<string, AppConfigVoiceEdit> = { 'audioConfig.minSilenceDuration': { mode: 'set', value: 0.9 }, 'audioConfig.keywordsScore': { mode: 'set', value: 1.5 } };
    const view = await render({ count: 2, sizes: [first!, second!], restoreDraft: true, draftVoiceEdits: voiceEdits });
    try {
      const before = JSON.stringify(view.sourceDraft);
      await flushPromises(); await view.click('继续本机未提交修改');
      expect(view.step.value).toBe(0); expect(view.read).not.toHaveBeenCalled();
      expect(view.wrapper.text()).toContain('不能继续本批配置');
      expect(view.store.selectedIds).toEqual(view.ids);
      expect(JSON.stringify(view.sourceDraft)).toBe(before);
      expect(view.saveDraft).not.toHaveBeenCalled();
      view.store.selectedIds = [view.ids[0]!]; await flushPromises(); await view.click('继续本机未提交修改');
      expect(view.step.value).toBe(0); expect(view.read).not.toHaveBeenCalled();
      expect(JSON.stringify(view.sourceDraft)).toBe(before);
      expect(view.saveDraft).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it("同尺寸单台草稿的两项语音编辑在恢复重读后完整保留", async () => {
    const voiceEdits: Record<string, AppConfigVoiceEdit> = { 'audioConfig.minSilenceDuration': { mode: 'set', value: 0.9 }, 'audioConfig.keywordsScore': { mode: 'set', value: 1.5 } };
    const view = await render({ count: 1, sizes: ['10'], schemas: ['default'], restoreDraft: true, draftVoiceEdits: voiceEdits });
    try {
      await flushPromises(); await view.click('继续本机未提交修改');
      expect(view.step.value).toBe(1); await view.click('重新读取配置'); await view.click('检查修改内容');
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!].set).toMatchObject({ 'audioConfig.minSilenceDuration': 0.9, 'audioConfig.keywordsScore': 1.5 });
      await vi.waitFor(() => expect(view.saveDraft).toHaveBeenCalled());
      expect(view.saveDraft.mock.calls.at(-1)![1]?.voiceEdits).toEqual(voiceEdits);
    } finally { view.dispose(); }
  });

  it("旧草稿中已删除目标仍按原规则过滤，剩余同尺寸目标可恢复", async () => {
    const view = await render({ count: 2, sizes: ['10', 'unknown'], restoreDraft: true });
    try {
      await flushPromises();
      view.store.snapshot.screens = view.store.snapshot.screens.slice(0, 1); await flushPromises();
      await view.click('继续本机未提交修改');
      expect(view.step.value).toBe(1); expect(view.store.selectedIds).toEqual([view.ids[0]]);
      expect(view.inputControl('WebSocket(可选)').props('value')).toBe('wss://draft.example.org');
      await view.click('重新读取配置');
      expect(view.read).toHaveBeenCalledExactlyOnceWith(view.store.projectId, [view.ids[0]]);
    } finally { view.dispose(); }
  });

  it.each([false, true])("读取中尺寸变化后迟到结果不会通过，尺寸再改回=%s仍需完整重读", async returnToOriginal => {
    const view = await render({ count: 2 });
    const reads = await deferConfigReads(view);
    try {
      await view.click('读取 2 台配置');
      view.store.snapshot.screens[1]!.size = '4';
      if (returnToOriginal) view.store.snapshot.screens[1]!.size = '10';
      await flushPromises(); await reads.finish(); reads.useOriginal();
      expect(view.read).toHaveBeenCalledTimes(2);
      expect(view.wrapper.text()).not.toContain('读取成功');
      expect(view.button('检查修改内容').attributes('disabled')).toBeDefined();
      if (!returnToOriginal) {
        expect(view.button('重新读取配置').attributes('disabled')).toBeDefined();
        await view.click('重新选屏'); expect(view.step.value).toBe(0);
        await view.click('清空选择'); await view.checkbox('运维选择 测试屏1', true); await view.click('读取 1 台配置');
        expect(view.read.mock.calls.at(-1)![1]).toEqual([view.ids[0]]);
      } else {
        await view.click('重新读取配置');
        expect(view.read.mock.calls.slice(2).map(call => call[1])).toEqual(view.ids.map(id => [id]));
      }
    } finally { await reads.finish(); view.dispose(); }
  });

  it("编辑页排除不同尺寸不能绕过重新读取完整范围，且不会丢掉已编辑值", async () => {
    const view = await render({ count: 2 });
    try {
      await view.click('读取 2 台配置'); await view.input('WebSocket(可选)', 'wss://edited.example.org');
      view.store.snapshot.screens[1]!.size = '4'; await flushPromises();
      expect(view.wrapper.findAll('.n-alert')).toHaveLength(0);
      await view.checkbox('选择 测试屏2', false);
      expect(view.button('重新读取配置').attributes('disabled')).toBeDefined();
      view.wrapper.findAllComponents(NButton).find(button => button.text() === '重新读取配置')!.vm.$emit('click'); await flushPromises();
      expect(view.read).toHaveBeenCalledTimes(2);
      expect(view.inputControl('WebSocket(可选)').props('value')).toBe('wss://edited.example.org');
      expect(view.button('检查修改内容').attributes('disabled')).toBeDefined();
      expect(view.preflight).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it("预检期间尺寸变化再恢复，迟到预检结果不能重新启用确认提交", async () => {
    const view = await render({ count: 2 });
    const preflight = view.preflight.getMockImplementation()!;
    let finish!: (items: Awaited<ReturnType<typeof preflight>>) => void;
    view.preflight.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    try {
      await view.click('读取 2 台配置'); await view.input('WebSocket(可选)', 'wss://edited.example.org'); await view.click('检查修改内容');
      const request = view.preflight.mock.calls[0]!;
      const items = await preflight(...request);
      view.store.snapshot.screens[1]!.size = '4'; view.store.snapshot.screens[1]!.size = '10';
      await flushPromises(); finish(items); await flushPromises();
      expect(view.wrapper.findAll('button').some(button => button.text().includes('确认修改'))).toBe(false);
      expect(view.button('检查修改内容').attributes('disabled')).toBeDefined();
      expect(view.execute).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it.each(['4', 'unknown'] as const)("已预检后尺寸改为%s会使旧确认失效，旧点击处理也不能提交", async size => {
    const view = await render({ count: 1 });
    try {
      await view.click('读取 1 台配置'); await view.input('WebSocket(可选)', 'wss://edited.example.org'); await view.click('检查修改内容');
      const oldSubmit = view.wrapper.findAllComponents(NButton).find(button => button.text() === '确认修改 1 台')!;
      expect(oldSubmit).toBeDefined();
      view.store.snapshot.screens[0]!.size = size; await flushPromises();
      oldSubmit.vm.$emit('click'); await flushPromises();
      expect(view.execute).not.toHaveBeenCalled();
      expect(view.wrapper.findAll('button').some(button => button.text().includes('确认修改'))).toBe(false);
      expect(view.button('检查修改内容').attributes('disabled')).toBeDefined();
      expect(view.inputControl('WebSocket(可选)').props('value')).toBe('wss://edited.example.org');
    } finally { view.dispose(); }
  });

  it("详情音频变化直接在对应行比较，改回原值逐项恢复且标题零项不显示", async () => {
    const view = await render({ count: 1, schemas: ['default'] });
    try {
      await view.click('读取 1 台配置');
      await view.number('静音判定时长', 0.9); await view.number('热词增强分数', 1.5);
      await view.clickLabel('查看 测试屏1 配置详情');
      const value = (field: string) => view.wrapper.get(`.screen-config-current [data-config-field="${field}"]`);
      expect(view.wrapper.get('.screen-config-detail-title').text()).toContain('测试屏1 · 配置详情');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 2 项');
      expect(value('audioConfig.minSilenceDuration').text()).toBe('0.6 秒 → 0.9 秒');
      expect(value('audioConfig.keywordsScore').text()).toBe('2 → 1.5');
      expect(value('audioConfig.vadThreshold').text()).toBe('0.8');
      expect(view.wrapper.get('.screen-config-current').findAll('dd.is-edited')).toHaveLength(2);
      expect(view.wrapper.get('.screen-config-detail-body').find('table').exists()).toBe(false);
      await view.closeDetail(); await view.number('静音判定时长', 0.6); await view.clickLabel('查看 测试屏1 配置详情');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 1 项');
      expect(value('audioConfig.minSilenceDuration').text()).toBe('0.6 秒');
      expect(value('audioConfig.minSilenceDuration').classes()).not.toContain('is-edited');
      await view.closeDetail(); await view.number('热词增强分数', 2); await view.clickLabel('查看 测试屏1 配置详情');
      expect(view.wrapper.find('.screen-config-detail-pending').exists()).toBe(false);
      expect(view.wrapper.get('.screen-config-current').findAll('dd.is-edited')).toHaveLength(0);
      expect(view.wrapper.get('.screen-config-current').text()).not.toContain('→');
      expect(view.preflight).not.toHaveBeenCalled(); expect(view.execute).not.toHaveBeenCalled();
    } finally { view.dispose(); }
  });

  it("详情只计算本屏实际变化，多屏统一采样率不把原值相同屏标为待改", async () => {
    const view = await render({ count: 2, schemas: ['default', 'small'] });
    try {
      await view.click('读取 2 台配置'); await view.select('麦克风采样率', 16000);
      await view.clickLabel('查看 测试屏1 配置详情');
      expect(view.wrapper.find('.screen-config-detail-pending').exists()).toBe(false);
      expect(view.wrapper.get('[data-config-field="audioConfig.micSampleRate"]').text()).not.toContain('→');
      await view.closeDetail(); await view.clickLabel('查看 测试屏2 配置详情');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 1 项');
      const sampling = view.wrapper.get('[data-config-field="audioConfig.micSampleRate"]');
      expect(sampling.text()).toMatch(/11025.*→.*16000/); expect(sampling.text()).toContain('Hz');
      expect(sampling.classes()).toContain('is-edited');
      await view.closeDetail(); await view.click('检查修改内容');
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[1]!]: { set: { 'audioConfig.micSampleRate': 16000 }, clear: [] } });
    } finally { view.dispose(); }
  });

  it("详情覆盖环境地址开关名称与主屏的全部变化，不把保存后有效地址重复计数", async () => {
    const view = await render({ count: 1, sameEnvironment: true, voiceValues: [false], homeValues: [false] });
    try {
      await view.click('读取 1 台配置'); await view.select('选择环境', 'test');
      await view.input('OTA 地址', 'https://new.example.org/ota');
      await view.input('WebSocket(可选)', 'wss://new.example.org/ws');
      await view.input('H5地址(可选)', 'https://new.example.org/h5');
      await view.checkbox('H5检测', false); await view.checkbox('固定小新为主屏', true); await view.checkbox('语音引擎随应用启动', true);
      await view.clickLabel('查看 测试屏1 配置详情'); await view.input('测试屏1设备名', '新名称');
      const details = view.wrapper.get('.screen-config-detail-body');
      const value = (field: string) => details.get(`[data-config-field="${field}"]`).text();
      expect(value('environments.current')).toBe('预发环境 → 测试环境 · 查看测试环境配置');
      expect(value('environments.test.otaUrl')).toBe(`${originalOta} → https://new.example.org/ota`);
      expect(value('environments.test.wsUrl')).toBe(`${originalWs} → wss://new.example.org/ws`);
      expect(value('environments.test.h5Url')).toBe(`${originalH5} → https://new.example.org/h5`);
      expect(value('environments.test.h5ReadyCheckEnabled')).toBe('开启 → 关闭');
      expect(value('autoStartVoiceEngine')).toBe('关闭 → 开启');
      expect(value('homeEnabled')).toBe('未固定 → 固定并激活');
      expect(value('customDeviceName')).toBe(`${view.ids[0]} → 新名称`);
      expect(details.findAll('dd.is-edited')).toHaveLength(8);
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 8 项');
      const facts = Object.fromEntries(details.findAll('dt').map(label => [label.text(), label.element.nextElementSibling?.textContent]));
      expect(facts['语音使用值（手动设置）：']).toBe(originalWs);
      expect(facts['保存后语音使用（手动设置）：']).toBe('wss://new.example.org/ws');
      expect(details.findAll('.screen-config-name-field .n-input')).toHaveLength(1);
    } finally { view.dispose(); }
  });

  it("详情清空手动地址显示真实差异，服务端有效地址来源保持独立且还原后消失", async () => {
    const view = await render({ count: 1, sameEnvironment: true });
    const read = view.read.getMockImplementation()!;
    view.read.mockImplementationOnce(async (...args) => {
      const rows = await read(...args);
      rows[0]!.config!.environments.pre.otaWsUrl = 'wss://server.example.org/ws';
      rows[0]!.config!.environments.pre.otaH5Url = 'https://server.example.org/h5';
      return rows;
    });
    try {
      await view.click('读取 1 台配置'); await view.input('WebSocket(可选)', ''); await view.input('H5地址(可选)', '');
      await view.clickLabel('查看 测试屏1 配置详情');
      const details = view.wrapper.get('.screen-config-detail-body');
      const facts = Object.fromEntries(details.findAll('dt').map(label => [label.text(), label.element.nextElementSibling?.textContent]));
      expect(facts['WebSocket(可选)：']).toBe(`${originalWs} → 清空手动值`);
      expect(facts['H5地址(可选)：']).toBe(`${originalH5} → 清空手动值`);
      expect(facts['语音使用值（手动设置）：']).toBe(originalWs);
      expect(facts['保存后语音使用（服务端下发）：']).toBe('wss://server.example.org/ws');
      expect(facts['保存后网页使用（服务端下发）：']).toBe('https://server.example.org/h5');
      expect(details.findAll('dd.is-edited')).toHaveLength(2);
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 2 项');
      await view.closeDetail(); await view.input('WebSocket(可选)', originalWs); await view.input('H5地址(可选)', originalH5);
      await view.clickLabel('查看 测试屏1 配置详情');
      expect(view.wrapper.find('.screen-config-detail-pending').exists()).toBe(false);
      expect(view.wrapper.get('.screen-config-current').text()).not.toContain('→');
      expect(view.wrapper.get('.screen-config-current').text()).not.toContain('保存后语音使用');
    } finally { view.dispose(); }
  });

  it("附属切主屏勾选与持久属性独立，属性改回原值清除附属动作", async () => {
    const view = await render({ count: 1, homeValues: [true], homeActiveValues: [false] });
    try {
      await view.click("读取 1 台配置");
      const home = view.wrapper.get('[data-testid="screen-config-home"]');
      await view.checkbox("切到小新主屏", true);
      expect(home.findAll('.is-edited')).toHaveLength(1);
      expect(home.get('.is-edited').classes()).toContain('screen-config-home-activation');
      await view.checkbox("固定小新为主屏", false);
      expect(home.find('.screen-config-home-activation').exists()).toBe(false);
      expect(view.wrapper.text()).toContain("本次修改 1 台");
      await view.checkbox("固定小新为主屏", true);
      expect(home.findAll('.is-edited')).toHaveLength(0);
      expect(view.wrapper.findAllComponents(NCheckbox).find(control => control.attributes('aria-label') === '切到小新主屏')!.props('checked')).toBe(false);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      expect(view.wrapper.text()).not.toContain('撤销');
      expect(home.findAll('button')).toHaveLength(0);
    } finally { view.dispose(); }
  });

  it("附属切主屏动作仅提交未激活的屏，重读已生效后取消待提交", async () => {
    const options = { count: 2, homeValues: [true, true], homeActiveValues: [true, false] };
    const view = await render(options);
    try {
      await view.click("读取 2 台配置"); await view.checkbox("切到小新主屏", true); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[1]!]: { set: {}, clear: [], homeEnabled: true } });
      options.homeActiveValues[1] = true;
      await view.click("重新读取配置");
      expect(view.wrapper.find('.screen-config-home-activation').exists()).toBe(false);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      options.homeActiveValues[1] = false;
      await view.click("重新读取配置");
      expect(view.wrapper.findAllComponents(NCheckbox).find(control => control.attributes('aria-label') === '切到小新主屏')!.props('checked')).toBe(false);
    } finally { view.dispose(); }
  });

  it.each([true, undefined])("主屏附属动作草稿 %s 必须重新读取确认，旧草稿不会自动激活", async homeActivation => {
    const view = await render({ count: 1, homeValues: [true], homeActiveValues: [false], restoreDraft: true, draftHomeActivation: homeActivation });
    try {
      await flushPromises(); await view.click("继续本机未提交修改");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.click("重新读取配置"); await view.click("检查修改内容");
      const patch = view.preflight.mock.calls[0]![2][view.ids[0]!]!;
      if (homeActivation) expect(patch.homeEnabled).toBe(true); else expect(patch).not.toHaveProperty('homeEnabled');
      await vi.waitFor(() => expect(view.saveDraft).toHaveBeenCalled());
      expect(view.saveDraft.mock.calls.at(-1)![1]).toMatchObject({ homeActivation: Boolean(homeActivation) });
    } finally { view.dispose(); }
  });

  it("混合H5检测三态可恢复原值，修改色只跟随实际差异", async () => {
    const view = await render({ count: 3, readyValues: [true, false, false], sameEnvironment: true });
    try {
      await view.click("读取 3 台配置");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes('aria-label') === 'H5检测')!;
      expect(control.props('indeterminate')).toBe(true);
      await view.checkbox('H5检测', true); await view.click('检查修改内容');
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids.slice(1));
      await view.checkbox('H5检测', false);
      expect(view.wrapper.text()).toContain('本次修改 1 台');
      await view.checkbox('H5检测', true);
      expect(control.props('indeterminate')).toBe(true);
      expect(view.wrapper.text()).toContain('本次修改 0 台');
      expect(view.wrapper.get('.screen-config-checks').findAll('.is-edited')).toHaveLength(0);
    } finally { view.dispose(); }
  });

  it("混合空地址输入再清空恢复原值，统一清空复选框明确选择且可取消", async () => {
    const view = await render({ count: 2, sameEnvironment: true, wsValues: [null, originalWs] });
    try {
      await view.click('读取 2 台配置');
      const input = view.inputControl('WebSocket(可选)');
      expect(input.props('value')).toBe('');
      expect(input.props('placeholder')).toContain('多个值');
      await view.input('WebSocket(可选)', 'wss://new.example.org');
      expect(input.props('status')).toBe('warning');
      await view.input('WebSocket(可选)', '');
      expect(view.wrapper.text()).toContain('本次修改 0 台');
      expect(input.props('status')).toBeUndefined();
      await view.checkbox('统一清空WebSocket(可选)', true); await view.click('检查修改内容');
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[1]!]: { set: {}, clear: ['environments.pre.wsUrl'] } });
      await view.checkbox('统一清空WebSocket(可选)', false);
      expect(view.wrapper.text()).toContain('本次修改 0 台');
      await view.checkbox('统一清空WebSocket(可选)', true);
      await view.input('WebSocket(可选)', 'wss://new.example.org'); await view.click('检查修改内容');
      for (const patch of Object.values(view.preflight.mock.calls[1]![2])) expect(patch).toEqual({ set: { 'environments.pre.wsUrl': 'wss://new.example.org' }, clear: [] });
      expect(view.wrapper.text()).not.toContain('撤销');
    } finally { view.dispose(); }
  });

  it("地址名称改回读值恢复颜色且不提交，界面不保留撤销按钮", async () => {
    const view = await render({ count: 1, sameEnvironment: true });
    try {
      await view.click('读取 1 台配置');
      await view.input('WebSocket(可选)', 'wss://new.example.org');
      expect(view.inputControl('WebSocket(可选)').props('status')).toBe('warning');
      await view.input('WebSocket(可选)', originalWs);
      expect(view.inputControl('WebSocket(可选)').props('status')).toBeUndefined();
      await view.clickLabel('查看 测试屏1 配置详情');
      expect(view.wrapper.find('.screen-config-detail-pending').exists()).toBe(false);
      await view.input('测试屏1设备名', '临时名称');
      expect(view.inputControl('测试屏1设备名').props('status')).toBe('warning');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 1 项');
      expect(view.wrapper.get('[data-config-field="customDeviceName"]').text()).toBe(`${view.ids[0]} → 临时名称`);
      await view.input('测试屏1设备名', view.ids[0]!);
      expect(view.inputControl('测试屏1设备名').props('status')).toBeUndefined();
      expect(view.wrapper.find('.screen-config-detail-pending').exists()).toBe(false);
      expect(view.wrapper.find('[data-config-field="customDeviceName"]').exists()).toBe(false);
      expect(view.wrapper.get('.screen-config-detail-body').text()).not.toContain('→');
      expect(view.wrapper.text()).toContain('本次修改 0 台');
      expect(view.wrapper.text()).not.toContain('撤销');
    } finally { view.dispose(); }
  });

  it("真实Kotlin契约驱动九项语音浮层，修改单叶子不会覆盖不同采样率", async () => {
    const view = await render({ count: 2, schemas: ["default", "small"] });
    try {
      await view.click("读取 2 台配置");
      expect(view.wrapper.find('input[aria-label="语音引擎随应用启动"]').exists()).toBe(false);
      expect(view.wrapper.get('.screen-config-checks').text()).toContain("语音配置 >");
      await view.openVoice();
      expect(view.wrapper.findAll("[data-voice-field]")).toHaveLength(9);
      expect(view.wrapper.text()).toContain("默认值因设备而异");
      const sampling = view.wrapper.findAllComponents(NSelect).find(control => control.attributes("aria-label") === "麦克风采样率")!;
      expect(sampling.props("disabled")).toBe(false);
      expect(sampling.props("value")).toBeNull();
      expect(sampling.props("options")?.map(option => "value" in option ? option.value : undefined)).toEqual([8000, 11025, 16000, 22050, 44100, 48000]);
      expect(view.preflight).not.toHaveBeenCalled();
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      await view.number("静音判定时长", 0.9);
      await view.click("完成");
      expect(view.wrapper.get('[data-testid="screen-config-voice"]').classes()).toContain("is-edited");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual(Object.fromEntries(view.ids.map(id => [id, { set: { "audioConfig.minSilenceDuration": 0.9 }, clear: [] }])));
      expect(view.wrapper.text()).toContain("预计重启 2 台");
    } finally { view.dispose(); }
  });

  it("新增已声明语音字段按定义生成控件和标签，定义文案只作文本", async () => {
    const field: AppConfigFieldDefinition = { key: "audioConfig.futurePause", type: "number", label: "扩展停顿", group: "voice", section: "扩展参数", order: 99,
      writable: true, clearable: false, defaultValue: 0.3, min: 0, max: 3, step: 0.1, unit: "秒", description: "<b>按定义显示</b>", restartPolicy: "appRestart" };
    const view = await render({ count: 1, schemas: ["default"], definitions: [field], audioOverrides: [{ futurePause: 1.2 }] });
    try {
      await view.click("读取 1 台配置"); await view.openVoice();
      expect(view.wrapper.findAll("[data-voice-field]")).toHaveLength(10);
      const row = view.wrapper.get('[data-voice-field="audioConfig.futurePause"]');
      expect(row.text()).toContain("<b>按定义显示</b>");
      expect(row.find("b").exists()).toBe(false);
      await view.number("扩展停顿", 0.8); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toEqual({ set: { "audioConfig.futurePause": 0.8 }, clear: [] });
      await view.clickLabel("查看 测试屏1 配置详情");
      const details = view.wrapper.get('.screen-config-detail-body');
      const facts = Object.fromEntries(details.findAll('dt').map(label => [label.text(), label.element.nextElementSibling?.textContent]));
      expect(Object.keys(facts)).toEqual(expect.arrayContaining(['当前运行：', 'OTA 地址：', 'H5检测：', '固定小新为主屏：', '语音引擎随应用启动：', '扩展停顿：']));
      for (const definition of nativeFixture.default.capabilities.configSchema.fields as AppConfigFieldDefinition[]) if (definition.key.startsWith('audioConfig.')) expect(facts).toHaveProperty(`${definition.label}：`);
      expect(facts['扩展停顿：']).toBe('1.2 秒 → 0.8 秒');
      expect(facts['静音判定时长：']).toBe('0.6 秒');
      expect(details.find('table').exists()).toBe(false);
      expect(details.get('[data-config-field="audioConfig.futurePause"]').classes()).toContain('is-edited');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 1 项');
    } finally { view.dispose(); }
  });

  it("新旧APP混合只禁不支持的语音参数，移除旧屏后可按叶子提交", async () => {
    const view = await render({ count: 2, schemas: ["legacy", "default"], voiceValues: [false, false] });
    try {
      await view.click("读取 2 台配置"); await view.openVoice();
      expect(view.wrapper.findAllComponents(NCheckbox).find(control => control.attributes("aria-label") === "语音引擎随应用启动")!.props("disabled")).toBe(false);
      expect(view.wrapper.findAllComponents(NInputNumber).find(control => control.attributes("aria-label") === "静音判定时长")!.props("disabled")).toBe(true);
      await view.checkbox("选择 测试屏1", false); await view.number("静音判定时长", 0.9); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[1]!]: { set: { "audioConfig.minSilenceDuration": 0.9 }, clear: [] } });
    } finally { view.dispose(); }
  });

  it("当前越界值原样展示，聚焦失焦不clamp，用户改为合法值后才可提交", async () => {
    const view = await render({ count: 1, schemas: ["default"], audioOverrides: [{ minSilenceDuration: 0.1 }] });
    try {
      await view.click("读取 1 台配置"); await view.openVoice();
      const control = view.wrapper.findAllComponents(NInputNumber).find(item => item.attributes("aria-label") === "静音判定时长")!;
      expect(control.props("value")).toBe(0.1); expect(control.props("min")).toBeUndefined(); expect(control.props("max")).toBeUndefined();
      await control.find("input").trigger("focus"); await control.find("input").trigger("blur"); await flushPromises();
      expect(control.props("value")).toBe(0.1); expect(view.wrapper.text()).toContain("本次修改 0 台");
      await view.number("静音判定时长", 0.2);
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      expect(view.wrapper.text()).toContain("允许范围");
      await view.number("静音判定时长", 0.4); await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!].set).toEqual({ "audioConfig.minSilenceDuration": 0.4 });
    } finally { view.dispose(); }
  });

  it("语音新草稿重读后恢复叶子修改，达到目标恢复颜色；草稿不重复存schema", async () => {
    const options = { count: 1, schemas: ["default" as const], restoreDraft: true, audioOverrides: [{ minSilenceDuration: 0.6 }], draftVoiceEdits: { "audioConfig.minSilenceDuration": { mode: "set" as const, value: 0.9 } } };
    const view = await render(options);
    try {
      await flushPromises(); await view.click("继续本机未提交修改");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.click("重新读取配置"); await view.openVoice();
      expect(view.wrapper.findAllComponents(NInputNumber).find(control => control.attributes("aria-label") === "静音判定时长")!.props("value")).toBe(0.9);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!].set["audioConfig.minSilenceDuration"]).toBe(0.9);
      await vi.waitFor(() => expect(view.saveDraft).toHaveBeenCalled());
      const draft = view.saveDraft.mock.calls.at(-1)![1]!;
      expect(draft.rows[0]!.capabilities).not.toHaveProperty("configSchema");
      expect(draft.voiceEdits).toEqual({ "audioConfig.minSilenceDuration": { mode: "set", value: 0.9 } });
      options.audioOverrides[0]!.minSilenceDuration = 0.9;
      await view.click("重新读取配置");
      expect(view.wrapper.get('[data-testid="screen-config-voice"]').classes()).not.toContain("is-edited");
      await view.openVoice(); expect(view.wrapper.findAll("[data-voice-field]")).toHaveLength(9);
    } finally { view.dispose(); }
  });

  it("语音数字改回当前值或清空输入能取消修改，0.62不被step截断", async () => {
    const view = await render({ count: 1, schemas: ["default"], audioOverrides: [{ minSilenceDuration: 0.62 }] });
    try {
      await view.click("读取 1 台配置"); await view.number("静音判定时长", 0.8);
      expect(view.wrapper.get('[data-testid="screen-config-voice"]').classes()).toContain("is-edited");
      await view.number("静音判定时长", 0.62);
      expect(view.wrapper.get('[data-testid="screen-config-voice"]').classes()).not.toContain("is-edited");
      await view.number("静音判定时长", 0.9); await view.number("静音判定时长", null);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.wrapper.findAllComponents(NInputNumber).find(control => control.attributes("aria-label") === "静音判定时长")!.props("value")).toBe(0.62);
    } finally { view.dispose(); }
  });

  it.each([true, false])("语音启动读取实际值%s后保持原设置，重新读取不产生修改", async enabled => {
    const view = await render({ count: 1, voiceValues: [enabled] });
    try {
      await view.click("读取 1 台配置");
      await view.openVoice();
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "语音引擎随应用启动")!;
      expect(control.props("checked")).toBe(enabled);
      expect(control.props("disabled")).toBe(false);
      expect(control.props("indeterminate")).toBe(false);
      await view.click("重新读取配置");
      expect(view.wrapper.text()).toContain("本次修改 0 台 · 预计重启 0 台");
      await view.input("WebSocket(可选)", "wss://other.example.org");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]!.set).not.toHaveProperty("autoStartVoiceEngine");
    } finally { view.dispose(); }
  });

  it.each([true, false])("语音启动显式修改为%s走原预检提交，顶层字段变化只计一次重启", async enabled => {
    const view = await render({ count: 1, voiceValues: [!enabled] });
    try {
      await view.click("读取 1 台配置");
      await view.checkbox("语音引擎随应用启动", enabled);
      expect(view.wrapper.get('[data-testid="screen-config-voice"]').classes()).toContain("is-edited");
      expect(view.wrapper.text()).toContain("本次修改 1 台 · 预计重启 1 台");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[0]!]: { set: { autoStartVoiceEngine: enabled }, clear: [] } });
      await view.clickLabel("查看 测试屏1 配置详情");
      expect(view.wrapper.text()).toContain("语音引擎随应用启动");
      expect(view.wrapper.text()).toContain("保存成功后重启小新一次");
      await view.click("确认修改 1 台");
      expect(view.execute).toHaveBeenCalledOnce();
    } finally { view.dispose(); }
  });

  it("浮层语音启动混合值三态循环可恢复保留，统一只提交不同屏", async () => {
    const view = await render({ count: 3, voiceValues: [true, true, false] });
    try {
      await view.click("读取 3 台配置");
      await view.openVoice();
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "语音引擎随应用启动")!;
      expect(control.props("indeterminate")).toBe(true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      await view.checkbox("语音引擎随应用启动", true);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual([view.ids[2]]);
      await view.checkbox("语音引擎随应用启动", false);
      await view.checkbox("语音引擎随应用启动", true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "语音引擎随应用启动")!.props("indeterminate")).toBe(true);
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.checkbox("语音引擎随应用启动", true);
      await view.checkbox("语音引擎随应用启动", false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[1]![1].targetIds).toEqual(view.ids.slice(0, 2));
    } finally { view.dispose(); }
  });

  it.each(["unsupported", "missing", "invalid"] as const)("语音启动%s只禁该项且不当成关闭，其他配置仍可修改", async state => {
    const view = await render({ count: 2, voiceValues: [false, state] });
    try {
      await view.click("读取 2 台配置");
      await view.openVoice();
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "语音引擎随应用启动")!;
      expect(control.props("disabled")).toBe(true);
      expect(control.props("indeterminate")).toBe(true);
      expect(view.wrapper.text()).toContain("部分屏不支持或当前值未确认");
      await view.checkbox("语音引擎随应用启动", true);
      await view.input("WebSocket(可选)", "wss://other.example.org");
      await view.click("检查修改内容");
      for (const patch of Object.values(view.preflight.mock.calls[0]![2])) expect(patch.set).not.toHaveProperty("autoStartVoiceEngine");
      await view.clickLabel("查看 测试屏2 配置详情");
      expect(view.wrapper.text()).toContain("语音引擎随应用启动：未确认，当前不可修改");
    } finally { view.dispose(); }
  });

  it("取消不支持屏后可以编辑语音，选择变化使已有检查失效", async () => {
    const view = await render({ count: 2, voiceValues: [false, "unsupported"] });
    try {
      await view.click("读取 2 台配置");
      await view.checkbox("选择 测试屏2", false);
      await view.checkbox("语音引擎随应用启动", true);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual([view.ids[0]]);
      await view.checkbox("选择 测试屏2", true);
      expect(view.wrapper.findAll("button").some(item => item.text().includes("确认修改"))).toBe(false);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });

  it("语音明确草稿需重读后提交，切换环境保留全局选择且和环境修改共用一次重启", async () => {
    const view = await render({ count: 1, voiceValues: [true], restoreDraft: true, draftVoiceMode: "off" });
    try {
      await flushPromises(); await view.click("继续本机未提交修改");
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.click("重新读取配置");
      await view.select("选择环境", "test");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]!.set).toEqual({ autoStartVoiceEngine: false, "environments.current": "test" });
      expect(view.wrapper.text()).toContain("本次修改 1 台 · 预计重启 1 台");
      await vi.waitFor(() => expect(view.saveDraft).toHaveBeenCalled());
      expect(view.saveDraft.mock.calls.at(-1)).toEqual([view.store.projectId, expect.objectContaining({ voiceMode: "off" })]);
    } finally { view.dispose(); }
  });

  it("旧草稿没有语音模式仍保留实际值；重读失去能力后不重放已编辑开关", async () => {
    const options = { count: 1, voiceValues: [true as boolean | "missing"], restoreDraft: true };
    const view = await render(options);
    try {
      await flushPromises(); await view.click("继续本机未提交修改"); await view.click("重新读取配置");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]!.set).not.toHaveProperty("autoStartVoiceEngine");
      await view.checkbox("语音引擎随应用启动", false);
      options.voiceValues[0] = "missing";
      await view.click("重新读取配置");
      await view.openVoice();
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "语音引擎随应用启动")!;
      expect(control.props("disabled")).toBe(true);
      expect(control.props("indeterminate")).toBe(true);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[1]![2][view.ids[0]!]!.set).not.toHaveProperty("autoStartVoiceEngine");
    } finally { view.dispose(); }
  });

  it("重新读取后语音已达到目标时不重复提交，切项目后清除旧语音修改", async () => {
    const options = { count: 1, voiceValues: [false] };
    const view = await render(options);
    try {
      await view.click("读取 1 台配置"); await view.checkbox("语音引擎随应用启动", true);
      options.voiceValues[0] = true;
      await view.click("重新读取配置");
      expect(view.wrapper.text()).toContain("本次修改 0 台 · 预计重启 0 台");
      await view.checkbox("语音引擎随应用启动", false);
      await view.store.bindProject("config-ui-next"); await flushPromises();
      expect(view.step.value).toBe(0);
      view.store.selectedIds = [view.store.snapshot.screens[0]!.id];
      await flushPromises();
      await view.click("读取 1 台配置");
      expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });

  it.each([false, undefined])("已固定但运行状态%s时可显式激活，读取本身不产生修改", async active => {
    const view = await render({ count: 1, homeValues: [true], homeActiveValues: [active] });
    try {
      await view.click("读取 1 台配置");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "固定小新为主屏")!;
      expect(control.props("checked")).toBe(true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(false);
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      expect(control.props("indeterminate")).toBe(false);
      await view.checkbox("切到小新主屏", true);
      expect(view.wrapper.text()).toContain("本次修改 1 台 · 预计重启 0 台");
      expect(view.wrapper.get('[data-testid="screen-config-home-impact"]').text()).toBe("切到小新主屏可能重新打开页面，当前语音对话会结束。");
      expect(control.classes()).not.toContain("is-edited");
      expect(view.wrapper.get('.screen-config-home-activation').classes()).toContain("is-edited");
      expect(view.wrapper.get('[data-testid="screen-config-home"]').attributes("title")).toContain("勾选“切到小新主屏”后保存");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[0]!]: { set: {}, clear: [], homeEnabled: true } });
      await view.clickLabel('查看 测试屏1 配置详情');
      expect(view.wrapper.get('[data-config-field="homeEnabled"]').text()).toBe('已固定，激活未确认 → 固定并激活');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 1 项');
      await view.closeDetail();
      await view.checkbox("切到小新主屏", false);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(false);
    } finally { view.dispose(); }
  });
  it("混合批次开启只修改未固定屏，不暗中激活原已固定屏", async () => {
    const view = await render({ count: 3, homeValues: [true, true, false], homeActiveValues: [true, false, false] });
    try {
      await view.click("读取 3 台配置"); await view.checkbox("固定小新为主屏", true);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual([view.ids[2]]);
    } finally { view.dispose(); }
  });
  it("主屏选项位于H5检测同一行，初始勾选但未读取时不可修改", async () => {
    const view = await render({ count: 1, homeValues: [false] });
    let finishRead: ((rows: ScreenAppConfigRead[]) => void) | undefined;
    view.read.mockImplementationOnce(() => new Promise(resolve => { finishRead = resolve; }));
    try {
      await view.click("读取 1 台配置");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "固定小新为主屏")!;
      expect(control.props("checked")).toBe(true);
      expect(control.props("disabled")).toBe(true);
      expect(control.props("indeterminate")).toBe(false);
      const group = view.wrapper.get(".screen-config-checks");
      expect(group.text()).toContain("检查H5地址"); expect(group.text()).toContain("固定小新为主屏");
      expect(group.element.nextElementSibling!.textContent).toContain("重新读取配置");
      expect(group.text()).toContain("语音配置 >");
      expect(group.text()).not.toContain("语音引擎随应用启动");
      expect(screenActions.some(action => action.label === "设置主屏应用")).toBe(false);
    } finally { finishRead?.([]); await flushPromises(); view.dispose(); }
  });

  it.each([true, false])("读取主屏实际值 %s 后照实展示，未编辑不随其他配置写入", async enabled => {
    const view = await render({ count: 1, homeValues: [enabled] });
    try {
      await view.click("读取 1 台配置");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "固定小新为主屏")!;
      expect(control.props("checked")).toBe(enabled); expect(control.props("disabled")).toBe(false);
      await view.input("WebSocket(可选)", "wss://new.example.org");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).not.toHaveProperty("homeEnabled");
    } finally { view.dispose(); }
  });

  it.each([true, false])("显式修改主屏为 %s 与保存共用流程，单独修改无需重启", async enabled => {
    const view = await render({ count: 1, homeValues: [!enabled] });
    try {
      await view.click("读取 1 台配置");
      await view.checkbox("固定小新为主屏", enabled);
      expect(view.wrapper.text()).toContain("本次修改 1 台 · 预计重启 0 台");
      expect(view.wrapper.find('[data-testid="screen-config-home-impact"]').exists()).toBe(enabled);
      expect(view.wrapper.get('[data-testid="screen-config-home"]').attributes("title")).toContain("取消固定不切换当前桌面");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2]).toEqual({ [view.ids[0]!]: { set: {}, clear: [], homeEnabled: enabled } });
      await view.click("确认修改 1 台");
      expect(view.execute).toHaveBeenCalledOnce(); expect(view.step.value).toBe(2);
    } finally { view.dispose(); }
  });

  it("主屏多台不同显示半选，主动勾选仅修改未固定屏，循环恢复后各自保留", async () => {
    const view = await render({ count: 3, homeValues: [true, true, false] });
    try {
      await view.click("读取 3 台配置");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "固定小新为主屏")!;
      expect(control.props("indeterminate")).toBe(true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      await view.checkbox("固定小新为主屏", true);
      expect(control.props("indeterminate")).toBe(false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual([view.ids[2]]);
      await view.checkbox("固定小新为主屏", false);
      expect(view.wrapper.text()).toContain("本次修改 2 台");
      await view.checkbox("固定小新为主屏", true);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
      expect(control.props("indeterminate")).toBe(true);
    } finally { view.dispose(); }
  });

  it.each(["unsupported", "unknown"] as const)("主屏 %s 仅禁用该项，其他配置正常提交", async state => {
    const view = await render({ count: 2, homeValues: [false, state] });
    try {
      await view.click("读取 2 台配置");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "固定小新为主屏")!;
      expect(control.props("disabled")).toBe(true);
      expect(view.wrapper.get('[data-testid="screen-config-home"]').text()).toContain("不可改");
      expect(view.wrapper.get('[data-testid="screen-config-home"]').attributes("title")).toContain("其他配置可继续修改");
      await view.checkbox("固定小新为主屏", true);
      await view.input("WebSocket(可选)", "wss://new.example.org");
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![1].targetIds).toEqual(view.ids);
      for (const patch of Object.values(view.preflight.mock.calls[0]![2])) expect(patch).not.toHaveProperty("homeEnabled");
    } finally { view.dispose(); }
  });

  it("主屏草稿保留明确解除意图，重新读取后才能保存，切换环境不重置主屏修改", async () => {
    const view = await render({ count: 1, homeValues: [true], restoreDraft: true, draftHomeMode: "off" });
    try {
      await flushPromises(); await view.click("继续本机未提交修改");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "固定小新为主屏")!;
      expect(control.props("checked")).toBe(false); expect(control.props("disabled")).toBe(true);
      expect(view.button("检查修改内容").attributes("disabled")).toBeDefined();
      await view.click("重新读取配置"); await view.select("选择环境", "test");
      expect(control.props("checked")).toBe(false);
      await view.click("检查修改内容");
      expect(view.preflight.mock.calls[0]![2][view.ids[0]!]).toMatchObject({ homeEnabled: false });
      await vi.waitFor(() => expect(view.saveDraft).toHaveBeenCalled());
      expect(view.saveDraft.mock.calls.at(-1)).toEqual([view.store.projectId, expect.objectContaining({ homeMode: "off" })]);
    } finally { view.dispose(); }
  });

  it("主屏失败后重试重新读取实际值，不自动重放旧开关或默认勾选", async () => {
    const view = await render({ count: 1, homeValues: [false] });
    try {
      const screens = [...view.store.snapshot.screens];
      await view.click("读取 1 台配置"); await view.checkbox("固定小新为主屏", true);
      await view.click("检查修改内容"); await view.click("确认修改 1 台");
      // 执行适配器是替身，重试仍需保留这个合成项目中的实际目标记录。
      view.store.snapshot.screens = screens;
      view.store.selectedIds = view.ids;
      view.store.snapshot.tasks = [{ id: "config-task", projectId: view.store.projectId, action: "app_config", state: "failed", createdAt: "2026-10-13T00:00:00Z", updatedAt: "2026-10-13T00:00:00Z", targets: [{ screenId: view.ids[0]!, name: "测试屏1", ip: "192.0.2.1", state: "failed", progress: 100, message: "主屏设置被拒绝" }], logs: [] }];
      view.store.openedTaskId = "config-task"; await flushPromises();
      view.wrapper.getComponent(ScreenTaskResults).vm.$emit("retry", view.ids); await flushPromises();
      await view.click("读取 1 台配置");
      const control = view.wrapper.findAllComponents(NCheckbox).find(item => item.attributes("aria-label") === "固定小新为主屏")!;
      expect(control.props("checked")).toBe(false);
      expect(view.wrapper.text()).toContain("本次修改 0 台");
    } finally { view.dispose(); }
  });
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
      const facts = Object.fromEntries(view.wrapper.get('.screen-config-detail-body').findAll('dt').map(label => [label.text(), label.element.nextElementSibling?.textContent]));
      expect(facts['当前运行：']).toBe('生产环境 · 查看预发环境配置');
      expect(facts['OTA 地址：']).toBe('https://different.example.org');
      expect(facts['服务端语音地址：']).toBe('未下发');
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
      await view.clickLabel('查看 测试屏1 配置详情');
      const facts = Object.fromEntries(view.wrapper.get('.screen-config-detail-body').findAll('dt').map(label => [label.text(), label.element.nextElementSibling?.textContent]));
      expect(facts['WebSocket(可选)：']).toBe(`${originalWs} → wss://new.example.org/ws`);
      expect(facts['语音使用值（手动设置）：']).toBe(originalWs);
      expect(facts['保存后语音使用（手动设置）：']).toBe('wss://new.example.org/ws');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 1 项');
      expect(view.wrapper.get('.screen-config-current').findAll('dd.is-edited')).toHaveLength(1);
      await view.closeDetail(); await view.clickLabel('查看 测试屏4 配置详情');
      expect(view.wrapper.get('[data-config-field="environments.current"]').text()).toBe('生产环境 · 查看预发环境配置');
      expect(view.wrapper.get('[data-config-field="environments.current"]').classes()).not.toContain('is-edited');
      expect(view.wrapper.get('.screen-config-detail-pending').text()).toBe('待修改 1 项');
      await view.closeDetail();
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
      expect(view.wrapper.get('.screen-config-name-field small').text()).toBe('仅修改本屏的小新名称');
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
    const reads = await deferConfigReads(view);
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
      await reads.finish();
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
