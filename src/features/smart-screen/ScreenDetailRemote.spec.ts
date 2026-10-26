import { createPinia } from "pinia";
import { defineComponent, h, ref } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NButton, NDialogProvider, NMessageProvider } from "naive-ui";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { configureScreenRemoteAdapter, UnavailableScreenRemoteAdapter } from "@/shared/api/screenRemoteAdapter";
import type { ScreenRemoteAdapter } from "@/shared/api/screenRemoteAdapter";
import ScreenDetail from "./ScreenDetail.vue";
import ScreenRemoteDialog from "./remote-screen/ScreenRemoteDialog.vue";

async function fixture() {
  const pinia = createPinia(), store = useSmartScreensStore(pinia);
  store.projectId = "project-one";
  store.snapshot = createScreenSnapshot();
  const original = store.snapshot.screens[0]!;
  store.snapshot.screens = [{ ...original, id: "screen-one", name: "原屏", ip: "192.0.2.1", aliases: [] },
    { ...original, id: "screen-two", name: "另一屏", ip: "192.0.2.2", aliases: [] }];
  const selectedId = ref("screen-one");
  const adapter = {
    open: vi.fn(async (projectId: string, screenId: string) => ({ projectId, screenId, sessionId: "session-one", name: "原屏", ip: "192.0.2.1" })),
    capture: vi.fn(async () => ({ sessionId: "session-one", frameId: "f1", width: 800, height: 480, capturedAt: "2026-10-26T05:00:00Z", elapsedMs: 100, pngBase64: "iVBORw0KGgo=", controlActive: false, inputReady: false, inputGeneration: 0 })),
    close: vi.fn(async () => {}),
    beginControl: vi.fn<ScreenRemoteAdapter["beginControl"]>(),
    endControl: vi.fn<ScreenRemoteAdapter["endControl"]>(),
    action: vi.fn<ScreenRemoteAdapter["action"]>(),
  } satisfies ScreenRemoteAdapter;
  configureScreenRemoteAdapter(adapter);
  vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:detail-screen");
  vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
    default: () => h(NDialogProvider, null, { default: () => h(ScreenDetail, {
      screen: store.snapshot.screens.find(screen => screen.id === selectedId.value),
    }) }),
  }) }), { global: { plugins: [pinia], stubs: { teleport: true } }, attachTo: document.body });
  await flushPromises();
  await wrapper.getComponent(ScreenDetail).findAllComponents(NButton).find(button => button.text() === "查看屏幕")!.trigger("click");
  await flushPromises();
  return { wrapper, store, adapter, selectedId };
}

describe("屏详情刷新与远程窗口的身份边界", () => {
  afterEach(() => { vi.restoreAllMocks(); configureScreenRemoteAdapter(new UnavailableScreenRemoteAdapter()); });
  it("结束任务后的同id资产对象刷新保留窗口和当前会话", async () => {
    const { wrapper, store, adapter } = await fixture();
    try {
      const remoteUid = wrapper.getComponent(ScreenRemoteDialog).vm.$.uid;
      expect(adapter.open).toHaveBeenCalledTimes(1);
      const previous = store.snapshot.screens[0];
      store.snapshot = { ...store.snapshot, screens: store.snapshot.screens.map(screen => ({ ...screen,
        checkedAt: "2026-10-26T05:01:00Z", observedAppVersion: "2.1.3", adbAvailable: true,
      })), tasks: [...store.snapshot.tasks] };
      await flushPromises();
      expect(store.snapshot.screens[0]).not.toBe(previous);
      expect(wrapper.getComponent(ScreenRemoteDialog).vm.$.uid).toBe(remoteUid);
      expect(wrapper.getComponent(ScreenRemoteDialog).props("show")).toBe(true);
      expect(adapter.open).toHaveBeenCalledTimes(1);
      expect(adapter.close).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); }
  });
  it("切到另一屏立即关闭原窗口，不自动操作新屏", async () => {
    const { wrapper, adapter, selectedId } = await fixture();
    try {
      selectedId.value = "screen-two";
      await flushPromises();
      expect(wrapper.findComponent(ScreenRemoteDialog).exists()).toBe(false);
      expect(adapter.close).toHaveBeenCalledExactlyOnceWith("project-one", "session-one");
      expect(adapter.open).toHaveBeenCalledTimes(1);
    } finally { wrapper.unmount(); }
  });
  it("同一屏标识下切项目仍关闭会话，保持原项目的关闭参数", async () => {
    const { wrapper, store, adapter } = await fixture();
    try {
      store.projectId = "project-two";
      await flushPromises();
      expect(wrapper.findComponent(ScreenRemoteDialog).exists()).toBe(false);
      expect(adapter.close).toHaveBeenCalledExactlyOnceWith("project-one", "session-one");
      expect(adapter.open).toHaveBeenCalledTimes(1);
    } finally { wrapper.unmount(); }
  });
});
