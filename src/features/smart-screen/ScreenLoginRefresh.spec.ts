import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { expect, it, vi } from "vitest";
import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useProjectStore } from "@/stores/projects";

const loginRequest = { username: "tester", password: "fixture-only", sessionUuid: "fixture", imageCode: "1234" };
async function renderLoggedOut(path = "/screen/nodes") {
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() })) });
  const workbench = new FixtureWorkbenchAdapter();
  await workbench.logoutProject("project-shenzhen-bay");
  const adapter = new FixtureScreenAdapter(localStorage, 20);
  Object.defineProperty(adapter, "mode", { value: "real" });
  const originalLoad = adapter.load.bind(adapter);
  const readSnapshot = async (id: string) => {
    const snapshot = await originalLoad(id), session = await workbench.getProjectSession(id);
    return { ...snapshot, platformAvailable: session.state === "active", platformMessage: session.state === "active" ? "" : "平台认证失败：平台会话缺失或已过期" };
  };
  const load = vi.spyOn(adapter, "load").mockImplementation(readSnapshot);
  configureScreenAdapter(adapter); configureWorkbenchAdapter(workbench); configureActivityAdapter(new FixtureActivityAdapter(adapter));
  const { default: App } = await import("@/app/App.vue");
  await router.push(path); await router.isReady();
  const pinia = createPinia();
  const wrapper = mount(App, { attachTo: document.body, global: { plugins: [pinia, router, i18n], stubs: { teleport: true } } });
  await flushPromises();
  return { wrapper, adapter, workbench, load, readSnapshot, store: useSmartScreensStore(pinia), projects: useProjectStore(pinia) };
}

it.each(["/screen/nodes", "/screen/operations"])("%s 同项目登录自动读取平台、消除旧提示并保留当前页面状态", async (path) => {
  const { wrapper, adapter, load, store, projects } = await renderLoggedOut(path);
  const bind = vi.spyOn(store, "bindProject");
  try {
    expect(wrapper.get(".screen-workspace-alert").text()).toContain("平台认证失败");
    expect(wrapper.get(".screen-workspace-alert").text()).toContain("平台不可用");
    const selectedId = store.snapshot.screens[0]!.id;
    store.selectedIds = [selectedId]; store.pageSize = 5; store.page = 2; store.filters.size = "10"; store.openedTaskId = "retained-task";
    const before = load.mock.calls.length;
    await projects.login(projects.activeProjectId, loginRequest); await flushPromises();
    expect(load.mock.calls.length).toBe(before + 1);
    expect(load).toHaveBeenLastCalledWith(projects.activeProjectId, { refreshPlatform: true });
    expect(store.platformAvailable).toBe(true);
    expect(wrapper.find(".screen-workspace-alert").exists()).toBe(false);
    expect(store.selectedIds).toEqual([selectedId]); expect(store.filters.size).toBe("10");
    expect(store.page).toBe(2); expect(store.openedTaskId).toBe("retained-task");
    expect(router.currentRoute.value.path).toBe(path); expect(bind).not.toHaveBeenCalled();
    const after = load.mock.calls.length;
    await projects.checkSession(); await flushPromises();
    expect(load.mock.calls.length).toBe(after);
    await projects.logout(); await flushPromises();
    expect(store.platformAvailable).toBe(false);
    expect(wrapper.get(".screen-workspace-alert").text()).toContain("平台认证失败");
    expect(store.snapshot.screens.length).toBeGreaterThan(0);
  } finally { wrapper.unmount(); adapter.dispose(); }
}, 30000);

it("登录成功但共享连接失败，仍自动刷新可独立访问的平台数据", async () => {
  const { wrapper, adapter, workbench, store, projects } = await renderLoggedOut();
  const connection = vi.spyOn(workbench, "switchProject").mockRejectedValue(new Error("共享连接失败"));
  try {
    await projects.login(projects.activeProjectId, loginRequest); await flushPromises();
    expect(projects.isReady).toBe(false); expect(projects.allowsAccess("platform")).toBe(true);
    expect(store.platformAvailable).toBe(true); expect(wrapper.find(".screen-workspace-alert").exists()).toBe(false);
  } finally { connection.mockRestore(); wrapper.unmount(); adapter.dispose(); }
}, 30000);

it("登录失败保留实际认证提示，不把平台显示为可用", async () => {
  const { wrapper, adapter, workbench, load, store, projects } = await renderLoggedOut();
  const login = vi.spyOn(workbench, "loginProject").mockRejectedValue(new Error("登录失败"));
  try {
    const before = load.mock.calls.length;
    await expect(projects.login(projects.activeProjectId, loginRequest)).rejects.toThrow("登录失败"); await flushPromises();
    expect(load.mock.calls.length).toBe(before); expect(store.platformAvailable).toBe(false);
    expect(wrapper.get(".screen-workspace-alert").text()).toContain("平台认证失败");
  } finally { login.mockRestore(); wrapper.unmount(); adapter.dispose(); }
}, 30000);

it("登录前的旧读取迟到不会把认证失败提示重新覆盖回来", async () => {
  const { wrapper, adapter, load, readSnapshot, store, projects } = await renderLoggedOut();
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  try {
    const oldSnapshot = await readSnapshot(projects.activeProjectId);
    load.mockImplementationOnce(async () => { await gate; return oldSnapshot; });
    const previous = store.refresh(); await flushPromises();
    await projects.login(projects.activeProjectId, loginRequest); await flushPromises();
    expect(store.platformAvailable).toBe(true);
    release(); await previous; await flushPromises();
    expect(store.platformAvailable).toBe(true); expect(wrapper.find(".screen-workspace-alert").exists()).toBe(false);
  } finally { release(); wrapper.unmount(); adapter.dispose(); }
}, 30000);
