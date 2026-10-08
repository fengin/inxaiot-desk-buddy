import { defineComponent, h, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it } from "vitest";
import { NMessageProvider } from "naive-ui";
import { FixtureScreenAdapter, SCREEN_STORAGE_PREFIX } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ScreenLocalEditor from "./ScreenLocalEditor.vue";
import ScreenList from "./ScreenList.vue";

const cleanups: (() => void)[] = [];
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup(); });

async function render() {
  const project = `platform-editor-${crypto.randomUUID()}`;
  const adapter = new FixtureScreenAdapter(localStorage);
  configureScreenAdapter(adapter);
  const pinia = createPinia(), store = useSmartScreensStore(pinia);
  await store.bindProject(project);
  const screen = store.snapshot.screens.find((item) => item.source === "platform")!;
  const show = ref(true);
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
    default: () => h(ScreenLocalEditor, { show: show.value, screen, "onUpdate:show": (value: boolean) => { show.value = value; } })
  }) }), { attachTo: document.body, global: { plugins: [pinia], stubs: { teleport: true } } });
  cleanups.push(() => { wrapper.unmount(); store.stop(); adapter.dispose(); localStorage.removeItem(SCREEN_STORAGE_PREFIX + project); });
  await flushPromises();
  return { project, adapter, store, pinia, screen, wrapper, show };
}

describe("平台屏资料编辑", () => {
  it("平台MAC或草稿原值不被观测替换，采集值可明确填入草稿",async()=>{
    const {wrapper,screen,show,store,adapter,project}=await render();
    show.value=false;await flushPromises();
    const confirmed=screen.mac;screen.observedMac='26:da:35:7d:85:b5';
    await adapter.savePlatformDraft(project,screen.id,{...screenPlatformFields(screen),mac:''});await store.refresh();
    show.value=true;await flushPromises();
    expect(wrapper.get<HTMLInputElement>('input[placeholder="可留空"]').element.value).toBe('');
    expect(wrapper.get('[data-testid="screen-collected-mac-note"]').text()).toContain(screen.observedMac);
    await wrapper.get('[data-testid="screen-use-collected-mac"]').trigger('click');
    await wrapper.get('form').trigger('submit');await flushPromises();
    expect(store.snapshot.platformDrafts?.[screen.id]?.values.mac).toBe(screen.observedMac);
    expect(store.snapshot.screens.find(item=>item.id===screen.id)?.mac).toBe(confirmed);
  });
  it("编辑平台屏仅保存待提交修改，刷新与重开保留草稿且列表只显示一条", async () => {
    const { wrapper, screen, store, project, adapter, pinia, show } = await render();
    const beforeCount = store.stats.total;
    await wrapper.get('input[placeholder="例如：东区电梯出口右侧"]').setValue("新的安装位置");
    await wrapper.get("form").trigger("submit"); await flushPromises();
    expect(show.value).toBe(false);
    expect(store.snapshot.screens.find((item) => item.id === screen.id)?.location).toBe(screen.location);
    expect(store.snapshot.platformDrafts?.[screen.id]?.values.location).toBe("新的安装位置");
    expect(store.stats.total).toBe(beforeCount);
    const list = mount(ScreenList, { global: { plugins: [pinia] } });
    try {
      await list.get(`[data-testid="screen-draft-${screen.id}"]`).trigger("click");
      expect(list.emitted("register")?.[0]).toEqual([screen.id]);
      expect(list.findAll(`[data-testid="screen-draft-${screen.id}"]`)).toHaveLength(1);
    } finally { list.unmount(); }
    const reloaded = new FixtureScreenAdapter(localStorage);
    try {
      const snapshot = await reloaded.load(project);
      expect(snapshot.platformDrafts?.[screen.id]?.values.location).toBe("新的安装位置");
      expect(snapshot.screens.filter((item) => item.id === screen.id)).toHaveLength(1);
    } finally { reloaded.dispose(); }
    await adapter.setScenario(project, "platform_offline"); await store.refresh();
    expect(store.visibleScreens.filter((item) => item.id === screen.id)).toHaveLength(1);
    expect(store.visibleScreens.find((item) => item.id === screen.id)?.source).toBe("platform");
    expect(store.draftIds.has(screen.id)).toBe(true);
  });

  it("平台同字段变化时重新编辑显示当前平台值，确认后重建草稿比较依据", async () => {
    const { wrapper, screen, adapter, store, project, show } = await render();
    show.value = false; await flushPromises();
    await adapter.savePlatformDraft(project, screen.id, { ...screenPlatformFields(screen), location: "本机拟修改的位置" });
    const state = (adapter as unknown as { snapshots: Map<string, typeof store.snapshot> }).snapshots.get(project)!;
    state.screens.find((item) => item.id === screen.id)!.location = "平台其他人员修改的位置";
    state.screens.find((item) => item.id === screen.id)!.revision++;
    await store.refresh();
    Object.assign(screen, store.snapshot.screens.find((item) => item.id === screen.id));
    show.value = true; await flushPromises();
    expect(wrapper.get('[data-testid="screen-draft-conflict"]').text()).toContain("平台其他人员修改的位置");
    expect(wrapper.get<HTMLInputElement>('input[placeholder="例如：东区电梯出口右侧"]').element.value).toBe("本机拟修改的位置");
    await wrapper.get("form").trigger("submit"); await flushPromises();
    expect(store.snapshot.platformDrafts?.[screen.id]?.base.location).toBe("平台其他人员修改的位置");
    expect(store.snapshot.platformDrafts?.[screen.id]?.values.location).toBe("本机拟修改的位置");
  });

  it("注册后选择与数量转换到平台记录，刷新不会留下旧本机行", async () => {
    const { adapter, store, project } = await render();
    await adapter.saveLocal(project, { name: "注册数量验证屏", ip: "192.0.2.181", mac: "", size: "10", spaceId: "floor-a-1", location: "测试位置" });
    await store.refresh();
    const local = store.snapshot.screens.find((item) => item.ip === "192.0.2.181")!;
    const before = { ...store.stats };
    store.selectedIds = [local.id];
    const preview = await adapter.previewPlatformRegistration(project, [local.id]);
    await adapter.submitPlatformRegistration(project, { previewId: preview.id, screenIds: [local.id] });
    await store.refresh();
    const registered = store.snapshot.screens.find((item) => item.aliases.includes(local.id))!;
    expect(registered.source).toBe("platform");
    expect(store.selectedIds).toEqual([registered.id]);
    expect(store.selectedScreens).toHaveLength(1);
    expect(store.snapshot.screens.some((item) => item.id === local.id)).toBe(false);
    expect(store.stats.total).toBe(before.total);
    expect(store.stats.local).toBe(before.local - 1);
    expect(store.stats.platform).toBe(before.platform + 1);
    await store.refresh();
    expect(store.snapshot.screens.filter((item) => item.ip === local.ip)).toHaveLength(1);
  });
});
