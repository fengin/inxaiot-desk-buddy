import { defineComponent, h } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NCheckbox, NMessageProvider } from "naive-ui";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import type { ScreenRegistrationPreview } from "@/shared/model/screenRegistration";
import type { SmartScreen } from "@/shared/model/screen";
import ScreenOperations from "./ScreenOperations.vue";
import RegistrationPreview from "./ScreenRegistrationPreview.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";

function preview(projectId: string, screens: SmartScreen[]): ScreenRegistrationPreview {
  return { id: "registration-preview", projectId, createdAt: "2026-09-28T00:00:00Z", items: screens.map((screen) => ({
    screenId: screen.id, mode: "create", state: "ready", reason: "资料通过", before: null, after: screenPlatformFields(screen), diffs: [],
    expectedRevision: screen.revision, needsSpaceConfirmation: false, macSource: "collected", macMessage: "已模拟采集", requiredMacConfirmation: null, duplicateIds: []
  })) };
}
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
async function render() {
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 20);
  configureScreenAdapter(adapter);
  const store = useSmartScreensStore(pinia);
  await store.bindProject("registration-ui-project"); store.operation = "register";
  store.selectedIds = store.visibleScreens.slice(0, 2).map((screen) => screen.id);
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, { default: () => h(ScreenOperations) }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
  await flushPromises();
  return { wrapper, adapter, store, dispose() { wrapper.unmount(); store.stop(); adapter.dispose(); } };
}

describe("平台注册操作流程", () => {
  it("首步保留紧凑选屏布局，编辑资料通过公共编辑事件打开", async () => {
    const { wrapper, store, dispose } = await render();
    try {
      expect(wrapper.findAll(".process-step-heading strong").map((step) => step.text())).toEqual(["选择设备与资料", "检查并预览", "提交结果"]);
      expect(wrapper.find(".screen-target-picker").exists()).toBe(true);
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').text()).toBe("检查并预览");
      expect(wrapper.find(".screen-ops-footer").exists()).toBe(false);
      expect(wrapper.find('[aria-label="并发台数"]').exists()).toBe(false);
      await wrapper.get(".screen-registration-selected-row button").trigger("click");
      expect(wrapper.getComponent(ScreenOperations).emitted("edit")?.[0]?.[0]).toMatchObject({ id: store.selectedIds[0] });
    } finally { dispose(); }
  });

  it("MAC与空间确认逐屏生效，允许排除部分目标，并阻止双击重复提交", async () => {
    const { wrapper, adapter, store, dispose } = await render();
    const data = preview(store.projectId, store.selectedScreens);
    const item = data.items[0]!;
    Object.assign(item, { mode: "update", before: { ...item.after, spaceId: "old-space" }, needsSpaceConfirmation: true, requiredMacConfirmation: "existing", macSource: "history" });
    const check = vi.spyOn(adapter, "previewPlatformRegistration").mockResolvedValue(data);
    const gate = deferred<string>();
    const submit = vi.spyOn(adapter, "submitPlatformRegistration").mockImplementation(() => gate.promise);
    try {
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      expect(check).toHaveBeenCalledOnce(); expect(wrapper.get('[data-testid="screen-operation-submit"]').attributes("disabled")).toBeDefined();
      const confirms = wrapper.findAllComponents(NCheckbox).filter((checkbox) => checkbox.classes().includes("screen-registration-confirmation"));
      confirms[0]!.vm.$emit("update:checked", true); await flushPromises();
      expect(wrapper.get('[data-testid="screen-operation-submit"]').attributes("disabled")).toBeDefined();
      confirms[1]!.vm.$emit("update:checked", true); await flushPromises();
      wrapper.get(`.screen-registration-target[data-screen-id="${data.items[1]!.screenId}"]`).getComponent(NCheckbox).vm.$emit("update:checked", false);
      await flushPromises();
      const action = wrapper.get('[data-testid="screen-operation-submit"]');
      expect(action.text()).toContain("确认提交 1 台");
      await action.trigger("click"); await action.trigger("click");
      expect(submit).toHaveBeenCalledOnce();
      expect(submit.mock.calls[0]).toEqual([store.projectId, { previewId: data.id, screenIds: [item.screenId], macConfirmations: { [item.screenId]: "existing" }, spaceConfirmations: [item.screenId] }]);
      gate.resolve("registration-task"); await flushPromises(); expect(store.openedTaskId).toBe("registration-task");
    } finally { gate.resolve("registration-task"); dispose(); }
  });

  it("检查返回前项目切换，迟到预览不能进入新项目", async () => {
    const { wrapper, adapter, store, dispose } = await render();
    const data = preview(store.projectId, store.selectedScreens), gate = deferred<ScreenRegistrationPreview>();
    const check = vi.spyOn(adapter, "previewPlatformRegistration").mockImplementation(() => gate.promise);
    try {
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); expect(check).toHaveBeenCalledOnce();
      await store.bindProject("another-registration-project"); await flushPromises();
      gate.resolve(data); await flushPromises();
      expect(wrapper.findComponent(RegistrationPreview).exists()).toBe(false);
      expect(wrapper.find(".screen-ops-footer").exists()).toBe(false);
      expect(store.openedTaskId).toBe("");
    } finally { gate.resolve(data); dispose(); }
  });

  it("提交返回前项目切换，旧任务ID不覆盖新项目", async () => {
    const { wrapper, adapter, store, dispose } = await render();
    const data = preview(store.projectId, store.selectedScreens);
    vi.spyOn(adapter, "previewPlatformRegistration").mockResolvedValue(data);
    const gate = deferred<string>(), submit = vi.spyOn(adapter, "submitPlatformRegistration").mockImplementation(() => gate.promise);
    try {
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      await wrapper.get('[data-testid="screen-operation-submit"]').trigger("click"); expect(submit).toHaveBeenCalledOnce();
      await store.bindProject("another-registration-project"); await flushPromises();
      gate.resolve("old-registration-task"); await flushPromises();
      expect(store.openedTaskId).toBe(""); expect(wrapper.find(".screen-final-stage").exists()).toBe(false);
    } finally { gate.resolve("old-registration-task"); dispose(); }
  });

  it("预览后资料变更需要重新检查，疑似重复跳转也清除旧预览", async () => {
    const { wrapper, adapter, store, dispose } = await render();
    let data = preview(store.projectId, store.selectedScreens);
    vi.spyOn(adapter, "previewPlatformRegistration").mockImplementation(async () => data);
    const submit = vi.spyOn(adapter, "submitPlatformRegistration");
    try {
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      store.snapshot.screens[0]!.location = "修改后的位置"; await flushPromises();
      expect(wrapper.findComponent(RegistrationPreview).exists()).toBe(false);
      expect(wrapper.text()).toContain("设备资料或平台目录已变化"); expect(submit).not.toHaveBeenCalled();
      data = preview(store.projectId, store.selectedScreens);
      Object.assign(data.items[0]!, { state: "blocked", duplicateIds: ["another-screen"], reason: "IP重复" });
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      await wrapper.findAll("button").find((button) => button.text() === "核对疑似重复")!.trigger("click");
      expect(wrapper.getComponent(ScreenOperations).emitted("merge")).toHaveLength(1);
      expect(wrapper.findComponent(RegistrationPreview).exists()).toBe(false);
    } finally { dispose(); }
  });

  it("注册历史失败重试不依赖普通操作参数，重新进入检查且不直接重提", async () => {
    const { wrapper, adapter, store, dispose } = await render();
    const data = preview(store.projectId, store.selectedScreens);
    vi.spyOn(adapter, "previewPlatformRegistration").mockResolvedValue(data);
    const submit = vi.spyOn(adapter, "submitPlatformRegistration").mockResolvedValue("registration-task");
    const load = adapter.load.bind(adapter);
    vi.spyOn(adapter, "load").mockImplementation(async (projectId) => ({ ...(await load(projectId)), tasks: [{
      id: "registration-task", projectId, action: "register", state: "failed", createdAt: data.createdAt, updatedAt: data.createdAt,
      targets: [{ screenId: data.items[0]!.screenId, name: data.items[0]!.after.name, ip: data.items[0]!.after.ip, state: "failed", progress: 100, message: "模拟失败" }], logs: []
    }] }));
    try {
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      await wrapper.get('[data-testid="screen-operation-submit"]').trigger("click"); await flushPromises();
      const result = wrapper.getComponent(ScreenTaskResults);
      result.vm.$emit("retry", [data.items[0]!.screenId]); await flushPromises();
      expect(store.operation).toBe("register"); expect(store.selectedIds).toEqual([data.items[0]!.screenId]);
      expect(wrapper.findComponent(RegistrationPreview).exists()).toBe(false);
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').text()).toBe("检查并预览");
      expect(submit).toHaveBeenCalledOnce();
    } finally { dispose(); }
  });

  it("历史本机ID已登记为平台ID时重试采用现有记录，重新检查无需更新且不重复创建", async () => {
    const { wrapper, adapter, store, dispose } = await render();
    const check = vi.spyOn(adapter, "previewPlatformRegistration");
    const submit = vi.spyOn(adapter, "submitPlatformRegistration");
    try {
      const localId = "local-screen-4", count = store.snapshot.screens.length;
      store.selectedIds = [localId]; await adapter.setScenario(store.projectId, "write_denied"); await flushPromises();
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      await wrapper.get('[data-testid="screen-operation-submit"]').trigger("click"); await flushPromises();
      expect(wrapper.getComponent(ScreenTaskResults).props("task").state).toBe("failed");
      await adapter.setScenario(store.projectId, "normal");
      const newPreview = await adapter.previewPlatformRegistration(store.projectId, [localId]);
      await adapter.submitPlatformRegistration(store.projectId, { previewId: newPreview.id, screenIds: [localId] });
      await flushPromises();
      const canonical = store.snapshot.screens.find((screen) => screen.aliases.includes(localId))!;
      expect(canonical.source).toBe("platform"); expect(canonical.id).not.toBe(localId);
      const result = wrapper.getComponent(ScreenTaskResults);
      result.vm.$emit("retry", [localId, canonical.id, "removed-local-screen"]); await flushPromises();
      expect(store.selectedIds).toEqual([canonical.id]);
      expect(wrapper.text()).toContain("1 台历史设备记录已不存在，已排除");
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      expect(check.mock.lastCall).toEqual([store.projectId, [canonical.id]]);
      expect(wrapper.getComponent(RegistrationPreview).props("preview").items[0]).toMatchObject({ screenId: canonical.id, state: "skip" });
      expect(wrapper.get('[data-testid="screen-operation-submit"]').attributes("disabled")).toBeDefined();
      expect(submit).toHaveBeenCalledTimes(2);
      expect(store.snapshot.screens).toHaveLength(count);
    } finally { dispose(); }
  });
});
