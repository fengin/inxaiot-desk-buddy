import { createPinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useActivityStore } from "@/stores/activity";
import { useProjectStore } from "@/stores/projects";
import { NCheckbox, NPagination, NSelect } from "naive-ui";
import type { ScreenTask } from "@/shared/model/screen";
import ScreenDetail from "./ScreenDetail.vue";
import ScreenLocalEditor from "./ScreenLocalEditor.vue";
import ScreenStatusDialog from "./ScreenStatusDialog.vue";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";
import { getProjectSpacePath, projectSpacePath } from "@/shared/model/projectSpace";
import { SCREEN_UNLOCATED_SPACE_KEY } from "@/shared/model/screenSpace";
import ScreenSpaceSelect from "./ScreenSpaceSelect.vue";

async function chooseApk(wrapper: VueWrapper, file = new File(['apk-test'], 'xiaoxin.apk', { lastModified: 1 })) {
  const input = wrapper.get<HTMLInputElement>('[data-testid="screen-apk-input"]');
  Object.defineProperty(input.element, 'files', { configurable: true, value: [file] });
  await input.trigger('change'); await flushPromises();
}

async function render() {
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() })) });
  const adapter = new FixtureScreenAdapter(localStorage, 20);
  configureScreenAdapter(adapter); configureActivityAdapter(new FixtureActivityAdapter(adapter)); configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
  const { default: App } = await import("@/app/App.vue");
  await router.push("/screen/nodes"); await router.isReady();
  const pinia = createPinia();
  const wrapper = mount(App, { attachTo: document.body, global: { plugins: [pinia, router, i18n], stubs: { teleport: true } } });
  await flushPromises();
  return { wrapper, adapter, store: useSmartScreensStore(pinia), activity: useActivityStore(pinia), projects: useProjectStore(pinia) };
}

describe("智能屏交互原型页面", () => {
  it.each(["ping", "inspect", "mac"] as const)("%s 完成后从结果打开屏详情，关闭后保留结果且不重复检查", async (action) => {
    const { wrapper, adapter, store } = await render();
    const execute = vi.spyOn(adapter, "execute");
    try {
      const id = "platform-screen-3";
      store.selectedIds = [id]; store.operation = action;
      await router.push("/screen/operations"); await flushPromises();
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click");
      await vi.waitFor(() => expect(store.snapshot.tasks[0]?.state).toBe("succeeded"), { timeout: 2500 });
      const taskId = store.openedTaskId;
      const result = wrapper.get(".screen-final-stage");
      await result.get(`[data-screen-id="${id}"] td:last-child button`).trigger("click");
      await flushPromises();
      const detail = wrapper.getComponent(ScreenDetail);
      const screen = store.snapshot.screens.find(screen => screen.id === id)!;
      expect(detail.props("screen")).toEqual(screen);
      expect(detail.text()).toContain(screen.ip);
      expect(detail.text()).toContain("最近检查");
      expect(detail.text()).toContain(action === "ping" ? "本机 IP 检查" : action === "mac" ? "采集 MAC" : "系统 / 架构");
      await detail.findAll("button").find(button => button.text() === "关闭")!.trigger("click");
      await flushPromises();
      expect(detail.props("screen")).toBeUndefined();
      expect(router.currentRoute.value.path).toBe("/screen/operations");
      expect(store.openedTaskId).toBe(taskId);
      expect(wrapper.get(".screen-final-stage").text()).toContain("查看屏详情");
      if (action === "ping") {
        expect(result.get("tbody .n-tag").text()).toBe("离线");
        screen.ping = "online"; await flushPromises();
        expect(result.get("tbody .n-tag").text()).toBe("离线");
      }
      expect(execute).toHaveBeenCalledOnce();
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("本机历史按旧 ID 打开最新屏详情，关闭后保留页码；已移除记录不按同 IP 误关联", async () => {
    const { wrapper, adapter, store } = await render();
    const execute = vi.spyOn(adapter, "execute");
    try {
      store.stop();
      const seed = store.snapshot.screens[0]!;
      store.snapshot.screens = [{ ...seed, id: "current-screen", aliases: ["old-screen"], name: "已登记的屏", ping: "offline", checkedAt: "2026-10-08T04:00:00Z" }];
      store.filters.keyword = "隐藏全部屏";
      const task: ScreenTask = {
        id: "old-check", projectId: store.projectId, action: "ping", mode: "real", state: "succeeded",
        createdAt: "2026-10-07T00:00:00Z", updatedAt: "2026-10-07T00:00:00Z", logs: [],
        targets: Array.from({ length: 22 }, (_, index) => ({
          screenId: index === 20 ? "old-screen" : `removed-${index}`, name: seed.name, ip: seed.ip,
          state: "succeeded", progress: 100, message: "检查完成，结果已保存在本机"
        }))
      };
      store.snapshot.tasks = [task];
      await router.push("/screen/operations"); await flushPromises();
      await wrapper.findAll("button").find(button => button.text() === "查看历史记录")!.trigger("click");
      await flushPromises();
      const history = wrapper.get(".screen-local-result-detail");
      const pagination = history.getComponent(NPagination);
      pagination.vm.$emit("update:page-size", 20); await flushPromises();
      pagination.vm.$emit("update:page", 2); await flushPromises();
      await history.get('[data-screen-id="old-screen"] td:last-child button').trigger("click");
      await flushPromises();
      const detail = wrapper.getComponent(ScreenDetail);
      expect(detail.props("screen")?.id).toBe("current-screen");
      expect(detail.text()).toContain("不可达");
      expect(detail.text()).toContain("2026-10-08");
      await detail.findAll("button").find(button => button.text() === "关闭")!.trigger("click");
      await flushPromises();
      expect(history.element.isConnected).toBe(true);
      expect(pagination.props("page")).toBe(2);
      await history.get('[data-screen-id="removed-21"] td:last-child button').trigger("click");
      await flushPromises();
      expect(detail.props("screen")).toBeUndefined();
      expect(wrapper.text()).toContain("该屏记录已移除，仍可查看本次操作结果和日志。");
      expect(pagination.props("page")).toBe(2);
      expect(execute).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("楼幢筛选覆盖直属和深层空间，目录不可用时保留待核验关联", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      const deep = store.snapshot.spaces.find((space) => (getProjectSpacePath(store.snapshot.spaces, space.id)?.length ?? 0) >= 4)!;
      const path = getProjectSpacePath(store.snapshot.spaces, deep.id)!;
      const building = path.find((space) => space.kind === 'building')!;
      const floor = path.find((space) => space.kind === 'floor')!;
      const base = store.snapshot.screens.find((screen) => screen.source === 'local')!;
      const deepPath = projectSpacePath(store.snapshot.spaces, deep.id);
      store.snapshot.screens = [
        { ...base, id: 'direct-building', name: '楼幢直属屏', spaceId: building.id, spacePath: projectSpacePath(store.snapshot.spaces, building.id) },
        { ...base, id: 'deep-space', name: '深层空间屏', spaceId: deep.id, spacePath: deepPath },
        { ...base, id: 'unlocated', name: '待定屏', spaceId: null, spacePath: '' }
      ];
      store.filters.space = building.id; await flushPromises();
      expect(store.filtered.map((screen) => screen.id)).toEqual(['direct-building', 'deep-space']);
      expect(wrapper.get('.screen-table tbody').text()).toContain(deepPath);
      store.filters.space = floor.id; await flushPromises();
      expect(store.filtered.map((screen) => screen.id)).toEqual(['deep-space']);

      await router.push('/screen/operations'); await flushPromises();
      const picker = wrapper.getComponent(ScreenTargetPicker);
      picker.getComponent(ScreenSpaceSelect).vm.$emit('update:modelValue', building.id); await flushPromises();
      expect(picker.findAll('.screen-target-row')).toHaveLength(2);
      await picker.get('.target-search input').setValue(deepPath); await flushPromises();
      expect(picker.findAll('.screen-target-row')).toHaveLength(1);
      expect(picker.get('.node-selection-location').attributes('title')).toContain(deepPath);

      await router.push('/screen/nodes'); store.filters.space = ''; store.snapshot.spacesAvailable = false; await flushPromises();
      expect(wrapper.get('.screen-table tbody').text()).toContain(`${deepPath}（待核验）`);
      store.filters.space = SCREEN_UNLOCATED_SPACE_KEY; await flushPromises();
      expect(store.filtered.map((screen) => screen.id)).toEqual(['unlocated']);
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("安装混选尺寸时禁用文件与检查，筛选隐藏不同尺寸也不能绕过", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      store.selectedIds = ['platform-screen-2', 'platform-screen-19'];
      await router.push('/screen/operations'); await flushPromises();
      expect(wrapper.get('[data-testid="screen-apk-size-warning"]').text()).toBe('一批只能操作同规格尺寸屏');
      expect(wrapper.get('[data-testid="screen-apk-picker"]').attributes('disabled')).toBeDefined();
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').attributes('disabled')).toBeDefined();
      const picker = wrapper.getComponent(ScreenTargetPicker);
      picker.findAllComponents(NSelect).find((select) => select.attributes('aria-label') === '运维屏尺寸')!.vm.$emit('update:value', '10');
      await flushPromises();
      expect(picker.findAll('.screen-target-row.selected')).toHaveLength(1);
      expect(store.selectedIds).toHaveLength(2);
      expect(wrapper.get('[data-testid="screen-apk-size-warning"]').text()).toBe('一批只能操作同规格尺寸屏');
      store.operation = 'ping'; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-size-warning"]').exists()).toBe(false);
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').attributes('disabled')).toBeUndefined();
      store.operation = 'install'; store.selectedIds = ['platform-screen-2', 'platform-screen-3']; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-size-warning"]').exists()).toBe(false);
      expect(wrapper.get('[data-testid="screen-apk-picker"]').attributes('disabled')).toBeUndefined();
      await chooseApk(wrapper);
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').attributes('disabled')).toBeUndefined();
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("已有 APK 后混选或尺寸未知会清除文件，同尺寸恢复后必须重选", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      store.selectedIds = ['platform-screen-2']; await router.push('/screen/operations'); await flushPromises();
      await chooseApk(wrapper);
      store.selectedIds = ['platform-screen-2', 'platform-screen-19']; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
      expect(store.selectedIds).toHaveLength(2);
      store.selectedIds = ['platform-screen-2']; await flushPromises();
      expect(wrapper.get('[data-testid="screen-apk-picker"]').attributes('disabled')).toBeUndefined();
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').attributes('disabled')).toBeDefined();
      await chooseApk(wrapper);
      store.snapshot.screens.find((screen) => screen.id === 'platform-screen-2')!.size = 'unknown'; await flushPromises();
      expect(wrapper.get('[data-testid="screen-apk-size-warning"]').text()).toBe('请先确认所选屏尺寸');
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("先选包后选屏绑定尺寸，清空目标保留绑定，换尺寸必须重选包", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      await router.push('/screen/operations'); await flushPromises();
      await chooseApk(wrapper);
      store.selectedIds = ['platform-screen-2']; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(true);
      store.selectedIds = []; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(true);
      store.selectedIds = ['platform-screen-3']; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(true);
      store.selectedIds = []; await flushPromises();
      store.selectedIds = ['platform-screen-19']; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
      expect(wrapper.get('.screen-ops-alert').text()).toContain('所选屏尺寸已变化，请重新选择对应 APK 文件');
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').attributes('disabled')).toBeDefined();
      await chooseApk(wrapper);
      store.selectedIds = []; await flushPromises();
      await chooseApk(wrapper, new File(['new'], 'another-size.apk'));
      store.selectedIds = ['platform-screen-2']; await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(true);
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("已完成安装检查后实际尺寸变化会使检查失效并返回选屏", async () => {
    const { wrapper, adapter, store } = await render();
    const execute = vi.spyOn(adapter, 'execute');
    try {
      store.selectedIds = ['platform-screen-2']; await router.push('/screen/operations'); await flushPromises();
      await chooseApk(wrapper); await wrapper.get('[data-testid="screen-operation-first-action"]').trigger('click');
      await vi.waitFor(() => expect(wrapper.find('.screen-check-stage').exists()).toBe(true), { timeout: 2500 });
      store.snapshot.screens.find((screen) => screen.id === 'platform-screen-2')!.size = '4'; await flushPromises();
      expect(wrapper.find('.screen-check-stage').exists()).toBe(false);
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
      expect(wrapper.get('.screen-ops-alert').text()).toContain('所选屏尺寸已变化，请重新选择对应 APK 文件');
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').attributes('disabled')).toBeDefined();
      expect(execute).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("首步仅保留四项安装输入，动作位于右面板底部，换包需重新检查", async () => {
    const { wrapper, adapter, store } = await render();
    const preflight = vi.spyOn(adapter, 'preflight');
    try {
      store.selectedIds = ['platform-screen-2'];
      await router.push('/screen/operations'); await flushPromises();
      const panel = wrapper.get('.screen-parameter-panel');
      expect(panel.text()).toContain('选择应用'); expect(panel.text()).toContain('本地 APK 文件');
      expect(panel.text()).toContain('相同版本覆盖安装'); expect(panel.text()).toContain('并发台数');
      expect(panel.text()).not.toContain('目标版本'); expect(panel.text()).not.toContain('安装包架构');
      expect(panel.text()).not.toContain('最多同时'); expect(panel.text()).not.toContain('本次操作不会连接真实设备');
      expect(panel.get('.screen-parameter-action button').text()).toBe('安装检查');
      expect(panel.get('.screen-parameter-action button').attributes('disabled')).toBeDefined();
      expect(wrapper.find('.screen-ops-footer').exists()).toBe(false);
      await chooseApk(wrapper, new File(['first'], 'first.apk', { lastModified: 1 }));
      await panel.get('.screen-parameter-action button').trigger('click');
      await vi.waitFor(() => expect(wrapper.find('.screen-check-stage').exists()).toBe(true), { timeout: 2500 });
      expect(wrapper.get('[data-testid="screen-apk-check-boundary"]').text()).toContain('尚未解析包名、版本或架构');
      expect(wrapper.find('.screen-ops-footer').exists()).toBe(true);
      await wrapper.findAll('.screen-ops-footer button').find((button) => button.text() === '上一步')!.trigger('click');
      await chooseApk(wrapper, new File(['second'], 'second.apk', { lastModified: 2 }));
      expect(wrapper.find('.screen-check-stage').exists()).toBe(false);
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger('click');
      await vi.waitFor(() => expect(preflight).toHaveBeenCalledTimes(2), { timeout: 2500 });
      expect(preflight.mock.calls[1]![1]).toMatchObject({ applicationId: 'xiaoxin', appVersion: '', abi: 'universal', apk: { name: 'second.apk', size: 6, lastModified: 2 } });
      expect(preflight.mock.calls[1]![1].apk).not.toBeInstanceOf(File);
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("安装重试必须重选本地 APK，历史描述不能作为仍可读取的文件", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      store.selectedIds = ['platform-screen-2']; await router.push('/screen/operations'); await flushPromises();
      await chooseApk(wrapper);
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger('click');
      await vi.waitFor(() => expect(wrapper.find('.screen-check-stage').exists()).toBe(true), { timeout: 2500 });
      await wrapper.findAll('.screen-ops-footer button').find((button) => button.text().includes('开始安装'))!.trigger('click');
      await vi.waitFor(() => expect(store.snapshot.tasks[0]?.state).toBe('succeeded'), { timeout: 2500 });
      wrapper.getComponent(ScreenTaskResults).vm.$emit('retry', ['platform-screen-2']); await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').attributes('disabled')).toBeDefined();
      expect(store.selectedIds).toEqual(['platform-screen-2']);
      expect(wrapper.text()).toContain('请重新选择本地 APK 文件后检查');
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("切换项目清理所选 APK 并忽略旧检查结果", async () => {
    const { wrapper, adapter, store, projects } = await render();
    let finish!: () => void;
    const gate = new Promise<void>((resolve) => { finish = resolve; });
    const original = adapter.preflight.bind(adapter);
    const preflight = vi.spyOn(adapter, 'preflight').mockImplementation(async (projectId, input) => { const rows = await original(projectId, input); await gate; return rows; });
    try {
      store.selectedIds = ['platform-screen-2']; await router.push('/screen/operations'); await flushPromises();
      await chooseApk(wrapper); await wrapper.get('[data-testid="screen-operation-first-action"]').trigger('click');
      await vi.waitFor(() => expect(preflight).toHaveBeenCalledOnce(), { timeout: 2500 });
      await projects.switchProject('project-chengdu-center'); await flushPromises();
      finish(); await flushPromises();
      expect(wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
      expect(wrapper.find('.screen-check-stage').exists()).toBe(false);
      expect(wrapper.find('.screen-ops-footer').exists()).toBe(false);
    } finally { finish(); wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("切换项目后旧提交迟到返回不覆盖新项目任务", async () => {
    const { wrapper, adapter, store, projects } = await render();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const original = adapter.execute.bind(adapter);
    const submit = vi.spyOn(adapter, 'execute').mockImplementation(async (projectId, input) => { const id = await original(projectId, input); await gate; return id; });
    try {
      const previousProject = store.projectId;
      store.selectedIds = ['platform-screen-1'];
      await router.push('/screen/operations'); await flushPromises();
      await chooseApk(wrapper);
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger('click');
      await vi.waitFor(() => expect(wrapper.text()).toContain('开始安装 1 台'), {timeout:2500});
      await wrapper.findAll('button').find((button) => button.text().includes('开始安装 1 台'))!.trigger('click');
      await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce());
      await projects.switchProject('project-chengdu-center'); await flushPromises();
      expect(store.projectId).toBe('project-chengdu-center');
      release(); await flushPromises();
      expect(store.openedTaskId).toBe('');
      expect((await adapter.load(previousProject)).tasks).toHaveLength(1);
    } finally { release(); wrapper.unmount(); adapter.dispose(); }
  }, 30000);
  it("在既有桌面框架渲染列表、搜索并显示空结果", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      expect(wrapper.text()).toContain("交互原型"); expect(wrapper.text()).toContain("一体机管理");
      expect(wrapper.find('[aria-label="演示场景"]').exists()).toBe(false);
      expect(wrapper.findAll('.screen-summary-strip .summary-item')).toHaveLength(6);
      expect(wrapper.text()).toContain("待定空间");
      expect(wrapper.findAll(".screen-table tbody tr")).toHaveLength(20);
      await wrapper.get('.screen-search input').setValue("样板间");
      await flushPromises();
      expect(store.filtered).toHaveLength(1); expect(wrapper.findAll(".screen-table tbody tr")).toHaveLength(1);
      await wrapper.get('.screen-search input').setValue("不存在的设备");
      await flushPromises(); expect(wrapper.text()).toContain("没有符合条件的屏");
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("多选目标到检查、执行、结果与公共任务面板形成闭环", async () => {
    const { wrapper, adapter, store, activity } = await render();
    try {
      await wrapper.get('.screen-table tbody tr .n-checkbox').trigger("click");
      expect(store.selectedIds).toHaveLength(1);
      await wrapper.findAll("button").find((button) => button.text().includes("批量操作"))!.trigger("click");
      await flushPromises();
      expect(wrapper.get('[data-testid="screen-operation-first-action"]').text()).toBe("安装检查");
      expect(wrapper.find('.screen-ops-footer').exists()).toBe(false);
      await chooseApk(wrapper);
      await wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click");
      await vi.waitFor(() => expect(wrapper.text()).toContain("开始安装 1 台"), { timeout: 2500 });
      await wrapper.findAll("button").find((button) => button.text().includes("开始安装 1 台"))!.trigger("click");
      await vi.waitFor(() => expect(store.snapshot.tasks[0]?.state).toBe("succeeded"), { timeout: 2500 });
      expect(store.snapshot.tasks[0]?.input).toMatchObject({ applicationId: 'xiaoxin', apk: { name: 'xiaoxin.apk', size: 8, lastModified: 1 }, appVersion: '', abi: 'universal' });
      await wrapper.findAll("button").find((button) => button.text().includes("任务与日志面板"))!.trigger("click");
      await flushPromises();
      expect(activity.panelTab).toBe("logs"); expect(activity.tasks.some((task) => task.domainType === "screen")).toBe(true);
      expect(wrapper.text()).toContain("不会连接设备或修改数据库");
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("操作步骤随功能变化，筛选保留已选，历史从标题打开", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      await router.push('/screen/operations'); await flushPromises();
      expect(wrapper.find('.screen-operation-tabs').exists()).toBe(false);
      expect(wrapper.findAll('.screen-operation-flow .process-step')).toHaveLength(3);
      const actionSelect = () => wrapper.findAllComponents(NSelect).find((select) => select.attributes('aria-label') === '选择智能屏操作')!;
      actionSelect().vm.$emit('update:value','ping'); await flushPromises();
      expect(wrapper.findAll('.screen-operation-flow .process-step')).toHaveLength(2);
      actionSelect().vm.$emit('update:value','adb'); await flushPromises();
      expect(wrapper.findAll('.screen-operation-flow .process-step')).toHaveLength(4);
      const picker = wrapper.findComponent(ScreenTargetPicker);
      await picker.get('.screen-target-row .n-checkbox').trigger('click');
      expect(store.selectedIds).toHaveLength(1);
      await picker.get('.target-search input').setValue('样板间'); await flushPromises();
      expect(picker.findAll('.screen-target-row')).toHaveLength(1); expect(store.selectedIds).toHaveLength(1);
      expect(picker.text()).toContain('另有 1 台已选');
      await wrapper.findAll('button').find((button) => button.text() === '查看历史记录')!.trigger('click'); await flushPromises();
      expect(wrapper.text()).toContain('智能屏运维历史记录');
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("新增智能屏用单个空间树选择完整路径，保存空间ID与独立安装位置", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      await wrapper.findAll('button').find((button) => button.text() === '新增智能屏')!.trigger('click'); await flushPromises();
      const editor = wrapper.findComponent(ScreenLocalEditor);
      await editor.get('input[placeholder="例如：192.0.2.120"]').setValue('192.0.2.199');
      const node = store.snapshot.spaces.find((space) => (getProjectSpacePath(store.snapshot.spaces, space.id)?.length ?? 0) >= 4)!;
      expect(node).toBeDefined();
      const selector = editor.getComponent(ProjectSpaceSelect);
      selector.vm.$emit('update:modelValue', node.id); await flushPromises();
      await editor.get('input[placeholder="例如：东区电梯出口右侧"]').setValue('入口右侧墙面');
      await editor.get('form').trigger('submit'); await flushPromises();
      const created = store.snapshot.screens.find((screen) => screen.ip === '192.0.2.199')!;
      expect(created.spaceId).toBe(node.id);
      expect(created.spacePath).toBe(projectSpacePath(store.snapshot.spaces, node.id));
      expect(created.location).toBe('入口右侧墙面');
      store.filters.keyword = node.name; await flushPromises();
      expect(store.filtered.some((screen) => screen.id === created.id)).toBe(true);
      expect(wrapper.get('.screen-table tbody').text()).toContain(created.spacePath);
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("差异箭头从本机检查值指向平台待更新值，确认后平台与本机一致", async () => {
    const { wrapper, adapter, store } = await render();
    const cover = vi.spyOn(adapter, "coverStatus");
    try {
      expect(store.statusDifferences.length).toBeGreaterThan(0);
      const before = store.statusDifferences.map((screen) => ({ ...screen }));
      await wrapper.findAll(".screen-notice").find((item) => item.text().includes("状态不一致"))!.trigger("click");
      await flushPromises();
      expect(wrapper.text()).toContain("保留平台状态");
      const dialog = wrapper.findComponent(ScreenStatusDialog);
      expect(dialog.text()).not.toContain("浏览器");
      for (const screen of before) {
        const transition = dialog.get(`[data-screen-id="${screen.id}"] [data-testid="screen-status-transition"]`);
        expect(transition.get(".screen-status-source").text()).toBe(screen.ping === "online" ? "本机可达" : "本机不可达");
        expect(transition.get(".screen-status-target").text()).toContain(screen.ping === "online" ? "平台将在线" : "平台将离线");
        expect(transition.get(".screen-status-original").text()).toBe(`平台原值：${screen.platformStatus === "online" ? "在线" : "离线"}`);
        const arrow = transition.get('svg[aria-label="以本机检查值更新平台"]');
        expect(transition.get(".screen-status-source").element.compareDocumentPosition(arrow.element) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
        expect(arrow.element.compareDocumentPosition(transition.get(".screen-status-target").element) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      }
      const count = store.statusDifferences.length;
      await wrapper.findAll("button").find((button) => button.text().includes(`确认覆盖选中 ${count} 台`))!.trigger("click");
      await flushPromises();
      expect(cover).toHaveBeenCalledOnce();
      expect(cover.mock.calls[0]![1]).toEqual(before.map((screen) => expect.objectContaining({ id: screen.id, expected: screen.platformStatus, next: screen.ping, revision: screen.revision })));
      expect(wrapper.text()).toContain("本次确认结果"); expect(wrapper.text()).toContain("回读一致");
      expect(store.statusDifferences).toHaveLength(0);
      for (const screen of before) expect(store.snapshot.screens.find((item) => item.id === screen.id)?.platformStatus).toBe(screen.ping);
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("从检查结果重新检查时只预选勾选屏并返回选屏步骤，不直接执行或覆盖", async () => {
    const { wrapper, adapter, store } = await render();
    const execute = vi.spyOn(adapter, "execute"), cover = vi.spyOn(adapter, "coverStatus");
    try {
      store.selectedIds = ["platform-screen-3"]; store.operation = "ping";
      await router.push("/screen/operations"); await flushPromises();
      await wrapper.findAll("button").find((button) => button.text().includes("开始检查 1 台"))!.trigger("click");
      await vi.waitFor(() => expect(wrapper.find(".screen-final-stage").exists()).toBe(true), { timeout: 2500 });
      await vi.waitFor(() => expect(store.snapshot.tasks[0]?.state).toBe("succeeded"), { timeout: 2500 });
      await wrapper.findAll("button").find((button) => button.text().includes("查看平台状态差异"))!.trigger("click");
      await flushPromises();
      const dialog = wrapper.findComponent(ScreenStatusDialog);
      const differences = store.statusDifferences.map((screen) => screen.id);
      expect(differences.length).toBeGreaterThan(1);
      dialog.get(`[data-screen-id="${differences[0]}"]`).getComponent(NCheckbox).vm.$emit("update:checked", false);
      await flushPromises();
      await dialog.findAll("button").find((button) => button.text() === "重新检查本机状态")!.trigger("click");
      await flushPromises();
      expect(router.currentRoute.value.path).toBe("/screen/operations");
      expect(store.operation).toBe("ping"); expect(store.selectedIds).toEqual(differences.slice(1));
      expect(store.openedTaskId).toBe("");
      expect(dialog.props("show")).toBe(false);
      expect(wrapper.find(".screen-final-stage").exists()).toBe(false);
      expect(wrapper.findComponent(ScreenTargetPicker).findAll(".screen-target-row.selected")).toHaveLength(differences.length - 1);
      expect(execute).toHaveBeenCalledOnce(); expect(cover).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("平台条件变化时显示未更新原因并保留平台原值", async () => {
    const { wrapper, adapter, store } = await render();
    try {
      const first = { ...store.statusDifferences[0]! };
      await wrapper.findAll(".screen-notice").find((item) => item.text().includes("状态不一致"))!.trigger("click");
      await flushPromises();
      await adapter.setScenario(store.projectId, "status_changed"); await flushPromises();
      await wrapper.findAll("button").find((button) => button.text().includes("确认覆盖选中"))!.trigger("click");
      await flushPromises();
      const dialog = wrapper.findComponent(ScreenStatusDialog);
      expect(dialog.text()).toContain("未更新");
      expect(dialog.text()).toContain("确认后目标条件已变化，请刷新差异并重新确认");
      expect(store.snapshot.screens.find((screen) => screen.id === first.id)?.platformStatus).toBe(first.platformStatus);
    } finally { wrapper.unmount(); adapter.dispose(); }
  }, 30000);

  it("切换项目后旧状态覆盖迟到返回不覆盖新项目差异抽屉", async () => {
    const { wrapper, adapter, store, projects } = await render();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const original = adapter.coverStatus.bind(adapter);
    const cover = vi.spyOn(adapter, "coverStatus").mockImplementation(async (projectId, changes) => {
      const result = await original(projectId, changes); await gate; return result;
    });
    try {
      await wrapper.findAll(".screen-notice").find((item) => item.text().includes("状态不一致"))!.trigger("click");
      await flushPromises();
      await wrapper.findAll("button").find((button) => button.text().includes("确认覆盖选中"))!.trigger("click");
      await vi.waitFor(() => expect(cover).toHaveBeenCalledOnce());
      await projects.switchProject("project-chengdu-center"); await flushPromises();
      expect(wrapper.findComponent(ScreenStatusDialog).props("show")).toBe(false);
      await wrapper.findAll(".screen-notice").find((item) => item.text().includes("状态不一致"))!.trigger("click");
      await flushPromises();
      const count = store.statusDifferences.length;
      release(); await flushPromises();
      const dialog = wrapper.findComponent(ScreenStatusDialog);
      expect(store.projectId).toBe("project-chengdu-center");
      expect(dialog.findAll(".screen-status-row")).toHaveLength(count);
      expect(dialog.find(".screen-status-results").exists()).toBe(false);
      expect(dialog.text()).toContain(`确认覆盖选中 ${count} 台`);
      expect(wrapper.text()).not.toContain("模拟覆盖完成");
    } finally { release(); wrapper.unmount(); adapter.dispose(); }
  }, 30000);
});
