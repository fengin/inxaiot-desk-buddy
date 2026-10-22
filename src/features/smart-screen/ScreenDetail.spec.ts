import { createPinia } from "pinia";
import { defineComponent, h, nextTick } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NButton, NDialogProvider, NMessageProvider } from "naive-ui";
import { describe, expect, it } from "vitest";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import type { SmartScreen } from "@/shared/model/screen";
import { screenPlatformFields, type ScreenPlatformFields } from "@/shared/model/screenRegistration";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ScreenDetail from "./ScreenDetail.vue";
import ScreenList from "./ScreenList.vue";

async function render(changes: Partial<ScreenPlatformFields> = {}, source: SmartScreen["source"] = "platform") {
  const pinia = createPinia(), store = useSmartScreensStore(pinia), snapshot = createScreenSnapshot();
  snapshot.screens = [{ ...snapshot.screens[0]!, source, name: "已确认名称", ip: "192.0.2.10",
    mac: "02:11:22:33:44:55", observedMac: "02:AA:BB:CC:DD:10", size: "10",
    spaceId: "old-room", spacePath: "原楼/原房间", location: "原位置", platformStatus: "online", ping: "online",
    appVersion: "2.1.0", observedAppVersion: "2.1.2", versionCheckedIp: "192.0.2.90", appInstalled: true,
    appRunning: true, persistentAdb: true, aliases: [] }];
  snapshot.spaces = [
    { id: "old-building", name: "原楼", kind: "building" },
    { id: "old-room", name: "原房间", kind: "area", parentId: "old-building" },
    { id: "new-building", name: "新楼", kind: "building" },
    { id: "new-room", name: "新房间", kind: "area", parentId: "new-building" },
  ];
  snapshot.tasks = []; snapshot.ignoredPairs = []; snapshot.platformDrafts = {};
  snapshot.platformAvailable = true; snapshot.spacesAvailable = true;
  store.snapshot = snapshot;
  const screen = store.snapshot.screens[0]!, base = screenPlatformFields(screen);
  store.snapshot.platformDrafts![screen.id] = { screenId: screen.id, baseRevision: screen.revision,
    base, values: { ...base, ...changes }, updatedAt: "2026-10-23T02:00:00Z" };
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
    default: () => h(NDialogProvider, null, { default: () => [h(ScreenList), h(ScreenDetail, { screen })] })
  }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
  await flushPromises();
  const detail = wrapper.getComponent(ScreenDetail), list = wrapper.getComponent(ScreenList);
  const field = (key: keyof ScreenPlatformFields) => detail.get(`[data-detail-field="${key}"]`);
  const fact = (label: string) => {
    const row = detail.findAll(".device-detail-facts > div").find(item => item.get("dt").text() === label);
    expect(row, `详情应包含${label}`).toBeDefined();
    return row!.get("dd");
  };
  return { store, screen, wrapper, detail, list, field, fact,
    draft: store.snapshot.platformDrafts![screen.id]!, dispose() { wrapper.unmount(); store.stop(); } };
}

describe("智能屏详情与列表共用待提交资料展示", () => {
  it("六项草稿同时展示并着色，提示平台原值，编辑与运维仍引用已确认记录", async () => {
    const changes = { name: "待提交名称", ip: "192.0.2.20", mac: "02:22:33:44:55:66",
      size: "4" as const, spaceId: "new-room", location: "待提交位置" };
    const view = await render(changes), before = JSON.stringify(view.screen);
    try {
      for (const [key, text] of Object.entries({ ...changes, size: "4 寸屏", spaceId: "新楼/新房间" })) {
        const value = view.field(key as keyof ScreenPlatformFields);
        expect(value.text()).toBe(text);
        expect(value.classes()).toContain("screen-draft-value");
        expect(value.attributes("title")).toContain("待提交");
        expect(value.attributes("title")).toContain("最新平台值");
      }
      const row = view.list.get("tbody tr");
      for (const text of [changes.name, changes.ip, changes.mac, changes.location, "新楼/新房间", "4 寸"]) expect(row.text()).toContain(text);
      expect(view.detail.findAll(".screen-draft-value")).toHaveLength(6);
      expect(view.detail.get('[data-testid="screen-detail-draft"]').text()).toContain("资料有变更，尚未更新平台");
      expect(view.detail.get('[data-testid="screen-detail-confirmed-ip"]').text()).toContain("已确认 IP 192.0.2.10");
      const button = (label: string) => view.detail.findAllComponents(NButton).find(item => item.text() === label)!;
      await button("编辑资料").trigger("click");
      expect(view.detail.emitted("edit")?.[0]?.[0]).toBe(view.screen);
      await button("检查设备").trigger("click");
      expect(view.detail.emitted("operate")).toEqual([["inspect"]]);
      expect(JSON.stringify(view.screen)).toBe(before);
    } finally { view.dispose(); }
  });

  it("明确清空MAC、空间与位置不回填观测或旧路径，恢复原值后移除提示和颜色", async () => {
    const view = await render({ mac: "", spaceId: null, location: "" });
    try {
      expect(view.field("mac").text()).toBe("MAC 未填写");
      expect(view.field("mac").text()).not.toContain(view.screen.observedMac);
      expect(view.field("spaceId").text()).toBe("待定空间");
      expect(view.field("location").text()).toBe("未填写");
      expect(view.detail.findAll(".screen-draft-value")).toHaveLength(3);
      expect(view.fact("采集 MAC").text()).toBe(view.screen.observedMac);
      Object.assign(view.draft.values, view.draft.base);
      await nextTick();
      expect(view.detail.findAll(".screen-draft-value")).toHaveLength(0);
      expect(view.detail.find('[data-testid="screen-detail-draft"]').exists()).toBe(false);
      expect(view.field("mac").text()).toBe(view.screen.mac);
      expect(view.field("spaceId").text()).toBe("原楼/原房间");
      expect(view.field("location").text()).toBe("原位置");
      expect(view.store.snapshot.platformDrafts![view.screen.id]).toBeDefined();
    } finally { view.dispose(); }
  });

  it("未编辑字段跟随平台刷新，MAC格式同值和平台已收敛草稿不制造差异", async () => {
    const view = await render({ location: "待提交位置", mac: "02-11-22-33-44-55" });
    try {
      expect(view.field("mac").classes()).not.toContain("screen-draft-value");
      view.screen.name = "平台最新名称";
      view.screen.location = "平台另改位置";
      await nextTick();
      expect(view.field("name").text()).toBe("平台最新名称");
      expect(view.field("name").classes()).not.toContain("screen-draft-value");
      expect(view.field("location").text()).toBe("待提交位置");
      expect(view.field("location").attributes("title")).toContain("平台另改位置");
      view.screen.location = "待提交位置";
      await nextTick();
      expect(view.detail.findAll(".screen-draft-value")).toHaveLength(0);
      expect(view.detail.find('[data-testid="screen-detail-draft"]').exists()).toBe(false);
      expect(view.list.find(".screen-draft-link").exists()).toBe(false);
      delete view.store.snapshot.platformDrafts![view.screen.id];
      await nextTick();
      expect(view.field("name").text()).toBe("平台最新名称");
    } finally { view.dispose(); }
  });

  it("IP及尺寸草稿不改变历史版本、采集MAC、平台状态和按已确认尺寸判断的维护能力", async () => {
    const view = await render({ ip: "192.0.2.20", size: "4" });
    try {
      expect(view.field("ip").text()).toBe("192.0.2.20");
      expect(view.fact("采集 MAC").text()).toBe("02:AA:BB:CC:DD:10");
      expect(view.fact("平台在线状态").text()).toBe("在线");
      expect(view.fact("平台小新版本").text()).toBe("2.1.0");
      expect(view.fact("本机观测版本").text()).toBe("2.1.2（历史观测，需重新核对）");
      expect(view.fact("本机观测版本").attributes("title")).toContain("192.0.2.90");
      expect(view.fact("5555持久配置").text()).toBe("已读到5555配置");
      expect(view.detail.get('[data-testid="screen-detail-confirmed-ip"]').text()).toContain("待提交 IP 尚未用于这些结果");
      const adbButton = view.detail.findAllComponents(NButton).find(item => item.text() === "保持ADB端口")!;
      expect(adbButton.props("disabled")).toBe(false);
      expect(view.screen.ip).toBe("192.0.2.10"); expect(view.screen.size).toBe("10");
    } finally { view.dispose(); }
  });

  it("离线新空间不沿用旧路径，平台原值提示使用缓存语义", async () => {
    const view = await render({ spaceId: "unavailable-room", location: "新位置" });
    try {
      view.store.snapshot.platformAvailable = false;
      view.store.snapshot.spacesAvailable = false;
      view.store.snapshot.spaces = [];
      await nextTick();
      expect(view.field("spaceId").text()).toBe("待提交空间 unavailable-room（待核验）");
      expect(view.field("spaceId").text()).not.toContain("原楼/原房间");
      expect(view.field("location").attributes("title")).toContain("已确认值（缓存）：原位置");
      expect(view.list.get("tbody tr").text()).toContain(view.field("spaceId").text());
    } finally { view.dispose(); }
  });

  it("未注册屏只展示已保存本机资料，不采用平台草稿", async () => {
    const view = await render({ name: "不应采用", ip: "192.0.2.20" }, "local");
    try {
      expect(view.field("name").text()).toBe("已确认名称");
      expect(view.field("ip").text()).toBe("192.0.2.10");
      expect(view.detail.findAll(".screen-draft-value")).toHaveLength(0);
      expect(view.detail.find('[data-testid="screen-detail-draft"]').exists()).toBe(false);
      expect(view.field("mac").text()).toBe(view.screen.mac);
    } finally { view.dispose(); }
  });
});
