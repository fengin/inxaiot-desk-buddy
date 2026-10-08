import { defineComponent, h, ref } from "vue";
import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NMessageProvider } from "naive-ui";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import type { ScreenVersionPreview } from "@/shared/model/screenMaintenance";
import ScreenVersionSyncDialog from "./ScreenVersionSyncDialog.vue";
import ScreenOperations from "./ScreenOperations.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";

const cleanups: (() => void)[] = [];
afterEach(() => { cleanups.splice(0).forEach((cleanup) => cleanup()); });
async function setup() {
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 10);
  configureScreenAdapter(adapter);
  const store = useSmartScreensStore(pinia), project = `maintenance-ui-${crypto.randomUUID()}`;
  await store.bindProject(project);
  cleanups.push(() => { store.stop(); adapter.dispose(); });
  const state = (adapter as unknown as { snapshots: Map<string, typeof store.snapshot> }).snapshots.get(project)!;
  state.screens.find((s) => s.id === "platform-screen-2")!.appVersion = "0.9.0";
  await store.refresh();
  return { pinia, adapter, store, project };
}
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }

describe("维护补齐交互", () => {
  it("打开版本核对后展示实际IP、版本差异，确认仅同步版本并显示结果", async () => {
    const { pinia, store } = await setup();
    const wrapper = mount(ScreenVersionSyncDialog, { props: { show: true, ids: ["platform-screen-2"] }, global: { plugins: [pinia], stubs: { teleport: true } } });
    cleanups.unshift(() => wrapper.unmount()); await flushPromises();
    const row = wrapper.get('[data-version-screen="platform-screen-2"]');
    expect(row.text()).toContain("192.0.2.32"); expect(row.text()).toContain("0.9.0"); expect(row.text()).toContain("1.5.2");
    await wrapper.get('[data-testid="screen-version-submit"]').trigger("click"); await flushPromises();
    expect(wrapper.text()).toContain("没有执行安装");
    expect(store.snapshot.tasks[0]?.action).toBe("version_sync");
    expect(store.snapshot.screens.find((s) => s.id === "platform-screen-2")?.appVersion).toBe("1.5.2");
  });

  it("项目切换后迟到的版本预览不进入新项目", async () => {
    const { pinia, store, adapter, project } = await setup();
    const pending = deferred<ScreenVersionPreview>();
    vi.spyOn(adapter, "previewVersionSync").mockReturnValueOnce(pending.promise);
    const wrapper = mount(ScreenVersionSyncDialog, { props: { show: true, ids: ["platform-screen-2"] }, global: { plugins: [pinia], stubs: { teleport: true } } });
    cleanups.unshift(() => wrapper.unmount()); await flushPromises();
    store.projectId = "new-project"; await flushPromises();
    pending.resolve({ id: "late", projectId: project, createdAt: "2026-09-28T00:00:00Z", items: [{ screenId: "platform-screen-2", name: "旧项目迟到数据", ip: "192.0.2.32", platformVersion: null, deviceVersion: "1.5.2", checkedAt: "2026-09-28T00:00:00Z", state: "ready", reason: "通过" }] });
    await flushPromises();
    expect(wrapper.text()).not.toContain("旧项目迟到数据");
    expect(wrapper.emitted("update:show")?.at(-1)).toEqual([false]);
  });

  it("同步中重复点击不重提，平台离线时仍显示读取结果但禁止同步", async () => {
    const { pinia, adapter, store, project } = await setup();
    const pending = deferred<string>(), submit = vi.spyOn(adapter, "submitVersionSync").mockReturnValueOnce(pending.promise);
    const wrapper = mount(ScreenVersionSyncDialog, { props: { show: true, ids: ["platform-screen-2"] }, global: { plugins: [pinia], stubs: { teleport: true } } });
    cleanups.unshift(() => wrapper.unmount()); await flushPromises();
    await wrapper.get('[data-testid="screen-version-submit"]').trigger("click");
    await wrapper.get('[data-testid="screen-version-submit"]').trigger("click");
    expect(submit).toHaveBeenCalledOnce(); pending.resolve("simulated-task"); await flushPromises();
    await adapter.setScenario(project, "platform_offline"); await store.refresh();
    const read = wrapper.findAll("button").find((button) => button.text() === "重新读取版本")!;
    await read.trigger("click"); await flushPromises();
    expect(wrapper.text()).toContain("版本已读取并保留本机");
    expect(wrapper.get('[data-testid="screen-version-submit"]').attributes("disabled")).toBeDefined();
  });

  it("关键草稿显示确认与拟修改地址，可引导更新；只读检查明确仍用确认地址", async () => {
    const { pinia, adapter, store, project } = await setup();
    const current = store.snapshot.screens.find((s) => s.id === "platform-screen-2")!;
    await adapter.savePlatformDraft(project, current.id, { ...screenPlatformFields(current), ip: "192.0.2.220" }); await store.refresh();
    store.selectedIds = [current.id]; store.operation = "reboot";
    const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, { default: () => h(ScreenOperations) }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
    cleanups.unshift(() => wrapper.unmount()); await flushPromises();
    await wrapper.get('[data-testid="screen-critical-drafts"] button').trigger("click"); await flushPromises();
    expect(wrapper.get('[data-testid="screen-critical-draft-list"]').text()).toContain("192.0.2.32");
    expect(wrapper.get('[data-testid="screen-critical-draft-list"]').text()).toContain("192.0.2.220");
    store.operation = "ping"; await flushPromises();
    expect(wrapper.get('[data-testid="screen-critical-drafts"]').text()).toContain("只读检查仍连接下列确认地址");
    await wrapper.findAll("button").find((button) => button.text() === "前往注册/更新到平台")!.trigger("click");
    expect(store.operation).toBe("register");
  });

  it("注册结果提供独立版本核对并携带旧本机目标供关联解析", async () => {
    const { pinia, store, project } = await setup();
    const ids = ref<string[]>([]);
    const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, { default: () => h(ScreenTaskResults, { task: { id: "registration-result", projectId: project, action: "register", state: "succeeded", createdAt: "2026-09-28T00:00:00Z", updatedAt: "2026-09-28T00:00:00Z", targets: [{ screenId: "old-local", name: "已登记屏", ip: "192.0.2.180", state: "succeeded", progress: 100, message: "登记完成" }], logs: [] }, onVersions: (value: string[]) => { ids.value = value; } }) }) }), { global: { plugins: [pinia], stubs: { teleport: true } } });
    cleanups.unshift(() => wrapper.unmount()); await flushPromises();
    await wrapper.findAll("button").find((button) => button.text() === "核对版本并同步平台")!.trigger("click");
    expect(ids.value).toEqual(["old-local"]); expect(store.snapshot.tasks).toHaveLength(0);
  });
});
