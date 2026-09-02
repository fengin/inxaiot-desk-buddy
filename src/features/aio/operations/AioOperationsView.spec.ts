import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";
import { useActivityStore } from "@/stores/activity";
import { useDeploymentWorkflowStore } from "@/stores/deploymentWorkflow";

const recoveredTaskId = "fixture-recovered-deployment";

class RecoveryActivityAdapter extends FixtureActivityAdapter {
  completed = false;
  override async listTasks(projectId: string) {
    return [{
      id: recoveredTaskId,
      projectId,
      domainType: "aio",
      operationType: "full_upgrade",
      name: "恢复中的整包升级",
      state: this.completed ? "succeeded" as const : "running" as const,
      stage: "上传Release",
      progress: this.completed ? 100 : 35,
      targetCount: 1,
      completedCount: this.completed ? 1 : 0,
      updatedAt: new Date().toISOString(),
      cancellable: !this.completed
    }];
  }
}

class RecoveryOperationsAdapter extends FixtureOperationsAdapter {
  completed = false;
  override async getTask(projectId: string, taskId: string) {
    if (taskId !== recoveredTaskId) return super.getTask(projectId, taskId);
    return {
      id: taskId,
      projectId,
      operationType: "full_upgrade",
      name: "恢复中的整包升级",
      state: this.completed ? "succeeded" : "running",
      stage: this.completed ? "completed" : "upload",
      progress: this.completed ? 100 : 35,
      targetCount: 1,
      completedCount: this.completed ? 1 : 0,
      successCount: this.completed ? 1 : 0,
      failureCount: 0,
      cancelledCount: 0,
      cancellable: !this.completed,
      updatedAt: new Date().toISOString(),
      targets: [{
        mac: "000C293BB931",
        state: this.completed ? "succeeded" : "running",
        stage: this.completed ? "completed" : "upload",
        progress: this.completed ? 100 : 35
      }],
      steps: []
    };
  }
}

describe("部署页面活动任务恢复", () => {
  it("restores a running AIO task when the page is reopened", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockImplementation(() => ({
        matches: false,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn()
      }))
    });
    const activityAdapter = new RecoveryActivityAdapter();
    const operationsAdapter = new RecoveryOperationsAdapter();
    configureActivityAdapter(activityAdapter);
    configureOperationsAdapter(operationsAdapter);
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [pinia, router, i18n], stubs: { teleport: true } }
    });
    await flushPromises();

    expect(wrapper.text()).toContain("正在整包升级");
    expect(wrapper.get('[data-testid="operation-task-id"]').text()).toBe(recoveredTaskId);
    expect(wrapper.get('.execution-nodes strong').text()).toBe("AIO-1F-弱电间");
    expect(wrapper.get('.execution-nodes').text()).toContain("上传发布文件");
    expect(wrapper.get('.execution-nodes').text()).not.toContain("upload");
    const historyButton = wrapper.findAll('button').find((button) => button.text() === '查看历史记录');
    expect(historyButton).toBeDefined();
    await historyButton!.trigger('click');
    await flushPromises();
    expect(wrapper.findAll('.history-list button').length).toBeGreaterThan(0);
    expect(wrapper.text()).not.toContain('当前项目没有共享部署操作记录。');

    const activity = useActivityStore(pinia);
    const workflow = useDeploymentWorkflowStore(pinia);
    expect(activity.activeTaskCount).toBe(1);
    activityAdapter.completed = true;
    operationsAdapter.completed = true;
    await workflow.loadTask(workflow.currentTaskProjectId!, recoveredTaskId);
    await flushPromises();
    expect(activity.activeTaskCount).toBe(0);
    expect(wrapper.get('.result-list strong').text()).toBe("AIO-1F-弱电间");
    expect(wrapper.get('.result-list').text()).toContain("已完成");
    wrapper.unmount();
  }, 30000);
});
