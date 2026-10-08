import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { useActivityStore } from "@/stores/activity";
import { useAioNodesStore } from "@/stores/aioNodes";
import { useProjectStore } from "@/stores/projects";

describe("一体机服务检查入口", () => {
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
