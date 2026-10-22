import { createPinia } from "pinia";
import { defineComponent, h } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DesktopShell from "./DesktopShell.vue";
import { usePreferencesStore } from "@/stores/preferences";
import { useProjectStore } from "@/stores/projects";
import { useDataDirectoryStore } from "@/stores/dataDirectory";
import type { ProjectOverview } from "@/shared/model/project";

const lifecycle = vi.hoisted(() => ({ listen: vi.fn(), unlisten: vi.fn(), confirm: vi.fn() }));
vi.mock("@/shared/api/applicationLifecycle", () => ({
  listenApplicationExitImpact: lifecycle.listen,
  confirmApplicationExit: lifecycle.confirm,
}));

const preferenceKey = "inxaiot-desk-buddy.preferences.v1";
const navigationQuery = "(max-width: 1080px)";
const originalMatchMedia = Object.getOwnPropertyDescriptor(window, "matchMedia");
const originalInnerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth");
const cleanups: (() => void)[] = [];

beforeEach(() => {
  localStorage.clear();
  vi.clearAllMocks();
  lifecycle.listen.mockResolvedValue(lifecycle.unlisten);
});
afterEach(() => {
  for (const cleanup of cleanups.splice(0).reverse()) cleanup();
  vi.restoreAllMocks();
  if (originalMatchMedia) Object.defineProperty(window, "matchMedia", originalMatchMedia);
  if (originalInnerWidth) Object.defineProperty(window, "innerWidth", originalInnerWidth);
});

function viewport(initialWidth: number) {
  let width = initialWidth;
  const listeners = new Set<EventListenerOrEventListenerObject>();
  const target = new EventTarget();
  const add = vi.fn((type: string, listener: EventListenerOrEventListenerObject) => {
    if (type === "change") listeners.add(listener);
    target.addEventListener(type, listener);
  });
  const remove = vi.fn((type: string, listener: EventListenerOrEventListenerObject) => {
    if (type === "change") listeners.delete(listener);
    target.removeEventListener(type, listener);
  });
  const media = {
    media: navigationQuery, get matches() { return width <= 1080; }, onchange: null,
    addEventListener: add, removeEventListener: remove,
    dispatchEvent: (event: Event) => target.dispatchEvent(event),
  } as unknown as MediaQueryList;
  const matchMedia = vi.fn((query: string) => {
    if (query === navigationQuery) return media;
    return { matches: false, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() } as unknown as MediaQueryList;
  });
  Object.defineProperty(window, "matchMedia", { configurable: true, value: matchMedia });
  Object.defineProperty(window, "innerWidth", { configurable: true, get: () => width });
  return {
    media, add, remove, matchMedia, listenerCount: () => listeners.size,
    async resize(nextWidth: number) {
      const previousMatch = media.matches;
      width = nextWidth;
      window.dispatchEvent(new Event("resize"));
      if (previousMatch !== media.matches) target.dispatchEvent(Object.assign(new Event("change"), { matches: media.matches, media: navigationQuery }));
      await flushPromises();
    },
    async repeatChange() {
      target.dispatchEvent(Object.assign(new Event("change"), { matches: media.matches, media: navigationQuery }));
      await flushPromises();
    },
  };
}

function sampleProject(): ProjectOverview {
  return {
    id: "navigation-project", name: "导航回归项目", platformUrl: "https://platform.example.invalid",
    dbHost: "192.0.2.10", dbPort: 3306, dbUser: "fixture", dbTlsEnabled: false,
    businessDb: "fixture_business", workbenchDb: "fixture_workbench", databaseState: "connected",
    connectionState: "login_required", connectionEncrypted: false, statusMessage: "",
    session: { localProjectId: "navigation-project", state: "missing" },
  };
}

async function render(options: { width?: number; preferredCollapsed?: boolean; pendingInitialize?: Promise<void> } = {}) {
  const screen = viewport(options.width ?? 1280);
  const pinia = createPinia();
  const preferences = usePreferencesStore(pinia);
  preferences.apply({ ...preferences.snapshot, navigationCollapsed: options.preferredCollapsed ?? false });
  const initialStored = localStorage.getItem(preferenceKey);
  const persist = vi.spyOn(localStorage, "setItem");
  const projects = useProjectStore(pinia);
  projects.projects = [sampleProject()];
  projects.activeProjectId = projects.projects[0]!.id;
  vi.spyOn(projects, "initialize").mockImplementation(async () => { await options.pendingInitialize; });
  vi.spyOn(projects, "checkSession").mockImplementation(async () => projects.session);
  const dataDirectory = useDataDirectoryStore(pinia);
  vi.spyOn(dataDirectory, "initialize").mockResolvedValue(undefined);
  const page = (label: string) => defineComponent({ setup: () => () => h("div", { "data-testid": "route-page" }, label) });
  const router = createRouter({ history: createMemoryHistory(), routes: [
    { path: "/screen/nodes", component: page("智能屏列表页"), meta: { projectAccess: "local" } },
    { path: "/screen/operations", component: page("智能屏操作页"), meta: { projectAccess: "local" } },
    { path: "/aio/nodes", component: page("一体机列表页"), meta: { projectAccess: "platform" } },
    { path: "/aio/release", component: page("发布参数页"), meta: { projectAccess: "shared" } },
    { path: "/aio/operations", component: page("部署升级页"), meta: { projectAccess: "shared" } },
  ] });
  await router.push("/screen/nodes"); await router.isReady();
  const wrapper = mount(DesktopShell, {
    attachTo: document.body,
    global: { plugins: [pinia, router], stubs: { ProjectSwitcher: true, ActivityPanel: true, PreferencesDialog: true, teleport: true } },
  });
  let disposed = false;
  const dispose = () => { if (!disposed) { disposed = true; wrapper.unmount(); } };
  cleanups.push(dispose);
  await flushPromises();
  const expectCollapsed = (collapsed: boolean) => {
    expect(wrapper.attributes("data-navigation-collapsed")).toBe(String(collapsed));
    const toggle = wrapper.get(".navigation-collapse");
    expect(toggle.attributes("aria-expanded")).toBe(String(!collapsed));
    expect(toggle.attributes("aria-label")).toBe(collapsed ? "展开导航" : "折叠导航");
    expect(toggle.attributes("title")).toBe(collapsed ? "展开导航" : "折叠导航");
    expect(toggle.attributes("aria-controls")).toBe("business-navigation-links");
    if (collapsed) for (const group of ["aio", "screen"]) {
      expect(wrapper.get(`#${group}-navigation-links`).isVisible()).toBe(false);
      expect(wrapper.get(`button[aria-controls="${group}-navigation-links"]`).attributes("aria-expanded")).toBe("false");
    }
  };
  return {
    wrapper, preferences, projects, router, screen, persist, initialStored, expectCollapsed, dispose,
    async toggle() { await wrapper.get(".navigation-collapse").trigger("click"); await flushPromises(); },
  };
}

describe("桌面导航的响应式折叠", () => {
  it.each([
    { width: 1024, preferredCollapsed: false, expected: true },
    { width: 1080, preferredCollapsed: false, expected: true },
    { width: 1081, preferredCollapsed: false, expected: false },
    { width: 1280, preferredCollapsed: true, expected: true },
  ])("启动宽度$width、宽屏偏好$preferredCollapsed时使用真实状态$expected", async ({ width, preferredCollapsed, expected }) => {
    const view = await render({ width, preferredCollapsed });
    view.expectCollapsed(expected);
    expect(view.screen.matchMedia).toHaveBeenCalledWith(navigationQuery);
    expect(view.preferences.navigationCollapsed).toBe(preferredCollapsed);
    expect(localStorage.getItem(preferenceKey)).toBe(view.initialStored);
    expect(view.persist).not.toHaveBeenCalled();
    if (!expected) {
      expect(view.wrapper.get("#aio-navigation-links").isVisible()).toBe(true);
      expect(view.wrapper.get("#screen-navigation-links").isVisible()).toBe(true);
    }
  });

  it("1080/1081边界往返自动更新子菜单和按钮，不写持久偏好", async () => {
    const view = await render({ width: 1081 });
    view.expectCollapsed(false);
    await view.screen.resize(1080); view.expectCollapsed(true);
    await view.screen.resize(1081); view.expectCollapsed(false);
    expect(view.wrapper.get("#screen-navigation-links").isVisible()).toBe(true);
    expect(view.persist).not.toHaveBeenCalled();
  });

  it.each([false, true])("窄屏手动展开在同区间保留，放大恢复宽屏偏好%s", async preferredCollapsed => {
    const view = await render({ width: 1280, preferredCollapsed });
    view.expectCollapsed(preferredCollapsed);
    await view.screen.resize(1080); view.expectCollapsed(true);
    await view.toggle(); view.expectCollapsed(false);
    await view.screen.resize(1050); view.expectCollapsed(false);
    await view.screen.repeatChange(); view.expectCollapsed(false);
    await view.screen.resize(1024); view.expectCollapsed(false);
    await view.screen.resize(1280); view.expectCollapsed(preferredCollapsed);
    await view.screen.resize(1080); view.expectCollapsed(true);
    expect(view.preferences.navigationCollapsed).toBe(preferredCollapsed);
    expect(localStorage.getItem(preferenceKey)).toBe(view.initialStored);
    expect(view.persist).not.toHaveBeenCalled();
  });

  it("宽屏手动收放照常保存偏好，窄屏切换不覆盖最后的宽屏选择", async () => {
    const view = await render({ width: 1280 });
    await view.toggle(); view.expectCollapsed(true);
    expect(view.preferences.navigationCollapsed).toBe(true);
    expect(JSON.parse(localStorage.getItem(preferenceKey)!)).toMatchObject({ navigationCollapsed: true });
    const writes = view.persist.mock.calls.length;
    await view.screen.resize(1024); await view.toggle(); view.expectCollapsed(false);
    expect(view.persist).toHaveBeenCalledTimes(writes);
    await view.screen.resize(1280); view.expectCollapsed(true);
    await view.toggle(); view.expectCollapsed(false);
    expect(JSON.parse(localStorage.getItem(preferenceKey)!)).toMatchObject({ navigationCollapsed: false });
  });

  it.each([false, true])("窄屏设置快照与保存取消始终保留宽屏偏好%s", async preferredCollapsed => {
    const view = await render({ width: 1024, preferredCollapsed });
    view.expectCollapsed(true);
    const original = { ...view.preferences.snapshot };
    expect(original.navigationCollapsed).toBe(preferredCollapsed);
    view.preferences.apply({ ...original, theme: "dark" });
    expect(JSON.parse(localStorage.getItem(preferenceKey)!)).toMatchObject({ theme: "dark", navigationCollapsed: preferredCollapsed });
    view.expectCollapsed(true);
    await view.toggle(); view.expectCollapsed(false);
    view.preferences.apply(original); await flushPromises();
    expect(JSON.parse(localStorage.getItem(preferenceKey)!)).toMatchObject({ theme: original.theme, navigationCollapsed: preferredCollapsed });
    expect(Object.keys(view.preferences.snapshot).sort()).toEqual(["theme", "fontSize", "density", "reduceMotion", "pageSize", "navigationCollapsed"].sort());
    view.expectCollapsed(false);
    await view.screen.resize(1280); view.expectCollapsed(preferredCollapsed);
  });

  it.each([{ label: "一体机管理", group: "aio" }, { label: "智能屏管理", group: "screen" }])("折叠前已收起$label，点击图标直接展开导航和对应菜单", async ({ label, group }) => {
    const view = await render({ width: 1280 });
    const parent = view.wrapper.get(`button[aria-label="${label}"]`);
    await parent.trigger("click"); await flushPromises();
    expect(view.wrapper.get(`#${group}-navigation-links`).isVisible()).toBe(false);
    await view.screen.resize(1024); view.expectCollapsed(true);
    await parent.trigger("click"); await flushPromises();
    view.expectCollapsed(false);
    expect(parent.attributes("aria-expanded")).toBe("true");
    expect(view.wrapper.get(`#${group}-navigation-links`).isVisible()).toBe(true);
    expect(view.router.currentRoute.value.path).toBe("/screen/nodes");
    expect(view.persist).not.toHaveBeenCalled();
  });

  it("收放导航不改变路由权限，未登录平台仍受限，智能屏仍可访问", async () => {
    const view = await render({ width: 1024 });
    await view.wrapper.get('button[aria-label="一体机管理"]').trigger("click"); await flushPromises();
    view.expectCollapsed(false);
    const platformLink = view.wrapper.get('a[href="/aio/nodes"]');
    expect(platformLink.attributes("aria-disabled")).toBe("true");
    await platformLink.trigger("click"); await flushPromises();
    // 现有 RouterLink 可进入地址，但 Shell 必须继续拦住无权限的业务页面。
    expect(view.wrapper.find('[data-testid="project-access-guard"]').exists()).toBe(true);
    expect(view.wrapper.find('[data-testid="route-page"]').exists()).toBe(false);
    await view.wrapper.get('a[href="/screen/operations"]').trigger("click"); await flushPromises();
    expect(view.router.currentRoute.value.path).toBe("/screen/operations");
    expect(view.wrapper.get('[data-testid="route-page"]').text()).toBe("智能屏操作页");
    await view.toggle(); view.expectCollapsed(true);
    expect(view.router.currentRoute.value.path).toBe("/screen/operations");
    await view.router.push("/aio/nodes"); await flushPromises();
    expect(view.wrapper.find('[data-testid="project-access-guard"]').exists()).toBe(true);
    expect(view.wrapper.find('[data-testid="route-page"]').exists()).toBe(false);
    view.projects.projects[0] = { ...view.projects.projects[0]!, connectionState: "ready", session: { localProjectId: "navigation-project", state: "active" } };
    await flushPromises();
    expect(view.wrapper.find('[data-testid="project-access-guard"]').exists()).toBe(false);
    await view.wrapper.get('button[aria-label="一体机管理"]').trigger("click"); await flushPromises();
    expect(platformLink.attributes("aria-disabled")).toBe("false");
    await view.wrapper.get('a[href="/aio/release"]').trigger("click"); await flushPromises();
    expect(view.router.currentRoute.value.path).toBe("/aio/release");
    expect(view.wrapper.get('[data-testid="route-page"]').text()).toBe("发布参数页");
  });

  it("异步项目初始化期间已经响应窗口断点，不等待业务连接完成", async () => {
    let release!: () => void;
    const pendingInitialize = new Promise<void>(resolve => { release = resolve; });
    const view = await render({ width: 1280, pendingInitialize });
    try {
      expect(view.screen.listenerCount()).toBe(1);
      await view.screen.resize(1024); view.expectCollapsed(true);
      await view.toggle(); view.expectCollapsed(false);
    } finally { release(); await flushPromises(); }
  });

  it("卸载解绑断点监听和退出监听，重新挂载不会累积旧回调", async () => {
    const first = await render({ width: 1024 });
    expect(first.screen.listenerCount()).toBe(1);
    first.dispose();
    expect(first.screen.listenerCount()).toBe(0);
    expect(first.screen.remove).toHaveBeenCalledWith("change", first.screen.add.mock.calls[0]![1]);
    expect(lifecycle.unlisten).toHaveBeenCalledOnce();
    await first.screen.resize(1280);
    expect(first.persist).not.toHaveBeenCalled();
    const second = await render({ width: 1280 });
    expect(second.screen.listenerCount()).toBe(1);
    await second.screen.resize(1024); second.expectCollapsed(true);
    second.dispose();
    expect(second.screen.listenerCount()).toBe(0);
    expect(lifecycle.unlisten).toHaveBeenCalledTimes(2);
  });
});
