import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { h } from "vue";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, expect, it, vi } from "vitest";
import { NCheckbox, NInput, NMessageProvider, NSelect } from "naive-ui";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import type { AioNodeListItem, AioNodeListPage } from "@/shared/model/aio";
import type { ProjectOverview } from "@/shared/model/project";
import { useAioNodesStore } from "@/stores/aioNodes";
import { useProjectStore } from "@/stores/projects";
import { usePreferencesStore } from "@/stores/preferences";
import AioNodeListView from "./AioNodeListView.vue";

const wrappers: VueWrapper[] = [];
const project = (id: string): ProjectOverview => ({
  id, name: `项目${id}`, platformUrl: "http://192.0.2.1", dbHost: "192.0.2.1", dbPort: 3306,
  dbUser: "sample", businessDb: "sample", workbenchDb: "sample", dbTlsEnabled: false,
  connectionState: "ready", databaseState: "connected", connectionEncrypted: false, statusMessage: "测试状态",
  session: { localProjectId: id, state: "active" }
});
const nodes: AioNodeListItem[] = Array.from({ length: 45 }, (_, index) => ({
  mac: index.toString(16).padStart(12, "0"), macNormalized: index.toString(16).padStart(12, "0"),
  name: `一体机${index}`, ip: `192.0.2.${index + 1}`, location: "测试位置", managementState: "managed",
  deploymentState: "deployed", deployLabel: "已部署", platformState: "online", platformUpdatedAt: "2026-10-20T00:00:00Z",
  serviceState: "unknown", serviceLabel: "未检查", lastOperation: "测试操作", source: "platform", version: 1, conflicts: [], versions: []
}));
function page(query: { page: number; pageSize: number; search?: string }): AioNodeListPage {
  const matching = nodes.filter(node => !query.search || node.name.includes(query.search));
  return {
    items: matching.slice((query.page - 1) * query.pageSize, query.page * query.pageSize),
    total: matching.length, page: query.page, pageSize: query.pageSize,
    stats: { total: 45, online: 45, offline: 0, pending: 0, deployed: 45, attention: 0, unconfirmed: 0, conflicts: 0 },
    platformIssues: [], pagePlatformIssues: [], refreshedAt: "2026-10-20T00:00:00Z"
  };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}

async function renderList(options: { initialized?: boolean; initialPage?: number; firstPage?: Promise<AioNodeListPage>; switchingTo?: Promise<ProjectOverview> } = {}) {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const adapter = new FixtureAioAdapter();
  const list = vi.spyOn(adapter, "listNodes").mockImplementation(async (_id, query) => page(query));
  if (options.firstPage) list.mockImplementationOnce(() => options.firstPage!);
  configureAioAdapter(adapter);
  const workbench = new FixtureWorkbenchAdapter();
  vi.spyOn(workbench, "listProjects").mockResolvedValue([project("a"), project("b")]);
  const switchProject = vi.spyOn(workbench, "switchProject").mockImplementation(async id => project(id));
  if (options.switchingTo) switchProject.mockImplementationOnce(() => options.switchingTo!);
  configureWorkbenchAdapter(workbench);
  const pinia = createPinia(); setActivePinia(pinia);
  const projects = useProjectStore(pinia);
  projects.initialized = options.initialized !== false;
  if (projects.initialized) { projects.projects = [project("a"), project("b")]; projects.activeProjectId = "a"; }
  usePreferencesStore(pinia).pageSize = 20;
  const aio = useAioNodesStore(pinia);
  if (options.initialPage) { aio.setProjectContext("a"); aio.page = options.initialPage; }
  const context = vi.spyOn(aio, "setProjectContext");
  const router = createRouter({ history: createMemoryHistory(), routes: [{ path: "/", component: { template: "<div />" } }] });
  await router.push("/"); await router.isReady();
  const wrapper = mount({ render: () => h(NMessageProvider, () => h(AioNodeListView)) }, { attachTo: document.body, global: { plugins: [pinia, router] } });
  wrappers.push(wrapper);
  await flushPromises();
  const input = wrapper.findAllComponents(NInput).find(item => item.classes().includes("search-input"))!;
  return { wrapper, list, aio, projects, input, switchProject, pinia, context };
}

afterEach(() => {
  for (const wrapper of wrappers.splice(0)) wrapper.unmount();
  vi.useRealTimers(); vi.restoreAllMocks(); document.body.innerHTML = "";
});

it("已初始化项目首挂载只读取一次，分页和每页数量立即各读取一次", async () => {
  const { list, aio, pinia, projects } = await renderList();
  expect(list).toHaveBeenCalledTimes(1);
  projects.projects = projects.projects.map(item => ({ ...item, statusMessage: "连接提示文字更新" })); await flushPromises();
  expect(list).toHaveBeenCalledTimes(1);
  list.mockClear(); aio.page = 2; await flushPromises();
  expect(list).toHaveBeenCalledExactlyOnceWith("a", { search: undefined, state: "all", page: 2, pageSize: 20 });
  await vi.advanceTimersByTimeAsync(200); expect(list).toHaveBeenCalledTimes(1);
  list.mockClear(); usePreferencesStore(pinia).pageSize = 50; await flushPromises();
  expect(aio.page).toBe(1);
  expect(list).toHaveBeenCalledExactlyOnceWith("a", { search: undefined, state: "all", page: 1, pageSize: 50 });
});

it("返回同项目列表保留已有页码，仍只读取一次", async () => {
  const { list, aio } = await renderList({ initialPage: 2 });
  expect(aio.page).toBe(2);
  expect(list).toHaveBeenCalledExactlyOnceWith("a", { search: undefined, state: "all", page: 2, pageSize: 20 });
});

it("非首页连续改搜索与状态只在防抖后各读取一次，并保留原有跨页选择", async () => {
  const { wrapper, list, aio, input } = await renderList();
  wrapper.get(".node-table-header").getComponent(NCheckbox).vm.$emit("update:checked", true); await flushPromises();
  aio.page = 2; await flushPromises(); list.mockClear();
  for (const value of ["一", "一体", "一体机4"]) { input.vm.$emit("update:value", value); await flushPromises(); }
  expect(aio.page).toBe(1); expect(list).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(179); expect(list).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1); await flushPromises();
  expect(list).toHaveBeenCalledExactlyOnceWith("a", { search: "一体机4", state: "all", page: 1, pageSize: 20 });
  expect(wrapper.get(".asset-footer-summary").text()).toContain("已选 20 台");
  aio.page = 2; await flushPromises(); list.mockClear();
  wrapper.findAllComponents(NSelect).find(item => item.classes().includes("state-select"))!.vm.$emit("update:value", "deployed");
  await flushPromises(); expect(list).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(180); await flushPromises();
  expect(list).toHaveBeenCalledExactlyOnceWith("a", { search: "一体机4", state: "deployed", page: 1, pageSize: 20 });
});

it("切项目立即清旧上下文和选择，等待切换完成后只读取新项目第一页一次", async () => {
  const switched = deferred<ProjectOverview>();
  const { wrapper, list, aio, projects, context } = await renderList({ switchingTo: switched.promise });
  wrapper.get(".node-table-header").getComponent(NCheckbox).vm.$emit("update:checked", true); await flushPromises();
  aio.page = 2; await flushPromises(); list.mockClear();
  const switching = projects.switchProject("b"); await flushPromises();
  expect(context).toHaveBeenLastCalledWith("b"); expect(aio.nodes).toEqual([]); expect(list).not.toHaveBeenCalled();
  expect(wrapper.get(".asset-footer-summary").text()).toContain("已选 0 台");
  switched.resolve(project("b")); await switching; await flushPromises();
  expect(list).toHaveBeenCalledExactlyOnceWith("b", { search: undefined, state: "all", page: 1, pageSize: 20 });
  await vi.advanceTimersByTimeAsync(200); expect(list).toHaveBeenCalledTimes(1);
});

it("初次初始化完成只读一次，同项目连接及登录恢复仍自动刷新", async () => {
  const initialized = deferred<ProjectOverview>();
  const { list, projects } = await renderList({ initialized: false, switchingTo: initialized.promise });
  expect(projects.loading).toBe(true); expect(list).not.toHaveBeenCalled();
  initialized.resolve(project("a")); await flushPromises();
  expect(list).toHaveBeenCalledTimes(1);
  list.mockClear(); projects.activeProject!.connectionState = "schema_required"; await flushPromises();
  expect(list).toHaveBeenCalledTimes(1);
  list.mockClear(); projects.activeProject!.connectionState = "ready"; await flushPromises();
  expect(list).toHaveBeenCalledTimes(1);
  list.mockClear(); projects.activeProject!.session = { localProjectId: "a", state: "expired" }; projects.activeProject!.connectionState = "session_expired"; await flushPromises();
  expect(list).not.toHaveBeenCalled();
  projects.activeProject!.session = { localProjectId: "a", state: "active" }; projects.activeProject!.connectionState = "ready"; await flushPromises();
  expect(list).toHaveBeenCalledTimes(1);
});

it("切到无访问权限项目后，旧列表响应不能回写或残留", async () => {
  const old = deferred<AioNodeListPage>();
  const denied = project("b"); denied.session = { localProjectId: "b", state: "expired" }; denied.connectionState = "session_expired";
  const { list, aio, projects, context } = await renderList({ firstPage: old.promise, switchingTo: Promise.resolve(denied) });
  expect(list).toHaveBeenCalledTimes(1);
  await projects.switchProject("b"); await flushPromises();
  expect(list).toHaveBeenCalledTimes(1); expect(context).toHaveBeenLastCalledWith("b"); expect(aio.loading).toBe(false);
  old.resolve(page({ page: 1, pageSize: 20 })); await flushPromises();
  expect(aio.nodes).toEqual([]); expect(aio.total).toBe(0); expect(aio.metadataWarning).toBe("");
});

it("显式刷新取消待执行的筛选读取，退出页面也取消待执行计时器", async () => {
  const { wrapper, list, input } = await renderList();
  list.mockClear(); input.vm.$emit("update:value", "一体机4"); await flushPromises();
  await wrapper.get('[title="刷新列表"]').trigger("click"); await flushPromises();
  expect(list).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(200); expect(list).toHaveBeenCalledTimes(1);
  list.mockClear(); input.vm.$emit("update:value", "一体机3"); await flushPromises();
  wrapper.unmount(); wrappers.splice(wrappers.indexOf(wrapper), 1);
  await vi.advanceTimersByTimeAsync(200); expect(list).not.toHaveBeenCalled();
});
