import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NCheckbox, NPagination, NSelect } from "naive-ui";
import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { useActivityStore } from "@/stores/activity";
import { useAioNodesStore } from "@/stores/aioNodes";
import { useProjectStore } from "@/stores/projects";
import { usePreferencesStore } from "@/stores/preferences";

describe("一体机服务检查入口", () => {
  it("统一四种部署状态，待处理合并冲突与平台异常并按后端页展示原因", async () => {
    Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() })) });
    const adapter = new FixtureAioAdapter();
    const original = adapter.listNodes.bind(adapter);
    const issues = Array.from({ length: 22 }, (_, index) => ({ platformAioId: `issue-${index}`, name: `异常节点${index}`, ip: `192.0.2.${index + 1}`, code: "INVALID_MAC", rawMac: "无效地址", message: `MAC 格式无效：${index}` }));
    const list = vi.spyOn(adapter, "listNodes").mockImplementation(async (projectId, query) => {
      const result = await original(projectId, query);
      if (query.state !== "attention") return { ...result, platformIssues: issues, stats: { ...result.stats, attention: 23 } };
      const conflicts = (await original(projectId, { ...query, page: 1 })).items;
      const filteredIssues = issues.filter(issue => !query.search || issue.message.includes(query.search));
      const matchingConflicts = query.search ? [] : conflicts;
      const start = (query.page - 1) * query.pageSize;
      return {
        ...result, total: matchingConflicts.length + filteredIssues.length,
        items: matchingConflicts.slice(start, start + query.pageSize), platformIssues: issues,
        pagePlatformIssues: filteredIssues.slice(Math.max(0, start - matchingConflicts.length), Math.max(0, start + query.pageSize - matchingConflicts.length)),
        stats: { ...result.stats, attention: 23 }
      };
    });
    configureAioAdapter(adapter); configureActivityAdapter(new FixtureActivityAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes"); await router.isReady();
    const pinia = createPinia();
    usePreferencesStore(pinia).pageSize = 20;
    const wrapper = mount(App, { attachTo: document.body, global: { plugins: [pinia, router, i18n], stubs: { teleport: true } } });
    try {
      await flushPromises();
      const select = wrapper.findAllComponents(NSelect).find(component => component.classes().includes("state-select"))!;
      expect(select.props("options")).toEqual([
        { label: "全部状态", value: "all" }, { label: "待实施", value: "pending" },
        { label: "已部署", value: "deployed" }, { label: "待处理", value: "attention" },
        { label: "待确认", value: "unconfirmed" }
      ]);
      expect(wrapper.get(".node-table tbody").text()).not.toContain("平台已存在");
      expect(wrapper.get(".node-table tbody").text()).not.toContain("已管理");
      expect(wrapper.get('[data-testid="attention-summary"]').text()).toContain("23");
      await wrapper.get('[data-testid="attention-summary"]').trigger("click");
      await vi.waitFor(() => expect(wrapper.findAll(".platform-issue-row")).toHaveLength(19));
      expect(wrapper.findAll(".node-table tbody tr")).toHaveLength(20);
      expect(wrapper.get(".asset-footer-summary").text()).toContain("共 23 条");
      expect(wrapper.get(".node-table tbody tr .n-tag").attributes("title")).toContain("名称和 IP 与平台记录不一致");
      expect(wrapper.get(".platform-issue-row").text()).toContain("MAC 格式无效：0");
      expect(wrapper.get(".platform-issue-row").getComponent(NCheckbox).props("disabled")).toBe(true);
      expect(wrapper.get(".node-table-header").getComponent(NCheckbox).props("disabled")).toBe(true);
      wrapper.getComponent(NPagination).vm.$emit("update:page", 2);
      await vi.waitFor(() => expect(wrapper.findAll(".platform-issue-row")).toHaveLength(3));
      expect(wrapper.get(".platform-issue-row").text()).toContain("异常节点19");
      expect(wrapper.find(".empty-inline").exists()).toBe(false);
      expect(list.mock.lastCall?.[1]).toMatchObject({ page: 2, state: "attention" });
      await wrapper.get(".search-input input").setValue("MAC 格式无效：21");
      await vi.waitFor(() => expect(wrapper.findAll(".platform-issue-row")).toHaveLength(1));
      expect(wrapper.get(".asset-footer-summary").text()).toContain("共 1 条");
      expect(list.mock.lastCall?.[1]).toMatchObject({ page: 1, search: "MAC 格式无效：21", state: "attention" });
    } finally { wrapper.unmount(); }
  }, 30000);

  it("工作台连接或初始化恢复后自动刷新记录提示", async () => {
    Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() })) });
    const adapter = new FixtureAioAdapter();
    const original = adapter.listNodes.bind(adapter);
    let unavailable = false;
    const list = vi.spyOn(adapter, "listNodes").mockImplementation(async (...args) => ({ ...await original(...args), metadataWarning: unavailable ? "工作台部署记录暂未读取" : null }));
    configureAioAdapter(adapter); configureActivityAdapter(new FixtureActivityAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes"); await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, { attachTo: document.body, global: { plugins: [pinia, router, i18n], stubs: { teleport: true } } });
    try {
      await flushPromises();
      const projects = useProjectStore(pinia);
      unavailable = true; projects.activeProject!.connectionState = "schema_required";
      await flushPromises();
      expect(wrapper.text()).toContain("工作台部署记录暂未读取");
      const before = list.mock.calls.length;
      unavailable = false; projects.activeProject!.connectionState = "ready";
      await flushPromises();
      expect(list.mock.calls.length).toBeGreaterThan(before);
      expect(wrapper.text()).not.toContain("工作台部署记录暂未读取");
    } finally { wrapper.unmount(); }
  }, 30000);

  it("详情区分部署记录与实测，检查后自动展示结果和任务日志", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    const adapter = new FixtureAioAdapter();
    const check = vi.spyOn(adapter, "checkServices");
    configureAioAdapter(adapter);
    configureActivityAdapter(new FixtureActivityAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, { attachTo: document.body, global: { plugins: [pinia, router, i18n], stubs: { teleport: true } } });
    try {
      await flushPromises();
      expect(wrapper.text()).toContain("最近服务检查");
      await wrapper.get(".node-table tbody tr").trigger("click");
      await flushPromises();
      expect(wrapper.findAll('[data-testid="node-service-observation"]')).toHaveLength(4);
      expect(wrapper.text()).toContain("本机最近整机检查：尚未检查");
      expect(wrapper.text()).toContain("服务检查仅保存到本机");
      expect(wrapper.text()).toContain("0/4 项服务有本机实测结果");
      expect(wrapper.get('[data-testid="node-service-observation"]').findAll('dt').map((label) => label.text())).toEqual(["镜像版本", "运行状态", "检查时间", "检查来源"]);
      expect(wrapper.get('[data-testid="node-service-observation"] dd').attributes('title')).toBe("已部署记录，实际镜像尚未采集");
      expect(check).not.toHaveBeenCalled();
      await wrapper.get('[data-testid="check-node-services"]').trigger("click");
      await flushPromises();
      expect(check).toHaveBeenCalledTimes(1);
      expect(wrapper.get('[data-testid="check-node-services"]').attributes("disabled")).toBeDefined();
      expect(useActivityStore(pinia).panelTab).toBe("logs");
      await vi.waitFor(() => {
        expect(wrapper.text()).toContain("4/4 项服务有本机实测结果");
        expect(wrapper.get('[data-testid="check-node-services"]').text()).toBe("检查服务");
      }, { timeout: 2500 });
      expect(wrapper.get('[data-testid="node-service-observation"]').text()).toContain("手动检查");
      expect(wrapper.get('[data-testid="node-service-observation"]').text()).toContain("运行中 · 健康");
      expect(wrapper.get('[data-testid="node-service-observation"]').text()).not.toContain("生效配置镜像");
      const checkedLabels = wrapper.get('[data-testid="node-service-observation"]').findAll('dt').map((label) => label.text());
      expect(checkedLabels.filter((label) => label.includes("镜像") || label.includes("部署"))).toEqual(["镜像版本"]);
      const observed = useAioNodesStore(pinia).detail!.node.serviceCheck!.services[0]!;
      observed.expectedImage = "inx/device-edge:effective-v2";
      observed.state = "version_mismatch";
      await flushPromises();
      expect(wrapper.get('[data-testid="node-service-observation"]').text()).toContain("生效配置镜像inx/device-edge:effective-v2");
      expect(wrapper.get('[data-testid="node-service-observation"]').text()).toContain("版本偏差");
      observed.actualImage = "inx/device-edge:actual-v3";
      await flushPromises();
      expect(wrapper.get('[data-testid="node-service-observation"]').text()).toContain("镜像版本inx/device-edge:actual-v3");
      expect(wrapper.get('[data-testid="node-service-observation"]').findAll('dt').map((label) => label.text())).toContain("已部署版本");
    } finally {
      wrapper.unmount();
    }
  }, 30000);
});
