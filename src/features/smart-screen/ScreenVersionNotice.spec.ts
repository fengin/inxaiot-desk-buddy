import { createPinia } from "pinia";
import { defineComponent, h, nextTick, reactive } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NButton, NDialogProvider, NMessageProvider, NSelect } from "naive-ui";
import { describe, expect, it } from "vitest";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import type { SmartScreen } from "@/shared/model/screen";
import { screenNeedsVersionSync } from "@/shared/model/screenMaintenance";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ScreenList from "./ScreenList.vue";
import ScreenDetail from "./ScreenDetail.vue";

function sample(): SmartScreen {
  const screen = createScreenSnapshot().screens[0]!;
  return { ...screen, appVersion: "2.0.9", observedAppVersion: "2.1.0", appVersionCode: 6027,
    versionCheckedIp: screen.ip, versionCheckedAt: "2026-10-21T06:22:24Z", appInstalled: true };
}

describe("版本待同步提示的独立规则", () => {
  const cases: [string, Partial<SmartScreen>, boolean][] = [
    ["平台旧版本与同IP观测不同", {}, true], ["平台尚未填写版本", { appVersion: null }, true],
    ["只有格式空白差异", { observedAppVersion: " 2.0.9 " }, false],
    ["同名不同构建不属于平台版本差异", { observedAppVersion: "2.0.9", appVersionCode: 9999 }, false],
    ["未注册屏", { source: "local" }, false], ["空观测", { observedAppVersion: null }, false],
    ["空白观测", { observedAppVersion: "  " }, false], ["旧IP观测", { versionCheckedIp: "192.0.2.220" }, false],
    ["平台与历史观察地址均空", { ip: "", versionCheckedIp: "" }, false],
    ["缺少观测地址", { versionCheckedIp: undefined }, false], ["已确认未安装", { appInstalled: false }, false]
  ];
  it.each(cases)("%s", (_label, patch, expected) => {
    const screen = { ...sample(), ...patch }, before = JSON.stringify(screen);
    expect(screenNeedsVersionSync(screen)).toBe(expected);
    expect(JSON.stringify(screen)).toBe(before);
  });
});

describe("列表与详情版本同步提示", () => {
  it("列表保留平台主版本及筛选，独立提示携带版本来源与读取时间，点击只发核对事件", async () => {
    const pinia = createPinia(), store = useSmartScreensStore(pinia), snapshot = createScreenSnapshot();
    snapshot.screens = [sample()]; store.snapshot = snapshot;
    const screen = store.snapshot.screens[0]!, before = JSON.stringify(screen);
    const wrapper = mount(ScreenList, { global: { plugins: [pinia] } });
    try {
      const pending = wrapper.get(`[data-testid="screen-version-sync-${screen.id}"]`);
      expect(pending.text()).toBe("版本待同步");
      expect(pending.attributes("title")).toContain("平台版本：2.0.9");
      expect(pending.attributes("title")).toContain("本机读取版本：2.1.0");
      expect(pending.attributes("title")).toContain("读取时间：2026-10-21");
      expect(pending.attributes("title")).toContain(screen.abi);
      expect(wrapper.get("tbody tr").findAll("td")[5]!.text()).toBe("2.0.9版本待同步");
      expect(store.draftIds.has(screen.id)).toBe(false);
      expect(wrapper.find(".screen-draft-link").exists()).toBe(false);
      await pending.trigger("click");
      expect(wrapper.emitted("versions")).toEqual([[[screen.id]]]);
      expect(JSON.stringify(screen)).toBe(before); expect(store.selectedIds).toEqual([]);
      const options = wrapper.findAllComponents(NSelect).find(select => select.attributes("aria-label") === "应用版本筛选")!.props("options");
      expect(options).toContainEqual({ label: "小新 2.0.9", value: "2.0.9" });
      expect(options).not.toContainEqual({ label: "小新 2.1.0", value: "2.1.0" });
      store.filters.appVersion = "2.1.0"; expect(store.filtered).toEqual([]);
      store.filters.appVersion = "2.0.9"; expect(store.filtered).toEqual([screen]);
      store.filters.appVersion = "all"; screen.appVersion = "2.1.0"; await nextTick();
      expect(wrapper.find(".screen-version-pending").exists()).toBe(false);
      expect(wrapper.get("tbody tr").findAll("td")[5]!.text()).toContain(screen.abi);
      expect(store.draftIds.has(screen.id)).toBe(false);
    } finally { wrapper.unmount(); store.stop(); }
  });

  it("旧IP观测及未注册屏继续显示普通ABI，不制造平台待同步提示", () => {
    const pinia = createPinia(), store = useSmartScreensStore(pinia), snapshot = createScreenSnapshot();
    snapshot.screens = [{ ...sample(), versionCheckedIp: "192.0.2.220" }, { ...sample(), id: "local-version", source: "local", ip: "192.0.2.221" }];
    store.snapshot = snapshot;
    const wrapper = mount(ScreenList, { global: { plugins: [pinia] } });
    try {
      expect(wrapper.find(".screen-version-pending").exists()).toBe(false);
      expect(wrapper.findAll("tbody tr").every(row => row.findAll("td")[5]!.text().includes(sample().abi))).toBe(true);
      expect(store.draftIds.size).toBe(0);
    } finally { wrapper.unmount(); store.stop(); }
  });

  it("详情保留旧IP版本观测供追溯，不将它提示为当前屏的待同步版本", async () => {
    const pinia = createPinia(), store = useSmartScreensStore(pinia);
    const screen = { ...sample(), versionCheckedIp: "192.0.2.220" };
    store.snapshot = { ...createScreenSnapshot(), screens: [screen] };
    const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
      default: () => h(NDialogProvider, null, { default: () => h(ScreenDetail, { screen }) })
    }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
    try {
      await flushPromises();
      const detail = wrapper.getComponent(ScreenDetail);
      expect(detail.text()).toContain("2.1.0（历史观测，需重新核对）");
      expect(detail.text()).not.toContain("版本待同步");
      const value = detail.findAll("dd").find(item => item.text().includes("历史观测"))!;
      expect(value.attributes("title")).toContain("192.0.2.220");
      expect(value.classes()).not.toContain("screen-warning");
      const button = detail.findAllComponents(NButton).find(item => item.text() === "核对版本并同步平台")!;
      expect(button.props("type")).toBe("default");
    } finally { wrapper.unmount(); store.stop(); }
  });

  it("详情清楚标记本机观测待同步，复用唯一同步按钮，写回同值后恢复普通状态", async () => {
    const pinia = createPinia(), store = useSmartScreensStore(pinia), screen = reactive(sample());
    store.snapshot = { ...createScreenSnapshot(), screens: [screen] };
    const before = JSON.stringify(screen);
    const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
      default: () => h(NDialogProvider, null, { default: () => h(ScreenDetail, { screen }) })
    }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
    try {
      await flushPromises();
      const detail = wrapper.getComponent(ScreenDetail);
      expect(detail.text()).toContain("2.1.0（版本待同步）");
      const buttons = () => detail.findAllComponents(NButton).filter(button => button.text() === "核对版本并同步平台");
      expect(buttons()).toHaveLength(1); expect(buttons()[0]!.props("type")).toBe("warning");
      expect(buttons()[0]!.attributes("title")).toContain("平台版本：2.0.9");
      await buttons()[0]!.trigger("click"); expect(detail.emitted("versions")).toEqual([[]]);
      expect(JSON.stringify(screen)).toBe(before);
      screen.appVersion = screen.observedAppVersion!; await nextTick();
      expect(detail.text()).not.toContain("版本待同步"); expect(buttons()).toHaveLength(1);
      expect(buttons()[0]!.props("type")).toBe("default");
    } finally { wrapper.unmount(); store.stop(); }
  });
});
