import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NPopconfirm } from "naive-ui";
import { FixtureScreenAdapter, SCREEN_STORAGE_PREFIX } from "@/dev-fixtures/screenFixtureAdapter";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { buildScreenMergeFields, createScreenMergeChoices } from "@/shared/model/screen";
import type { ScreenMergeField, ScreenMergeResult, ScreenMergeSource, ScreenSnapshot } from "@/shared/model/screen";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ScreenMergeDialog from "./ScreenMergeDialog.vue";

const cleanups: (() => void)[] = [];
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup(); });

async function render(snapshot: ScreenSnapshot = createScreenSnapshot()) {
  const project = "merge-dialog-project";
  localStorage.setItem(SCREEN_STORAGE_PREFIX + project, JSON.stringify(snapshot));
  const adapter = new FixtureScreenAdapter(localStorage, 20);
  configureScreenAdapter(adapter);
  const pinia = createPinia();
  const store = useSmartScreensStore(pinia);
  await store.bindProject(project);
  const wrapper = mount(ScreenMergeDialog, { props: { show: true }, attachTo: document.body, global: { plugins: [pinia], stubs: { teleport: true } } });
  cleanups.push(() => { wrapper.unmount(); store.stop(); adapter.dispose(); });
  await flushPromises();
  const choose = async (field: ScreenMergeField, source: ScreenMergeSource) => {
    await wrapper.get(`input[type="radio"][name="merge-${field}"][value="${source}"]`).setValue(true);
    await flushPromises();
  };
  const submitDisabled = () => wrapper.get('[data-testid="merge-submit"]').attributes('disabled') !== undefined;
  return { wrapper, adapter, store, project, choose, submitDisabled };
}

describe("智能屏逐字段合并", () => {
  it("每行来源互斥，全选只改当前候选字段，混合来源合并后保留结果", async () => {
    const { wrapper, adapter, store, project, choose } = await render();
    const candidate = store.candidates[0]!;
    const merge = vi.spyOn(adapter, "merge");
    expect(wrapper.findAll('input[type="radio"]')).toHaveLength(14);
    expect(wrapper.findAll('thead [role="checkbox"]')).toHaveLength(2);
    expect(wrapper.get('[data-testid="merge-selection-summary"]').text()).toContain("平台来源 7 项");

    await choose("name", "local");
    const nameInputs = wrapper.findAll<HTMLInputElement>('input[name="merge-name"]');
    expect(nameInputs.map((input) => input.element.checked)).toEqual([true, false]);
    expect(wrapper.get('[data-merge-field="name"] .merge-value-result').text()).toContain(candidate.local.name);
    expect(wrapper.get('[data-merge-field="name"] .merge-value-result').classes()).toContain('merge-result-local');
    expect(wrapper.find('[data-merge-field="name"] .merge-value-result small').exists()).toBe(false);
    expect(wrapper.get('[data-testid="merge-all-local"]').attributes('aria-checked')).toBe('mixed');
    expect(wrapper.get('[data-testid="merge-all-platform"]').attributes('aria-checked')).toBe('mixed');
    await wrapper.get('[data-testid="merge-all-local"]').trigger('click');
    expect(wrapper.findAll<HTMLInputElement>('input[value="local"]').every((input) => input.element.checked)).toBe(true);
    expect(wrapper.get('[data-testid="merge-all-local"]').attributes('aria-checked')).toBe('true');
    await wrapper.get('[data-testid="merge-all-local"]').trigger('click');
    expect(wrapper.findAll<HTMLInputElement>('input[type="radio"]:checked')).toHaveLength(7);
    expect(wrapper.findAll<HTMLInputElement>('input[value="local"]').every((input) => input.element.checked)).toBe(true);
    expect(merge).not.toHaveBeenCalled();
    await wrapper.get('[data-testid="merge-all-platform"]').trigger('click');
    expect(wrapper.findAll<HTMLInputElement>('input[value="platform"]').every((input) => input.element.checked)).toBe(true);

    await choose("name", "local"); await choose("mac", "local"); await choose("appVersion", "local");
    await wrapper.get('[data-testid="merge-submit"]').trigger('click'); await flushPromises();
    expect(merge).toHaveBeenCalledOnce();
    expect(merge.mock.calls[0]![2]).toEqual({ kind: "merge", choices: { ...createScreenMergeChoices(), name: "local", mac: "local", appVersion: "local" }, identityConfirmed: false });
    expect(wrapper.get('[data-testid="merge-result"]').text()).toContain("合并成功");
    expect(wrapper.get('[data-merge-field="name"] .merge-value-result').text()).toContain(candidate.local.name);
    const saved = (await adapter.load(project)).screens;
    expect(saved.find((screen) => screen.id === candidate.platform.id)?.name).toBe(candidate.local.name);
    expect(saved.some((screen) => screen.id === candidate.local.id)).toBe(false);
  });

  it("MAC 冲突需要人工核实同一设备，切换候选清除该确认", async () => {
    const { wrapper, adapter, store, submitDisabled } = await render();
    const conflict = store.candidates.find((candidate) => candidate.conflict)!;
    const selectConflict = () => wrapper.findAll('.merge-candidates button').find((button) => button.attributes('data-candidate-key') === conflict.key)!;
    await selectConflict().trigger('click');
    expect(submitDisabled()).toBe(true);
    await wrapper.get('[data-testid="merge-confirm-identity"]').trigger('click');
    expect(submitDisabled()).toBe(false);
    await wrapper.findAll('.merge-candidates button')[0]!.trigger('click');
    await selectConflict().trigger('click');
    expect(submitDisabled()).toBe(true);
    await wrapper.get('[data-testid="merge-confirm-identity"]').trigger('click');
    const merge = vi.spyOn(adapter, 'merge');
    await wrapper.get('[data-testid="merge-submit"]').trigger('click'); await flushPromises();
    expect(merge.mock.calls[0]![2]).toMatchObject({ kind: "merge", identityConfirmed: true });
    expect(wrapper.get('[data-testid="merge-result"]').text()).toContain("合并成功");
  });

  it("空间作为整体选源，保留所选节点和完整路径", async () => {
    const snapshot = createScreenSnapshot();
    const local = snapshot.screens.find((screen) => screen.id === 'local-screen-1')!;
    Object.assign(local, { spaceId: 'area-b-2-room', spacePath: 'B座 / 2F / 东区 / 会议区 / 会议室' });
    const { wrapper, adapter, project, choose, submitDisabled } = await render(snapshot);
    expect(wrapper.find('[data-merge-field="building"]').exists()).toBe(false);
    expect(wrapper.find('[data-merge-field="floor"]').exists()).toBe(false);
    await choose('space', 'local');
    expect(wrapper.get('[data-merge-field="space"] .merge-value-result').text()).toBe('B座/2F/东区/会议区/会议室');
    expect(wrapper.find('[data-testid="merge-validation"]').exists()).toBe(false);
    expect(submitDisabled()).toBe(false);
    await wrapper.get('[data-testid="merge-submit"]').trigger('click'); await flushPromises();
    expect((await adapter.load(project)).screens.find((screen) => screen.id === 'platform-screen-1')).toMatchObject({ spaceId: 'area-b-2-room', buildingId: 'building-b', floorId: 'floor-b-2' });
  });

  it("源记录或空间目录变化后禁止旧预览提交，重新核对清除旧选择", async () => {
    const { wrapper, store, choose, submitDisabled } = await render();
    await choose('name', 'local');
    store.snapshot.screens.find((screen) => screen.id === 'local-screen-1')!.name = '更新后的屏';
    await flushPromises();
    expect(wrapper.find('[data-testid="merge-stale"]').exists()).toBe(true);
    expect(submitDisabled()).toBe(true);
    expect(wrapper.findAll<HTMLInputElement>('input[type="radio"]').every((input) => input.element.disabled)).toBe(true);
    await wrapper.get('[data-testid="merge-review-again"]').trigger('click');
    expect(wrapper.find('[data-testid="merge-stale"]').exists()).toBe(false);
    expect(wrapper.get('[data-testid="merge-selection-summary"]').text()).toContain('平台来源 7 项');
    store.snapshot.spaces[0]!.name = '更新后的楼幢'; await flushPromises();
    expect(wrapper.find('[data-testid="merge-stale"]').exists()).toBe(true);
    expect(submitDisabled()).toBe(true);
  });

  it("提交中禁止重复调用和修改，失败原因保留在抽屉供重试", async () => {
    const { wrapper, adapter } = await render();
    let reject!: (error: Error) => void;
    const pending = new Promise<ScreenMergeResult | undefined>((_resolve, rejectPromise) => { reject = rejectPromise; });
    const merge = vi.spyOn(adapter, 'merge').mockReturnValue(pending);
    await wrapper.get('[data-testid="merge-submit"]').trigger('click');
    await wrapper.get('[data-testid="merge-submit"]').trigger('click');
    expect(merge).toHaveBeenCalledOnce();
    expect(wrapper.findAll<HTMLInputElement>('input[type="radio"]').every((input) => input.element.disabled)).toBe(true);
    expect(wrapper.get('[data-testid="merge-all-local"]').classes()).toContain('n-checkbox--disabled');
    reject(new Error('平台写入失败，请重新核对')); await flushPromises();
    expect(wrapper.get('[data-testid="merge-error"]').text()).toContain('平台写入失败');
    expect(wrapper.get('[data-testid="merge-submit"]').attributes('disabled')).toBeUndefined();
    expect(wrapper.find('[data-testid="merge-result"]').exists()).toBe(false);
  });

  it.each(["success", "failure"] as const)("项目切换清空选择并忽略旧项目迟到的 %s", async (outcome) => {
    const { wrapper, adapter, store, choose } = await render();
    const candidate = store.candidates[0]!;
    let finish!: (result: ScreenMergeResult | undefined) => void;
    let reject!: (error: Error) => void;
    vi.spyOn(adapter, 'merge').mockReturnValue(new Promise((resolve, rejectPromise) => { finish = resolve; reject = rejectPromise; }));
    await choose('name', 'local');
    await wrapper.get('[data-testid="merge-submit"]').trigger('click');
    await store.bindProject('merge-another-project'); await flushPromises();
    expect(wrapper.get('[data-testid="merge-selection-summary"]').text()).toContain('平台来源 7 项');
    if (outcome === 'success') finish({ localId: candidate.local.id, platformId: candidate.platform.id, fields: buildScreenMergeFields(candidate, createScreenMergeChoices()) });
    else reject(new Error('旧项目写入失败'));
    await flushPromises();
    expect(wrapper.find('[data-testid="merge-result"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="merge-error"]').exists()).toBe(false);
    expect(wrapper.get('[data-testid="merge-submit"]').attributes('disabled')).toBeUndefined();
  });

  it("活动任务阻断合并，平台不可用显示原因并锁定操作", async () => {
    const { wrapper, store, submitDisabled } = await render();
    const candidate = store.candidates[0]!;
    store.snapshot.tasks.push({ id: 'active-task', projectId: store.projectId, action: 'ping', state: 'running', createdAt: '', updatedAt: '', logs: [], targets: [{ screenId: candidate.local.id, name: candidate.local.name, ip: candidate.local.ip, state: 'running', progress: 0, message: '' }] });
    await flushPromises();
    expect(wrapper.find('[data-testid="merge-busy"]').exists()).toBe(true);
    expect(submitDisabled()).toBe(true);
    store.snapshot.tasks = []; store.snapshot.platformAvailable = false; await flushPromises();
    expect(wrapper.get('[data-testid="merge-platform-unavailable"]').text()).toContain('平台不可用');
    expect(submitDisabled()).toBe(true);
    expect(wrapper.get('[data-action-owner="different-screen-confirm"]').attributes('disabled')).toBeDefined();
  });

  it("确认不是同一台后保留独立记录，结果在抽屉可见", async () => {
    const { wrapper, adapter, store, project } = await render();
    const before = store.snapshot.screens.length;
    const merge = vi.spyOn(adapter, 'merge');
    wrapper.getComponent(NPopconfirm).vm.$emit('positive-click'); await flushPromises();
    expect(merge.mock.calls[0]![2]).toEqual({ kind: 'ignore' });
    expect((await adapter.load(project)).screens).toHaveLength(before);
    expect(wrapper.get('[data-testid="merge-result"]').text()).toContain('已保留为不同屏');
  });

  it("已合并别名上的待核实任务也阻断当前候选", async () => {
    const snapshot = createScreenSnapshot();
    snapshot.screens.find((screen) => screen.id === 'platform-screen-1')!.aliases.push('previous-local-screen');
    snapshot.tasks.push({ id: 'review-task', projectId: 'merge-dialog-project', action: 'ping', state: 'needs_review', createdAt: '', updatedAt: '', logs: [], targets: [{ screenId: 'previous-local-screen', name: '原本机屏', ip: '192.0.2.31', state: 'needs_review', progress: 100, message: '' }] });
    const { wrapper, submitDisabled } = await render(snapshot);
    expect(wrapper.find('[data-testid="merge-busy"]').exists()).toBe(true);
    expect(submitDisabled()).toBe(true);
  });

  it("组件卸载后旧成功回调不再触发项目刷新", async () => {
    const { wrapper, adapter, store } = await render();
    const candidate = store.candidates[0]!;
    let finish!: (result: ScreenMergeResult | undefined) => void;
    vi.spyOn(adapter, 'merge').mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const refresh = vi.spyOn(store, 'refresh');
    await wrapper.get('[data-testid="merge-submit"]').trigger('click');
    wrapper.unmount();
    finish({ localId: candidate.local.id, platformId: candidate.platform.id, fields: buildScreenMergeFields(candidate, createScreenMergeChoices()) });
    await flushPromises();
    expect(refresh).not.toHaveBeenCalled();
  });
});
