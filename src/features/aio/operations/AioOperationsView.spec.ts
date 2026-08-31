import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";

const recoveredTaskId = "fixture-recovered-deployment";

class RecoveryActivityAdapter extends FixtureActivityAdapter {
  override async listTasks(projectId: string) {
    return [{
      id: recoveredTaskId,
      projectId,
      domainType: "aio",
      operationType: "full_upgrade",
      name: "恢复中的整包升级",
      state: "running" as const,
      stage: "上传Release",
      progress: 35,
      targetCount: 1,
      completedCount: 0,
      updatedAt: new Date().toISOString(),
      cancellable: true
    }];
  }
}

class RecoveryOperationsAdapter extends FixtureOperationsAdapter {
  override async getTask(projectId: string, taskId: string) {
    if (taskId !== recoveredTaskId) return super.getTask(projectId, taskId);
    return {
      id: taskId,
      projectId,
      operationType: "full_upgrade",
      name: "恢复中的整包升级",
      state: "running",
      stage: "上传Release",
      progress: 35,
      targetCount: 1,
      completedCount: 0,
      successCount: 0,
      failureCount: 0,
      cancelledCount: 0,
      cancellable: true,
      updatedAt: new Date().toISOString(),
      targets: [{
        mac: "00:0C:29:3B:B9:39",
        macNormalized: "000C293BB939",
        state: "running",
        stage: "上传Release",
        progress: 35
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
    configureActivityAdapter(new RecoveryActivityAdapter());
    configureOperationsAdapter(new RecoveryOperationsAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    await flushPromises();

    expect(wrapper.text()).toContain("正在整包升级");
    expect(wrapper.get('[data-testid="operation-task-id"]').text()).toBe(recoveredTaskId);
    expect(wrapper.text()).toContain("上传Release");
    wrapper.unmount();
  }, 30000);
});
