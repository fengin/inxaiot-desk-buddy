import { defineComponent, h, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NCheckbox, NMessageProvider, NPagination } from "naive-ui";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ScreenVersionPreview } from "@/shared/model/screenMaintenance";
import ScreenStatusDialog from "./ScreenStatusDialog.vue";
import ScreenVersionSyncDialog from "./ScreenVersionSyncDialog.vue";

const cleanups: (() => void)[] = [];
afterEach(() => { cleanups.splice(0).forEach((cleanup) => cleanup()); });

async function setup() {
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 10);
  configureScreenAdapter(adapter);
  const store = useSmartScreensStore(pinia), projectId = `sync-table-${crypto.randomUUID()}`;
  await store.bindProject(projectId);
  cleanups.push(() => { store.stop(); adapter.dispose(); });
  return { pinia, adapter, store, projectId };
}

describe("批量状态与版本核对表格", () => {
  it("300 台状态差异翻页不丢选择，确认仍提交全部已选设备，结果分页独立展示", async () => {
    const { pinia, adapter, store, projectId } = await setup();
    const base = store.snapshot.screens.find((screen) => screen.source === "platform")!;
    store.snapshot.screens = Array.from({ length: 300 }, (_, index) => ({
      ...base, id: `status-${index}`, name: `状态屏${index}`, ip: `192.0.2.${index % 250 + 1}`,
      platformStatus: "offline" as const, ping: "online" as const, checkedAt: "2026-10-08T01:00:00Z"
    }));
    const cover = vi.spyOn(adapter, "coverStatus").mockImplementation(async (_project, changes) =>
      changes.map((row) => ({ id: row.id, name: `状态屏${row.id.slice(7)}`, ok: true, message: "已更新并回读确认" })));
    const show = ref(false);
    const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
      default: () => h(ScreenStatusDialog, { show: show.value })
    }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
    cleanups.unshift(() => wrapper.unmount());
    show.value = true; await flushPromises();
    const dialog = wrapper.getComponent(ScreenStatusDialog);
    expect(dialog.findAll("tbody tr")).toHaveLength(50);
    expect(dialog.text()).toContain("已选 300 / 300 台");
    dialog.getComponent(NPagination).vm.$emit("update:page", 2); await flushPromises();
    const secondPageFirst = dialog.get('[data-screen-id="status-50"]');
    secondPageFirst.getComponent(NCheckbox).vm.$emit("update:checked", false); await flushPromises();
    dialog.getComponent(NPagination).vm.$emit("update:page", 1); await flushPromises();
    expect(dialog.get('[data-screen-id="status-0"]').getComponent(NCheckbox).props("checked")).toBe(true);
    expect(dialog.text()).toContain("已选 299 / 300 台");
    await dialog.findAll("button").find((button) => button.text() === "确认覆盖选中 299 台")!.trigger("click");
    await flushPromises();
    expect(cover).toHaveBeenCalledOnce();
    expect(cover.mock.calls[0]![0]).toBe(projectId);
    expect(cover.mock.calls[0]![1].map((row) => row.id)).toEqual(store.snapshot.screens.filter((screen) => screen.id !== "status-50").map((screen) => screen.id));
    expect(dialog.get('table[aria-label="状态同步结果"]').findAll("tbody tr")).toHaveLength(50);
    expect(dialog.text()).toContain("共 299 台");
    expect(dialog.text()).toContain("已更新并回读确认");
    dialog.getComponent(NPagination).vm.$emit("update:page-size", 100); await flushPromises();
    expect(dialog.findAll("tbody tr")).toHaveLength(100);
  });

  it("版本预览分页只改变展示，跨页取消和不可同步设备仍按原规则提交", async () => {
    const { pinia, adapter, store, projectId } = await setup();
    const preview: ScreenVersionPreview = {
      id: "large-preview", projectId, createdAt: "2026-10-08T01:00:00Z",
      items: Array.from({ length: 125 }, (_, index) => ({
        screenId: `version-${index}`, name: `版本屏${index}`, ip: `192.0.2.${index + 1}`,
        platformVersion: "2.0.9", deviceVersion: index === 51 ? null : "2.0.10", checkedAt: "2026-10-08T01:00:00Z",
        state: index === 51 ? "blocked" : "ready", reason: index === 51 ? "设备暂时无法访问" : "有版本差异，可同步"
      }))
    };
    vi.spyOn(adapter, "previewVersionSync").mockResolvedValue(preview);
    const submit = vi.spyOn(adapter, "submitVersionSync").mockResolvedValue("version-task");
    vi.spyOn(store, "refresh").mockResolvedValue();
    const wrapper = mount(ScreenVersionSyncDialog, {
      props: { show: true, ids: preview.items.map((item) => item.screenId) },
      global: { plugins: [pinia], stubs: { teleport: true } }
    });
    cleanups.unshift(() => wrapper.unmount()); await flushPromises();
    expect(wrapper.findAll("tbody tr")).toHaveLength(50);
    wrapper.getComponent(NPagination).vm.$emit("update:page", 2); await flushPromises();
    expect(wrapper.get('[data-version-screen="version-51"]').getComponent(NCheckbox).props("disabled")).toBe(true);
    wrapper.get('[data-version-screen="version-50"]').getComponent(NCheckbox).vm.$emit("update:checked", false); await flushPromises();
    wrapper.getComponent(NPagination).vm.$emit("update:page", 3); await flushPromises();
    expect(wrapper.findAll("tbody tr")).toHaveLength(25);
    expect(wrapper.get('[data-version-screen="version-124"]').getComponent(NCheckbox).props("checked")).toBe(true);
    await wrapper.get('[data-testid="screen-version-submit"]').trigger("click"); await flushPromises();
    expect(submit).toHaveBeenCalledExactlyOnceWith(projectId, "large-preview", preview.items.filter((item) => item.state === "ready" && item.screenId !== "version-50").map((item) => item.screenId));
  });
});
