import { createPinia } from "pinia";
import { defineComponent, h } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NCheckbox, NMessageProvider } from "naive-ui";
import { describe, expect, it, vi } from "vitest";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { screenActions, screenActionLabel, type ScreenOperationInput } from "@/shared/model/screen";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ScreenOperations from "./ScreenOperations.vue";
import ScreenAppConfig from "./ScreenAppConfig.vue";

const project = "sleep-off-ui";
const four = "platform-screen-19", ten = "platform-screen-2", unavailable = "platform-screen-17";
const input = (targetIds: string[]): ScreenOperationInput => ({
  action: "sleep_off", targetIds, appVersion: "", abi: "universal", reinstall: false, concurrency: 3
});

async function finish(adapter: FixtureScreenAdapter, taskId: string) {
  await vi.waitFor(async () => {
    expect((await adapter.load(project)).tasks.find(task => task.id === taskId)?.state).toBe("succeeded");
  }, { timeout: 2000 });
}

async function render(closed: string[] = []) {
  const adapter = new FixtureScreenAdapter(localStorage, 10);
  configureScreenAdapter(adapter);
  configureActivityAdapter(new FixtureActivityAdapter(adapter));
  if (closed.length) await finish(adapter, await adapter.execute(project, input(closed)));
  const pinia = createPinia();
  const store = useSmartScreensStore(pinia);
  await store.bindProject(project);
  store.selectedIds = [four, ten]; store.operation = "sleep_off";
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, () => h(ScreenOperations)) }), {
    attachTo: document.body, global: { plugins: [pinia] }
  });
  await flushPromises();
  return { wrapper, adapter, store, close: () => { wrapper.unmount(); store.stop(); adapter.dispose(); } };
}

describe("关闭屏睡眠的独立系统操作", () => {
  it("4寸和10寸可混选，沿用选屏、检查、结果布局，确认前不执行", async () => {
    const context = await render();
    const { wrapper, adapter, store } = context;
    const execute = vi.spyOn(adapter, "execute"), preflight = vi.spyOn(adapter, "preflight");
    try {
      expect(screenActions.some(action => action.value === "sleep_off")).toBe(false);
      expect(screenActionLabel("sleep_off")).toBe("关闭屏睡眠");
      expect(store.selectedScreens.map(screen => screen.size)).toEqual(["10", "4"]);
      expect(wrapper.findComponent(ScreenAppConfig).exists()).toBe(false);
      expect(wrapper.find(".screen-select-stage .screen-target-picker").exists()).toBe(true);
      const panel = wrapper.get(".screen-select-stage .screen-parameter-panel");
      expect(panel.text()).toContain("持续供电时不自动息屏，仍可能调暗");
      expect(panel.text()).toContain("不修改原超时，不关闭H5屏保，不重启屏或小新");
      expect(panel.find('[data-testid="screen-apk-input"]').exists()).toBe(false);
      expect(panel.get('[data-testid="screen-operation-first-action"]').attributes("disabled")).toBeUndefined();
      await panel.get('[data-testid="screen-operation-first-action"]').trigger("click");
      await vi.waitFor(() => expect(wrapper.find(".screen-check-stage").exists()).toBe(true));
      expect(preflight.mock.calls[0]?.[1].targetIds).toEqual([four, ten]);
      expect(execute).not.toHaveBeenCalled();
      expect(wrapper.findAll(".screen-check-row")).toHaveLength(2);
      expect(wrapper.findAll(".screen-check-row").every(row => row.text().includes("待关闭"))).toBe(true);
      expect(wrapper.get('[data-testid="screen-operation-submit"]').text()).toBe("确认关闭屏睡眠 2 台");
      await wrapper.get('[data-testid="screen-operation-submit"]').trigger("click");
      await vi.waitFor(() => expect(store.snapshot.tasks[0]?.state).toBe("succeeded"));
      expect(execute).toHaveBeenCalledOnce();
      expect(execute.mock.calls[0]?.[1].targetIds).toEqual([four, ten]);
      expect(wrapper.get(".screen-final-stage").text()).toContain("未关闭 → 已关闭");
      expect(wrapper.get(".screen-final-stage").text()).toContain("未修改原自动息屏时长");
      expect(wrapper.find(".screen-result-table-body.inx-scroll-area").exists()).toBe(true);
    } finally { context.close(); }
  });

  it("待关闭、已关闭、不可执行逐台区分，只允许提交勾选的待关闭设备", async () => {
    const context = await render([ten]);
    const { wrapper, adapter, store } = context;
    store.selectedIds = [four, ten, unavailable]; await flushPromises();
    const execute = vi.spyOn(adapter, "execute");
    try {
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click");
      await vi.waitFor(() => expect(wrapper.find(".screen-check-stage").exists()).toBe(true));
      const row = (id: string) => wrapper.get(`.screen-check-stage [data-screen-id="${id}"]`);
      expect(row(four).text()).toContain("待关闭");
      expect(row(four).text()).toContain("尚未关闭，将设置接电保持唤醒");
      expect(row(four).get("td:nth-child(4)").classes()).toContain("screen-warning");
      expect(row(four).getComponent(NCheckbox).props("checked")).toBe(true);
      expect(row(ten).text()).toContain("已关闭屏睡眠（接电保持唤醒已生效），无需重复设置");
      expect(row(ten).getComponent(NCheckbox).props("checked")).toBe(false);
      expect(row(ten).getComponent(NCheckbox).props("disabled")).toBe(true);
      expect(row(ten).get("td:nth-child(4)").classes()).toContain("screen-success");
      expect(row(unavailable).text()).toContain("不可执行");
      expect(row(unavailable).get("td:nth-child(4)").classes()).toContain("screen-danger");
      expect(row(unavailable).getComponent(NCheckbox).props("disabled")).toBe(true);
      expect(wrapper.get(".screen-check-stage .screen-result-header .screen-warning").text()).toBe("1 台待关闭");
      expect(wrapper.get(".screen-check-stage .screen-result-header .screen-success").text()).toBe("1 台已关闭");
      row(ten).getComponent(NCheckbox).vm.$emit("update:checked", true);
      row(unavailable).getComponent(NCheckbox).vm.$emit("update:checked", true);
      await flushPromises();
      expect(wrapper.get('[data-testid="screen-operation-submit"]').text()).toBe("确认关闭屏睡眠 1 台");
      await wrapper.get('[data-testid="screen-operation-submit"]').trigger("click");
      await vi.waitFor(() => expect(execute).toHaveBeenCalledOnce());
      expect(execute.mock.calls[0]?.[1].targetIds).toEqual([four]);
      await vi.waitFor(() => expect(store.snapshot.tasks[0]?.state).toBe("succeeded"));
      expect(store.snapshot.tasks[0]?.targets.map(target => target.screenId)).toEqual([four]);
    } finally { context.close(); }
  });

  it("全部已关闭时明确无需执行，不创建空任务或重复写入", async () => {
    const context = await render([four, ten]);
    const { wrapper, adapter, store } = context;
    const execute = vi.spyOn(adapter, "execute");
    const taskCount = store.snapshot.tasks.length;
    try {
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click");
      await vi.waitFor(() => expect(wrapper.find(".screen-check-stage").exists()).toBe(true));
      expect(wrapper.get('[data-testid="screen-sleep-off-summary"]').text()).toBe("所选 2 台均已关闭屏睡眠，无需执行。");
      const submit = wrapper.get('[data-testid="screen-operation-submit"]');
      expect(submit.text()).toBe("均已关闭，无需执行");
      expect(submit.attributes("disabled")).toBeDefined();
      expect(wrapper.findAll(".screen-check-row").every(row => row.getComponent(NCheckbox).props("checked") === false)).toBe(true);
      await submit.trigger("click"); await flushPromises();
      expect(execute).not.toHaveBeenCalled();
      expect(store.snapshot.tasks).toHaveLength(taskCount);
    } finally { context.close(); }
  });

  it("模拟权限不足逐台阻断，模拟关闭状态可重载且不进入设备业务字段", async () => {
    const adapter = new FixtureScreenAdapter(localStorage, 10);
    let reloaded: FixtureScreenAdapter | undefined;
    try {
      await adapter.setScenario(project, "write_denied");
      const denied = await adapter.preflight(project, input([four, ten]));
      expect(denied.every(row => row.state === "blocked" && row.reason.includes("系统设置权限"))).toBe(true);
      await expect(adapter.execute(project, input([four, ten]))).rejects.toThrow("只选择检查通过");
      expect((await adapter.load(project)).tasks).toHaveLength(0);
      await adapter.setScenario(project, "normal");
      await finish(adapter, await adapter.execute(project, input([four, ten])));
      adapter.dispose();
      reloaded = new FixtureScreenAdapter(localStorage, 10);
      expect((await reloaded.preflight(project, input([four, ten]))).map(row => row.state)).toEqual(["skip", "skip"]);
      await expect(reloaded.execute(project, input([four, ten]))).rejects.toThrow("只选择检查通过");
      const state = await reloaded.load(project);
      expect(state.tasks).toHaveLength(1);
      expect(state.screens.every(screen => !("sleepDisabled" in screen))).toBe(true);
      await reloaded.reset(project);
      expect((await reloaded.preflight(project, input([four, ten]))).map(row => row.state)).toEqual(["ready", "ready"]);
    } finally { adapter.dispose(); reloaded?.dispose(); }
  });
});
