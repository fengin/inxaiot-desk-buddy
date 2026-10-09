import { NMessageProvider } from "naive-ui";
import { createPinia } from "pinia";
import { defineComponent, h } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import type { LocationQueryRaw } from "vue-router";
import { describe, expect, it, vi } from "vitest";

import { router } from "@/app/router";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { useProjectStore } from "@/stores/projects";
import AioOperationsView from "./AioOperationsView.vue";

const projectId = "project-shenzhen-bay";

async function renderSelection(query: LocationQueryRaw) {
  configureActivityAdapter(new FixtureActivityAdapter());
  configureAioAdapter(new FixtureAioAdapter());
  configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
  const operations = new FixtureOperationsAdapter();
  const preflight = vi.spyOn(operations, "preflight");
  const submit = vi.spyOn(operations, "submit");
  configureOperationsAdapter(operations);
  const pinia = createPinia();
  const projects = useProjectStore(pinia);
  await projects.initialize();
  await router.push({ name: "aio-operations", query });
  await router.isReady();
  const wrapper = mount(defineComponent({
    setup: () => () => h(NMessageProvider, null, { default: () => h(AioOperationsView) })
  }), { global: { plugins: [pinia, router], stubs: { teleport: true } } });
  await flushPromises();
  const selectedNames = () => wrapper.findAll(".node-selection-list label.selected .node-selection-name")
    .map((node) => node.text());
  return { wrapper, projects, selectedNames, preflight, submit };
}

describe("一体机列表选择带入部署升级", () => {
  it("默认勾选已部署且在线节点，包含没有工作台部署历史的平台注册节点，最多四台", async () => {
    const context = await renderSelection({});
    try {
      expect(context.selectedNames()).toEqual(["AIO-1F-弱电间", "AIO-2F-弱电间", "AIO-3F-弱电间", "AIO-B栋-2F"]);
      expect(context.selectedNames()).not.toContain("AIO-B栋-3F");
      expect(context.preflight).not.toHaveBeenCalled();
      expect(context.submit).not.toHaveBeenCalled();
    } finally { context.wrapper.unmount(); }
  });

  it("只勾选列表指定的离线及待实施设备，兼容MAC格式并去重", async () => {
    const context = await renderSelection({
      targets: ["000c293bb934", "00-0c-29-3b-b9-35", "00:0C:29:3B:B9:34"],
      project: projectId
    });
    try {
      expect(context.selectedNames()).toEqual(["AIO-B1-设备间", "AIO-B栋-1F"]);
      expect(context.wrapper.get('[data-testid="operation-mode-service"] input').element).toHaveProperty("checked", true);
      expect(context.wrapper.get('[data-testid="selected-node-count"]').text()).toBe("2");
      expect(router.currentRoute.value.query).toEqual({});
      expect(context.preflight).not.toHaveBeenCalled();
      expect(context.submit).not.toHaveBeenCalled();
    } finally { context.wrapper.unmount(); }
  });

  it("目标已移除或存在冲突时提示并仅保留原选择中的有效设备", async () => {
    const context = await renderSelection({
      targets: ["000C293BB931", "000C293BB937", "FFFFFFFFFFFF"], project: projectId
    });
    try {
      expect(context.selectedNames()).toEqual(["AIO-1F-弱电间"]);
      expect(context.wrapper.get('[data-testid="operation-target-selection-notice"]').text())
        .toContain("2 台一体机已移除或存在信息冲突");
    } finally { context.wrapper.unmount(); }
  });

  it.each([{ targets: ["FFFFFFFFFFFF"] }, { targets: [] }])("全部目标不可用或为空时不自动补选在线设备：$targets", async ({ targets }) => {
    const context = await renderSelection({ targets: targets.length ? targets : "", project: projectId });
    try {
      expect(context.selectedNames()).toEqual([]);
      expect(context.wrapper.get('[data-testid="operation-preflight"]').attributes("disabled")).toBeDefined();
      expect(context.wrapper.find('[data-testid="operation-target-selection-notice"]').exists()).toBe(true);
    } finally { context.wrapper.unmount(); }
  });

  it.each(["another-project", undefined])("批选缺少正确项目归属时不带入设备：%s", async (project) => {
    const context = await renderSelection({ targets: ["000C293BB934"], project });
    try {
      expect(context.selectedNames()).toEqual([]);
      expect(context.wrapper.get('[data-testid="operation-target-selection-notice"]').text()).toContain("属于其他项目");
    } finally { context.wrapper.unmount(); }
  });

  it("兼容详情页原有的单台target跳转", async () => {
    const context = await renderSelection({ target: "00:0C:29:3B:B9:34" });
    try {
      expect(context.selectedNames()).toEqual(["AIO-B1-设备间"]);
      expect(router.currentRoute.value.query).toEqual({});
    } finally { context.wrapper.unmount(); }
  });

  it("切换项目后不复用上一项目的批选目标，再切回来也不重新套用", async () => {
    const context = await renderSelection({ targets: ["000C293BB935"], project: projectId });
    try {
      expect(context.selectedNames()).toEqual(["AIO-B栋-1F"]);
      const otherId = "selection-other-project";
      context.projects.projects.push({ ...context.projects.activeProject!, id: otherId });
      context.projects.activeProjectId = otherId;
      await flushPromises();
      expect(context.selectedNames()).toHaveLength(4);
      expect(context.selectedNames()).not.toContain("AIO-B栋-1F");
      context.projects.activeProjectId = projectId;
      await flushPromises();
      expect(context.selectedNames()).toHaveLength(4);
      expect(context.selectedNames()).not.toContain("AIO-B栋-1F");
    } finally { context.wrapper.unmount(); }
  });
});
