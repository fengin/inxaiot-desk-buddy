import { defineComponent, h, nextTick, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NButton, NCheckbox, NInputNumber, NMessageProvider } from "naive-ui";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useActivityStore } from "@/stores/activity";
import type { ScreenOperationInput, ScreenPreflightItem, ScreenTask } from "@/shared/model/screen";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import ScreenInspect from "./ScreenInspect.vue";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";

const project = "inspect-ui";
const snapshot = <T,>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
async function render(options: { action?: "ping" | "inspect"; concurrency?: number; retry?: string } = {}) {
  localStorage.clear();
  const adapter = new FixtureScreenAdapter(localStorage, 20), pinia = createPinia();
  configureScreenAdapter(adapter);
  const store = useSmartScreensStore(pinia), activity = useActivityStore(pinia);
  const refreshActivity = vi.spyOn(activity, "refreshTasks").mockResolvedValue();
  await store.bindProject(project);
  const sample = store.snapshot.screens[0]!;
  store.snapshot.screens = ["4", "10", "4"].map((size, index) => ({ ...sample, id: `inspect-${index + 1}`,
    name: `测试屏${index + 1}`, ip: `192.0.2.${index + 1}`, mac: `02:33:44:55:66:0${index + 1}`,
    size: size as "4" | "10", source: "local", aliases: [] }));
  store.selectedIds = store.snapshot.screens.map(screen => screen.id);
  const ids = [...store.selectedIds];
  const step = ref(0), action = ref(options.action ?? "inspect"), concurrency = ref(options.concurrency ?? 3);
  const ready = (input: ScreenOperationInput): ScreenPreflightItem[] => input.targetIds.map(id => {
    const screen = store.snapshot.screens.find(screen => screen.id === id)!;
    return { screenId: id, name: screen.name, ip: screen.ip, state: "ready", reason: "可以检查" };
  });
  const preflight = vi.spyOn(adapter, "preflight").mockImplementation(async (_project, input) => ready(input));
  const taskFor = (input: ScreenOperationInput): ScreenTask => ({ id: "inspect-task", projectId: project, action: input.action,
    state: "running", createdAt: "2026-11-04T00:00:00Z", updatedAt: "2026-11-04T00:00:00Z", input: snapshot(input), logs: [],
    targets: input.targetIds.map(id => { const screen = store.snapshot.screens.find(screen => screen.id === id)!;
      return { screenId: id, name: screen.name, ip: screen.ip, state: "queued", progress: 0, message: "等待派发" }; }) });
  const execute = vi.spyOn(adapter, "execute").mockImplementation(async (_project, input) => {
    const task = taskFor(input); store.snapshot.tasks = [task]; return task.id;
  });
  const load = vi.spyOn(adapter, "load").mockImplementation(async () => ({ ...snapshot(store.snapshot), spacesAvailable: store.snapshot.spacesAvailable ?? false }));
  const cancel = vi.spyOn(adapter, "cancel").mockResolvedValue();
  const status = vi.fn(), versions = vi.fn(), detail = vi.fn(), retry = vi.fn(), reset = vi.fn(), busy = vi.fn();
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, () => h(ScreenInspect, {
    action: action.value, "onUpdate:action": value => { action.value = value; },
    step: step.value, "onUpdate:step": value => { step.value = value; },
    concurrency: concurrency.value, "onUpdate:concurrency": value => { concurrency.value = value; },
    retryOfOperationId: options.retry, onStatus: status, onVersions: versions, onDetail: detail,
    onRetry: retry, onReset: reset, onBusy: busy,
  })) }), { attachTo: document.body, global: { plugins: [pinia] } });
  await flushPromises();
  const start = async () => { await wrapper.get('[data-testid="screen-inspect-start"]').trigger("click"); await flushPromises(); };
  const setTask = async (task: ScreenTask) => { store.snapshot.tasks = [snapshot(task)]; await nextTick(); await flushPromises(); };
  const terminal = async (state: ScreenTask["state"] = "succeeded", ping: "online" | "offline" = "online") => {
    const task = snapshot(store.snapshot.tasks[0]!); task.state = state;
    task.targets = task.targets.map(target => ({ ...target, state: state === "cancelled" ? "cancelled" : state === "failed" ? "failed" : "succeeded",
      progress: 100, message: "读取完成", result: { device: state === "failed" ? "failed" : "succeeded", business: "not_required", shared: "not_required",
        observation: { id: `obs-${target.screenId}`, observedIp: target.ip, observedAt: "2026-11-04T00:00:00Z", ping,
          adbAvailable: task.action === "inspect", abis: [], errors: [] } } }));
    await setTask(task);
  };
  return { wrapper, store, adapter, ids, step, action, concurrency, preflight, execute, load, cancel,
    refreshActivity, status, versions, detail, retry, reset, busy, ready, taskFor, start, setTask, terminal,
    close() { wrapper.unmount(); store.stop(); adapter.dispose(); } };
}

describe("统一设备检查", () => {
  it("默认完整检查、4/10混选，复用首屏面板并绑定实际身份", async () => {
    const c = await render();
    try {
      const mode = c.wrapper.findAllComponents(NCheckbox).find(box => box.attributes("aria-label") === "仅检查网络连通性")!;
      expect(mode.props("checked")).toBe(false);
      expect(c.wrapper.get('[data-testid="screen-operation-selection-summary"]').text()).toContain("4寸 2 台");
      expect(c.wrapper.get('[data-testid="screen-operation-selection-summary"]').text()).toContain("10寸 1 台");
      await c.start();
      const input = c.preflight.mock.calls[0]![1];
      expect(input).toMatchObject({ action: "inspect", concurrency: 3, targetIds: c.ids });
      expect(input.expectedTargets).toEqual(Object.fromEntries(c.store.snapshot.screens.map(screen => [screen.id, screenMaintenanceFingerprint(screen)])));
      expect(c.execute).toHaveBeenCalledExactlyOnceWith(project, input);
      expect(c.step.value).toBe(0); expect(c.wrapper.getComponent(ScreenTargetPicker).props("disabled")).toBe(true);
      expect(c.wrapper.get('[data-testid="screen-inspect-progress"]').text()).toContain("正在检查 0 台");
    } finally { c.close(); }
  });

  it.each([1, 2, 3])("仅网络检查走原ping，并发%s及重试来源保持", async concurrency => {
    const c = await render({ concurrency, retry: "original-ping" });
    try {
      c.wrapper.findAllComponents(NCheckbox).find(box => box.attributes("aria-label") === "仅检查网络连通性")!.vm.$emit("update:checked", true);
      await nextTick(); await c.start();
      expect(c.action.value).toBe("ping");
      expect(c.execute.mock.calls[0]![1]).toMatchObject({ action: "ping", concurrency, retryOfOperationId: "original-ping" });
      expect(c.wrapper.get('[data-testid="screen-inspect-mode"]').text()).toContain("不连接ADB");
    } finally { c.close(); }
  });

  it("预检、创建和后台运行全程首屏；仅按实际running目标显示进度", async () => {
    const c = await render({ concurrency: 2 }), wait = deferred<ScreenPreflightItem[]>();
    c.preflight.mockReturnValueOnce(wait.promise);
    try {
      await c.start();
      expect(c.step.value).toBe(0); expect(c.wrapper.text()).toContain("正在核对所选 3 台"); expect(c.execute).not.toHaveBeenCalled();
      wait.resolve(c.ready(c.preflight.mock.calls[0]![1])); await flushPromises();
      const task = snapshot(c.store.snapshot.tasks[0]!); task.targets[0]!.state = "running"; task.targets[0]!.progress = 20;
      await c.setTask(task);
      expect(c.wrapper.get('[data-testid="screen-inspect-progress"]').text()).toContain("正在检查 1 台");
      expect(c.wrapper.get('[data-testid="screen-inspect-progress"]').text()).toContain("正在检查：测试屏1");
      expect(c.wrapper.get('[data-testid="screen-inspect-progress"]').text()).not.toContain("测试屏2");
      expect(c.wrapper.getComponent(ScreenTargetPicker).props("disabled")).toBe(true);
      task.targets[0]!.state = "succeeded"; task.targets[0]!.progress = 100; task.targets[1]!.state = "running";
      await c.setTask(task);
      expect(c.step.value).toBe(0); expect(c.wrapper.get('[data-testid="screen-inspect-progress"]').text()).toContain("已完成 1/3 台");
      expect(c.wrapper.get('[data-testid="screen-inspect-progress"]').text()).toContain("正在检查：测试屏2");
      await c.terminal();
      expect(c.step.value).toBe(1); expect(c.wrapper.findComponent(ScreenTaskResults).exists()).toBe(true);
      expect(c.busy).toHaveBeenLastCalledWith(false);
      expect(c.refreshActivity).toHaveBeenCalledWith(project, "inspect-task");
    } finally { c.close(); }
  });

  it("连续点击只创建一个任务，运行中模式和并发均禁用", async () => {
    const c = await render(), wait = deferred<ScreenPreflightItem[]>();
    c.preflight.mockReturnValueOnce(wait.promise);
    try {
      const start = c.wrapper.findAllComponents(NButton).find(button => button.attributes("data-testid") === "screen-inspect-start")!;
      start.vm.$emit("click"); start.vm.$emit("click"); await flushPromises();
      expect(c.preflight).toHaveBeenCalledOnce();
      wait.resolve(c.ready(c.preflight.mock.calls[0]![1])); await flushPromises();
      expect(c.execute).toHaveBeenCalledOnce();
      expect(c.wrapper.findAllComponents(NInputNumber).find(input => input.attributes("aria-label") === "并发台数")!.props("disabled")).toBe(true);
      const mode = c.wrapper.findAllComponents(NCheckbox).find(box => box.attributes("aria-label") === "仅检查网络连通性")!;
      expect(mode.props("disabled")).toBe(true); mode.vm.$emit("update:checked", true); await nextTick();
      expect(c.action.value).toBe("inspect"); expect(c.preflight).toHaveBeenCalledOnce();
    } finally { c.close(); }
  });

  it("部分blocked在首屏展示原因，不提交半批，调整范围后能重试", async () => {
    const c = await render();
    c.preflight.mockImplementationOnce(async (_project, input) => c.ready(input).map((row, index) => index === 1 ? { ...row, state: "blocked", reason: "该屏存在未完成任务" } : row));
    try {
      await c.start();
      expect(c.step.value).toBe(0); expect(c.execute).not.toHaveBeenCalled();
      expect(c.wrapper.text()).toContain("该屏存在未完成任务");
      c.store.selectedIds = [c.ids[0]!, c.ids[2]!]; await nextTick(); await c.start();
      expect(c.execute).toHaveBeenCalledOnce(); expect(c.execute.mock.calls[0]![1].targetIds).toEqual([c.ids[0], c.ids[2]]);
    } finally { c.close(); }
  });

  it.each(["preflight", "execute"] as const)("%s接口失败留首屏，旧结果不自动重放", async stage => {
    const c = await render();
    c[stage].mockRejectedValueOnce(new Error("本轮接口失败"));
    try {
      await c.start(); expect(c.step.value).toBe(0); expect(c.wrapper.text()).toContain("本轮接口失败");
      expect(c.wrapper.find('[data-testid="screen-inspect-start"]').attributes("disabled")).toBeUndefined();
      if (stage === "preflight") expect(c.execute).not.toHaveBeenCalled();
      await c.start(); expect(c.preflight).toHaveBeenCalledTimes(2);
      expect(c.execute).toHaveBeenCalledTimes(stage === "execute" ? 2 : 1);
    } finally { c.close(); }
  });

  it("全skip不创建空任务", async () => {
    const c = await render(); c.preflight.mockImplementationOnce(async (_project, input) => c.ready(input).map(row => ({ ...row, state: "skip", reason: "无需检查" })));
    try { await c.start(); expect(c.execute).not.toHaveBeenCalled(); expect(c.step.value).toBe(0); expect(c.wrapper.text()).toContain("无需检查"); }
    finally { c.close(); }
  });

  it.each(["identity", "selection", "action", "concurrency"] as const)("预检期间%s变化丢弃旧响应", async change => {
    const c = await render(), wait = deferred<ScreenPreflightItem[]>(); c.preflight.mockReturnValueOnce(wait.promise);
    try {
      await c.start(); const rows = c.ready(c.preflight.mock.calls[0]![1]);
      if (change === "identity") c.store.snapshot.screens[0]!.ip = "192.0.2.99";
      else if (change === "selection") c.store.selectedIds = c.ids.slice(1);
      else if (change === "action") c.action.value = "ping";
      else c.concurrency.value = 1;
      await nextTick(); wait.resolve(rows); await flushPromises();
      expect(c.execute).not.toHaveBeenCalled(); expect(c.step.value).toBe(0); expect(c.store.openedTaskId).toBe("");
      expect(c.busy).toHaveBeenLastCalledWith(false);
    } finally { c.close(); }
  });

  it.each(["preflight", "execute"] as const)("%s期间切项目不回填旧任务", async stage => {
    const c = await render(), wait = stage === "preflight" ? deferred<ScreenPreflightItem[]>() : deferred<string>();
    if (stage === "preflight") c.preflight.mockReturnValueOnce(wait.promise as Promise<ScreenPreflightItem[]>);
    else c.execute.mockReturnValueOnce(wait.promise as Promise<string>);
    try {
      await c.start(); const result = stage === "preflight" ? c.ready(c.preflight.mock.calls[0]![1]) : "old-project-task";
      await c.store.bindProject("another-project"); await nextTick();
      (wait.resolve as (value: ScreenPreflightItem[] | string) => void)(result); await flushPromises();
      expect(c.step.value).toBe(0); expect(c.store.openedTaskId).toBe("");
      if (stage === "preflight") expect(c.execute).not.toHaveBeenCalled();
      expect(c.busy).toHaveBeenLastCalledWith(false);
    } finally { c.close(); }
  });

  it("Ping正常离线是已完成结果，不当作任务失败或触发平台更新", async () => {
    const c = await render({ action: "ping" });
    const cover = vi.spyOn(c.adapter, "coverStatus");
    try {
      await c.start(); await c.terminal("succeeded", "offline");
      expect(c.step.value).toBe(1); expect(c.wrapper.getComponent(ScreenTaskResults).props("task").state).toBe("succeeded");
      expect(c.wrapper.text()).toContain("离线"); expect(c.wrapper.find('[role="alert"]').exists()).toBe(false);
      expect(cover).not.toHaveBeenCalled(); expect(c.status).not.toHaveBeenCalled();
    } finally { c.close(); }
  });

  it.each(["failed", "partially_succeeded"] as const)("正式任务%s仍进入结果页供原流程处理", async state => {
    const c = await render();
    try { await c.start(); await c.terminal(state); expect(c.step.value).toBe(1); expect(c.wrapper.findComponent(ScreenTaskResults).exists()).toBe(true); }
    finally { c.close(); }
  });

  it("取消仅作用于本任务，继续等待正式取消终态", async () => {
    const c = await render();
    try {
      await c.start();
      await c.wrapper.get('[data-testid="screen-inspect-cancel"]').trigger("click"); await flushPromises();
      expect(c.cancel).toHaveBeenCalledExactlyOnceWith(project, "inspect-task"); expect(c.step.value).toBe(0);
      expect(c.wrapper.get('[data-testid="screen-inspect-cancel"]').attributes("disabled")).toBeDefined();
      await c.terminal("cancelled"); expect(c.step.value).toBe(1); expect(c.busy).toHaveBeenLastCalledWith(false);
    } finally { c.close(); }
  });

  it("结果操作只透传，开始新检查清理当前页面上下文", async () => {
    const c = await render();
    try {
      await c.start(); await c.terminal();
      const result = c.wrapper.getComponent(ScreenTaskResults);
      result.vm.$emit("status"); result.vm.$emit("versions", c.ids); result.vm.$emit("detail", c.ids[0]); result.vm.$emit("retry", [c.ids[0]]);
      expect(c.status).toHaveBeenCalledOnce(); expect(c.versions).toHaveBeenCalledWith(c.ids); expect(c.detail).toHaveBeenCalledWith(c.ids[0]);
      expect(c.retry).toHaveBeenCalledWith(expect.objectContaining({ id: "inspect-task", action: "inspect" }), [c.ids[0]]);
      await c.wrapper.get('[data-testid="screen-inspect-new"]').trigger("click"); await flushPromises();
      expect(c.step.value).toBe(0); expect(c.reset).toHaveBeenCalledOnce(); expect(c.store.openedTaskId).toBe("");
    } finally { c.close(); }
  });
});
