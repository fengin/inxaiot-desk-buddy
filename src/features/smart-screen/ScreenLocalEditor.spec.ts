import { defineComponent, h, reactive, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NMessageProvider } from "naive-ui";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";
import type { SmartScreen } from "@/shared/model/screen";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";
import ScreenLocalEditor from "./ScreenLocalEditor.vue";

const spaces: ProjectSpaceNode[] = [
  { id: 'building', name: 'A座', kind: 'building' },
  { id: 'floor', name: '1F', kind: 'floor', parentId: 'building' },
  { id: 'area', name: '东区', kind: 'area', parentId: 'floor' },
  { id: 'deep', name: '接待入口', kind: 'other', parentId: 'area' }
];
const cleanups: (() => void)[] = [];
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup(); });

async function render(available = true, spaceId = 'deep', overrides: Partial<SmartScreen> = {}) {
  const adapter = new FixtureScreenAdapter(localStorage);
  configureScreenAdapter(adapter);
  const save = vi.spyOn(adapter, 'saveLocal').mockResolvedValue(undefined);
  const pinia = createPinia();
  const store = useSmartScreensStore(pinia);
  store.projectId = 'space-editor-test';
  store.snapshot = { ...createScreenSnapshot(), spaces: [...spaces], spacesAvailable: available };
  const screen = reactive({ ...store.snapshot.screens.find((screen) => screen.source === 'local')!, spaceId, spacePath: 'A座 / 1F / 东区 / 接待入口', location: '墙面右侧', ...overrides });
  const show = ref(true);
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
    default: () => h(ScreenLocalEditor, { show: show.value, screen, 'onUpdate:show': (value: boolean) => { show.value = value; } })
  }) }), { attachTo: document.body, global: { plugins: [pinia], stubs: { teleport: true } } });
  cleanups.push(() => { wrapper.unmount(); store.stop(); adapter.dispose(); });
  await flushPromises();
  const choose = async (id: string | null) => { wrapper.getComponent(ProjectSpaceSelect).vm.$emit('update:modelValue', id); await flushPromises(); };
  const submit = async () => { await wrapper.get('form').trigger('submit'); await flushPromises(); };
  return { wrapper, store, save, choose, submit, screen };
}

describe("智能屏编辑使用已采集MAC", () => {
  const captured = '26:da:35:7d:85:b5';
  it("本机未填写MAC时带入采集值，打开不写资料，确认保存才提交", async () => {
    const {wrapper,screen,save,submit}=await render(true,'deep',{mac:'',observedMac:captured});
    expect(wrapper.get<HTMLInputElement>('input[placeholder="可留空"]').element.value).toBe(captured);
    expect(wrapper.get('[data-testid="screen-collected-mac-note"]').text()).toContain('保存后记入本机资料');
    expect(screen.mac).toBe('');expect(save).not.toHaveBeenCalled();
    await submit();expect(save.mock.calls[0]![1].mac).toBe(captured);
  });
  it("已有资料MAC不被采集值覆盖，可以明确采用新的采集值",async()=>{
    const original='02:11:22:33:44:55';const {wrapper,save,submit}=await render(true,'deep',{mac:original,observedMac:captured});
    expect(wrapper.get<HTMLInputElement>('input[placeholder="可留空"]').element.value).toBe(original);
    await wrapper.get('[data-testid="screen-use-collected-mac"]').trigger('click');await submit();
    expect(save.mock.calls[0]![1].mac).toBe(captured);
  });
  it("IP变化后不继续默认使用原地址的采集MAC",async()=>{
    const {wrapper,save,submit}=await render(true,'deep',{mac:'',observedMac:captured});
    await wrapper.get('input[placeholder="例如：192.0.2.120"]').setValue('192.0.2.182');await flushPromises();
    expect(wrapper.get<HTMLInputElement>('input[placeholder="可留空"]').element.value).toBe('');
    expect(wrapper.find('[data-testid="screen-use-collected-mac"]').exists()).toBe(false);
    await submit();expect(save.mock.calls[0]![1]).toMatchObject({ip:'192.0.2.182',mac:''});
  });
  it("手工输入或主动清空后，新的采集结果不改动正在编辑的MAC",async()=>{
    const {wrapper,screen,save,submit}=await render(true,'deep',{mac:'',observedMac:captured});
    await wrapper.get('input[placeholder="可留空"]').setValue('');screen.observedMac='26:da:35:7d:85:b6';await flushPromises();
    expect(wrapper.get<HTMLInputElement>('input[placeholder="可留空"]').element.value).toBe('');
    await submit();expect(save.mock.calls[0]![1].mac).toBe('');
  });
  it.each(['00:00:00:00:00:00','ff:ff:ff:ff:ff:ff','invalid'])('不带入无效采集值：%s',async observedMac=>{
    const {wrapper}=await render(true,'deep',{mac:'',observedMac});
    expect(wrapper.get<HTMLInputElement>('input[placeholder="可留空"]').element.value).toBe('');
    expect(wrapper.find('[data-testid="screen-collected-mac-note"]').exists()).toBe(false);
  });
});

describe("智能屏空间编辑", () => {
  it("允许直属楼幢，提交单个空间ID和完整路径，安装位置独立", async () => {
    const { save, choose, submit } = await render();
    await choose('building'); await submit();
    expect(save).toHaveBeenCalledOnce();
    expect(save.mock.calls[0]![1]).toMatchObject({ spaceId: 'building', spacePath: 'A座', location: '墙面右侧' });
    expect(save.mock.calls[0]![1]).not.toHaveProperty('buildingId');
    expect(save.mock.calls[0]![1]).not.toHaveProperty('floorId');
  });

  it("目录暂不可用时保留原ID及缓存路径，仍可编辑其他字段", async () => {
    const { wrapper, save, submit } = await render(false);
    expect(wrapper.getComponent(ProjectSpaceSelect).props('disabled')).toBe(true);
    expect(wrapper.text()).toContain('待核验');
    await wrapper.get('input[placeholder="例如：东区电梯出口右侧"]').setValue('更新后的墙面位置');
    await submit();
    expect(save.mock.calls[0]![1]).toMatchObject({ spaceId: 'deep', spacePath: 'A座 / 1F / 东区 / 接待入口', location: '更新后的墙面位置' });
  });

  it("目录暂不可用时只有主动清空才移除旧关联", async () => {
    const { wrapper, save, submit } = await render(false);
    await wrapper.get('[data-testid="screen-clear-space"]').trigger('click'); await submit();
    expect(save.mock.calls[0]![1]).toMatchObject({ spaceId: null, spacePath: '' });
  });

  it("目录可用但旧ID失效时拒绝保存，重新选择有效节点后才提交", async () => {
    const { wrapper, save, choose, submit } = await render(true, 'deleted-space');
    await submit();
    expect(save).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain('原空间关联已失效');
    await choose('floor'); await submit();
    expect(save.mock.calls[0]![1]).toMatchObject({ spaceId: 'floor', spacePath: 'A座/1F' });
  });

  it("新选择后目录断开不能分配新ID，可显式恢复原关联再保存", async () => {
    const { wrapper, store, save, choose, submit } = await render();
    await choose('floor'); store.snapshot.spacesAvailable = false; await flushPromises();
    await submit();
    expect(save).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain('不能分配新的空间');
    await wrapper.get('[data-testid="screen-restore-space"]').trigger('click'); await submit();
    expect(save.mock.calls[0]![1]).toMatchObject({ spaceId: 'deep', spacePath: 'A座 / 1F / 东区 / 接待入口' });
  });
});
