import { createPinia } from "pinia";
import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { describe, expect, it, vi } from "vitest";
import { NCheckbox } from "naive-ui";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import type { ScreenSpaceNode } from "@/shared/model/screenSpace";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ScreenList from "./ScreenList.vue";

function largeSnapshot() {
  const snapshot = createScreenSnapshot();
  const spaces: ScreenSpaceNode[] = [{ id: "root", name: "项目", kind: "other" }];
  for (let i = 0; i < 4; i++) spaces.push({ id: `building-${i}`, name: `楼幢${i}`, kind: "building", parentId: "root" });
  for (let i = 0; i < 75; i++) spaces.push({ id: `floor-${i}`, name: `楼层${i}`, kind: "floor", parentId: `building-${i % 4}` });
  for (let i = 0; i < 450; i++) spaces.push({ id: `room-${i}`, name: `房间${i}`, kind: "area", parentId: `floor-${i % 75}` });
  const base = snapshot.screens.find(screen => screen.source === "platform")!;
  return {
    ...snapshot, spaces, tasks: [], ignoredPairs: [], platformAvailable: false, spacesAvailable: true,
    screens: Array.from({ length: 269 }, (_, i) => ({ ...base, id: `screen-${i}`, name: `屏${i}`, spaceId: `room-${i}`, aliases: [] }))
  };
}

describe("智能屏列表空间统计与选择", () => {
  it("269台离线屏勾选、翻页与筛选不重复全量统计，实际空间变化仍刷新计数", async () => {
    const pinia = createPinia();
    const store = useSmartScreensStore(pinia);
    store.snapshot = largeSnapshot();
    const count = vi.spyOn(store.spaceIndex, "counts");
    const wrapper = mount(ScreenList, { global: { plugins: [pinia] } });
    try {
      expect(wrapper.findAll(".screen-table tbody tr")).toHaveLength(20);
      expect(wrapper.text()).toContain("当前显示 20 / 269 条");
      expect(wrapper.findAll(".screen-floor")).toHaveLength(75);
      expect(count).toHaveBeenCalledTimes(1);
      wrapper.get(".screen-table tbody tr").getComponent(NCheckbox).vm.$emit("update:checked", true);
      await nextTick();
      expect(store.selectedIds).toEqual(["screen-0"]);
      expect(wrapper.get(".screen-table tbody tr").classes()).toContain("selected");
      store.page = 2;
      await nextTick();
      store.selectPage(true);
      await nextTick();
      expect(store.selectedIds).toHaveLength(21);
      expect(count).toHaveBeenCalledTimes(1);
      store.filters.keyword = "屏1";
      store.filterChanged();
      await nextTick();
      expect(store.selectedIds).toEqual([]);
      expect(count).toHaveBeenCalledTimes(1);
      const before = Number(wrapper.findAll(".screen-floor")[0]!.get("small").text());
      store.snapshot.screens[0]!.spaceId = "room-1";
      await nextTick();
      expect(count).toHaveBeenCalledTimes(2);
      expect(Number(wrapper.findAll(".screen-floor")[0]!.get("small").text())).toBe(before - 1);
    } finally { wrapper.unmount(); }
  });

  it("列表显示六项待提交资料，逐字段着色并保留可见旧IP与事实来源，事件仍引用原记录", async () => {
    const pinia = createPinia();
    const store = useSmartScreensStore(pinia);
    store.snapshot = createScreenSnapshot();
    store.snapshot.screens = [store.snapshot.screens[0]!];
    const screen = store.snapshot.screens[0]!;
    screen.observedMac = "02:11:22:33:44:55";
    screen.appVersion = "1.6.0";
    const base = screenPlatformFields(screen);
    store.snapshot.platformDrafts = { [screen.id]: { screenId: screen.id, baseRevision: screen.revision, base,
      values: { ...base, name: "新名称", ip: "192.168.255.254", mac: "", size: "4", spaceId: "floor-b-1", location: "新位置" }, updatedAt: new Date().toISOString() } };
    const wrapper = mount(ScreenList, { global: { plugins: [pinia] } });
    try {
      const row = wrapper.get("tbody tr");
      expect(row.get(".screen-name").text()).toBe("新名称");
      expect(row.get(".screen-name").attributes("title")).toContain(base.name);
      expect(row.getComponent(NCheckbox).attributes("aria-label")).toBe("选择 新名称");
      expect(row.findAll(".screen-draft-value")).toHaveLength(6);
      expect(row.get(".screen-confirmed-ip").text()).toBe(`已确认：${screen.ip}`);
      expect(row.get(".screen-network").text()).toContain("192.168.255.254");
      expect(row.get(".screen-network").text()).toContain("MAC 未填写");
      expect(row.get(".screen-network").text()).not.toContain(screen.observedMac);
      for (const index of [2, 5, 6]) expect(row.findAll("td")[index]!.attributes("title")).toContain(`已确认 IP ${screen.ip}`);
      expect(row.text()).toContain("平台在线");
      expect(row.text()).toContain("1.6.0");
      expect(row.text()).toContain(screen.abi);
      expect(wrapper.text()).toContain("0 台 10 寸 · 1 台 4 寸");
      await row.get(".screen-name").trigger("click");
      expect(wrapper.emitted("detail")?.[0]).toEqual([screen.id]);
      row.getComponent(NCheckbox).vm.$emit("update:checked", true);
      await nextTick();
      expect(store.selectedScreens[0]).toBe(screen);
      expect(store.selectedScreens[0]!.ip).toBe(base.ip);
      expect(row.find(`[data-testid="screen-draft-${screen.id}"]`).exists()).toBe(true);
      store.snapshot.platformAvailable = false;
      await nextTick();
      expect(row.get(".screen-name").attributes("title")).toContain("已确认值（缓存）");
    } finally { wrapper.unmount(); }
  });

  it("空间与位置分别着色，平台未编辑字段采用新值，收敛/撤销草稿后解除颜色", async () => {
    const pinia = createPinia();
    const store = useSmartScreensStore(pinia);
    store.snapshot = createScreenSnapshot();
    store.snapshot.screens = [store.snapshot.screens[0]!];
    const screen = store.snapshot.screens[0]!;
    const base = screenPlatformFields(screen);
    store.snapshot.platformDrafts = { [screen.id]: { screenId: screen.id, baseRevision: screen.revision, base,
      values: { ...base, location: "待提交位置" }, updatedAt: new Date().toISOString() } };
    const wrapper = mount(ScreenList, { global: { plugins: [pinia] } });
    try {
      const row = wrapper.get("tbody tr");
      const location = () => row.findAll("td")[1]!.get(".screen-secondary").findAll("span");
      expect(location()[0]!.classes()).not.toContain("screen-draft-value");
      expect(location()[1]!.classes()).toContain("screen-draft-value");
      screen.name = "平台最新名称";
      screen.location = "平台另改位置";
      await nextTick();
      expect(row.get(".screen-name").text()).toBe("平台最新名称");
      expect(row.get(".screen-name").classes()).not.toContain("screen-draft-value");
      expect(location()[1]!.attributes("title")).toContain("最新平台值：平台另改位置");
      screen.location = "待提交位置";
      await nextTick();
      expect(row.findAll(".screen-draft-value")).toHaveLength(0);
      expect(row.find(".screen-draft-link").exists()).toBe(false);
      expect(store.snapshot.platformDrafts[screen.id]).toBeDefined();
      delete store.snapshot.platformDrafts[screen.id];
      await nextTick();
      expect(row.find(".screen-draft-link").exists()).toBe(false);
      expect(row.get(".screen-name").text()).toBe(screen.name);
    } finally { wrapper.unmount(); }
  });
});
