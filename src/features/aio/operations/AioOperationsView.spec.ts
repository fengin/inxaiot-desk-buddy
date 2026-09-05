import { createPinia } from "pinia";
import { NCheckbox, NProgress } from "naive-ui";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import {
  FixtureActivityAdapter,
  publishFixtureTaskEvent
} from "@/dev-fixtures/activityFixtureAdapter";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";
import { useActivityStore } from "@/stores/activity";
import { useDeploymentWorkflowStore } from "@/stores/deploymentWorkflow";

const recoveredTaskId = "fixture-recovered-deployment";

class RecoveryActivityAdapter extends FixtureActivityAdapter {
  completed = false;
  finalState: "succeeded" | "failed" = "succeeded";
  override async listTasks(projectId: string) {
    return [{
      id: recoveredTaskId,
      projectId,
      domainType: "aio",
      operationType: "full_upgrade",
      name: "恢复中的整包升级",
      state: this.completed ? this.finalState : "running" as const,
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
  finalState: "succeeded" | "failed" = "succeeded";
  override async getTask(projectId: string, taskId: string) {
    if (taskId !== recoveredTaskId) return super.getTask(projectId, taskId);
    return {
      id: taskId,
      projectId,
      operationType: "full_upgrade",
      name: "恢复中的整包升级",
      state: this.completed ? this.finalState : "running",
      stage: this.completed ? "completed" : "upload",
      progress: this.completed ? 100 : 35,
      targetCount: 1,
      completedCount: this.completed ? 1 : 0,
      successCount: this.completed && this.finalState === "succeeded" ? 1 : 0,
      failureCount: this.completed && this.finalState === "failed" ? 1 : 0,
      cancelledCount: 0,
      cancellable: !this.completed,
      updatedAt: new Date().toISOString(),
      targets: [{
        mac: "000C293BB931",
        state: this.completed ? this.finalState : "running",
        stage: this.completed ? "completed" : "upload",
        progress: this.completed ? 100 : 35
      }],
      steps: []
    };
  }
}

class UntaggedFixtureOperationsAdapter extends FixtureOperationsAdapter {
  readonly inspectedExpectedImages: Array<string | undefined> = [];
  readonly preflightImageTags: string[] = [];

  override async inspectImage(path: string, expectedImage?: string) {
    this.inspectedExpectedImages.push(expectedImage);
    return {
      archive: { path, size: 1024, repoTags: [] },
      expectedImage,
      expectedMatches: true
    };
  }

  override async preflight(
    projectId: string,
    preflightTaskId: string,
    plan: Parameters<FixtureOperationsAdapter["preflight"]>[2]
  ) {
    this.preflightImageTags.push(...plan.imageFiles.map((image) => image.imageTag));
    return super.preflight(projectId, preflightTaskId, plan);
  }
}

class PausedPreflightOperationsAdapter extends FixtureOperationsAdapter {
  preflightTaskId = "";
  private releaseProgressEvent!: () => void;
  private releaseCheck!: () => void;
  private markStarted!: () => void;
  private markProgressPublished!: () => void;
  readonly started = new Promise<void>((resolve) => { this.markStarted = resolve; });
  readonly progressPublished = new Promise<void>((resolve) => { this.markProgressPublished = resolve; });
  private readonly progressGate = new Promise<void>((resolve) => { this.releaseProgressEvent = resolve; });
  private readonly gate = new Promise<void>((resolve) => { this.releaseCheck = resolve; });

  publishProgress() {
    this.releaseProgressEvent();
  }

  release() {
    this.releaseProgressEvent();
    this.releaseCheck();
  }

  override async preflight(
    projectId: string,
    preflightTaskId: string,
    plan: Parameters<FixtureOperationsAdapter["preflight"]>[2]
  ) {
    this.preflightTaskId = preflightTaskId;
    this.markStarted();
    await this.progressGate;
    const progressTotal = 3 + plan.targetMacs.length;
    publishFixtureTaskEvent({
      eventId: `${preflightTaskId}-progress`,
      localTaskId: preflightTaskId,
      sequence: 1,
      localProjectId: projectId,
      domainType: "aio",
      resourceType: "aio",
      resourceKey: plan.targetMacs[0] ?? null,
      stage: "检查镜像",
      status: "checking",
      progressCurrent: 1,
      progressTotal,
      level: "info",
      messageCode: "PREFLIGHT_IMAGES_STARTED",
      messageParams: {
        operationType: plan.mode,
        taskName: "部署检查",
        deploymentMode: plan.mode,
        targetCount: String(plan.targetMacs.length)
      },
      message: "正在检查镜像文件",
      timestamp: new Date().toISOString()
    });
    this.markProgressPublished();
    await this.gate;
    return super.preflight(projectId, preflightTaskId, plan);
  }
}

class PausedSubmitOperationsAdapter extends FixtureOperationsAdapter {
  submitCalls = 0;
  preflightCalls = 0;
  inspectCalls = 0;
  submittedProjectId = "";
  submittedPreflightTaskId = "";
  submittedExecutionSnapshot?: Parameters<FixtureOperationsAdapter["submit"]>[2];
  private releaseSubmit!: () => void;
  private markStarted!: () => void;
  readonly started = new Promise<void>((resolve) => { this.markStarted = resolve; });
  private readonly gate = new Promise<void>((resolve) => { this.releaseSubmit = resolve; });

  release() {
    this.releaseSubmit();
  }

  override async inspectImage(path: string, expectedImage?: string) {
    this.inspectCalls += 1;
    return super.inspectImage(path, expectedImage);
  }

  override async preflight(
    projectId: string,
    preflightTaskId: string,
    plan: Parameters<FixtureOperationsAdapter["preflight"]>[2]
  ) {
    this.preflightCalls += 1;
    return super.preflight(projectId, preflightTaskId, plan);
  }

  override async submit(
    projectId: string,
    preflightTaskId: Parameters<FixtureOperationsAdapter["submit"]>[1],
    executionSnapshot: Parameters<FixtureOperationsAdapter["submit"]>[2]
  ) {
    this.submitCalls += 1;
    this.submittedProjectId = projectId;
    this.submittedPreflightTaskId = preflightTaskId;
    this.submittedExecutionSnapshot = executionSnapshot;
    this.markStarted();
    await this.gate;
    return { taskId: "paused-submit-task", state: "queued", submittedAt: new Date().toISOString() };
  }

  override async getTask(projectId: string, taskId: string) {
    if (taskId !== "paused-submit-task") return super.getTask(projectId, taskId);
    return {
      id: taskId,
      projectId,
      operationType: "service_upgrade",
      name: "单服升级",
      state: "queued",
      stage: "queued",
      progress: 0,
      targetCount: 2,
      completedCount: 0,
      successCount: 0,
      failureCount: 0,
      cancelledCount: 0,
      cancellable: true,
      updatedAt: new Date().toISOString(),
      targets: [
        { mac: "00:0C:29:3B:B9:31", state: "pending", stage: "queued", progress: 0, updatedAt: new Date().toISOString() },
        { mac: "00:0C:29:3B:B9:32", state: "pending", stage: "queued", progress: 0, updatedAt: new Date().toISOString() }
      ],
      steps: []
    };
  }
}

class SubmittedTaskActivityAdapter extends FixtureActivityAdapter {
  deploymentVisible = false;

  override async listTasks(projectId: string, limit: number) {
    const tasks = await super.listTasks(projectId, limit);
    if (!this.deploymentVisible) return tasks;
    return [{
      id: "paused-submit-task",
      projectId,
      domainType: "aio",
      operationType: "service_upgrade",
      name: "单服升级",
      state: "queued" as const,
      stage: "排队中",
      progress: 0,
      targetCount: 2,
      completedCount: 0,
      updatedAt: new Date().toISOString(),
      cancellable: true
    }, ...tasks];
  }
}

class FailingSubmitOperationsAdapter extends FixtureOperationsAdapter {
  override async submit() {
    throw {
      code: "NOT_FOUND",
      messageKey: "error.not_found",
      params: { summary: "未找到资源：部署检查任务不存在：fixture-missing" },
      traceId: "fixture-submit-trace"
    };
  }
}

class MissingSnapshotOperationsAdapter extends FixtureOperationsAdapter {
  submitCalls = 0;

  override async preflight(
    projectId: string,
    preflightTaskId: string,
    plan: Parameters<FixtureOperationsAdapter["preflight"]>[2]
  ) {
    const report = await super.preflight(projectId, preflightTaskId, plan);
    return { ...report, executionSnapshot: null };
  }

  override async submit(
    projectId: string,
    preflightTaskId: Parameters<FixtureOperationsAdapter["submit"]>[1],
    executionSnapshot: Parameters<FixtureOperationsAdapter["submit"]>[2]
  ) {
    this.submitCalls += 1;
    return super.submit(projectId, preflightTaskId, executionSnapshot);
  }
}

describe("部署页面活动任务恢复", () => {
  it("检查期间按预检任务事件显示真实事项进度并选中对应日志任务", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    const activityAdapter = new FixtureActivityAdapter();
    const operationsAdapter = new PausedPreflightOperationsAdapter();
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
    try {
      await flushPromises();
      await wrapper.get('[data-testid="operation-mode-service"]').trigger("click");
      await wrapper.get('[data-testid="operation-image-select-device-edge"]').trigger("click");
      await flushPromises();
      await wrapper.get('[data-testid="operation-preflight"]').trigger("click");
      await operationsAdapter.started;
      await flushPromises();

      expect(wrapper.get('[data-testid="operation-preflight-progress-count"]').text()).toBe("已完成 0/7 项");
      operationsAdapter.publishProgress();
      await operationsAdapter.progressPublished;
      await flushPromises();

      const progress = wrapper.get('[data-testid="operation-preflight-progress"]');
      expect(progress.text()).toContain("正在检查镜像文件");
      expect(wrapper.get('[data-testid="operation-preflight-progress-count"]').text()).toBe("已完成 1/7 项");
      expect(wrapper.findComponent(NProgress).props("processing")).toBe(true);
      const activity = useActivityStore(pinia);
      expect(activity.selectedTaskId).toBe(operationsAdapter.preflightTaskId);
      await vi.waitFor(() => {
        expect(activity.tasks.find((task) => task.id === operationsAdapter.preflightTaskId)?.targetCount).toBe(4);
      });
      expect(activity.tasks.find((task) => task.id === operationsAdapter.preflightTaskId)?.completedCount).toBe(0);
      expect(operationsAdapter.preflightTaskId).toMatch(/^[0-9a-f-]{36}$/);

      operationsAdapter.release();
      await vi.waitFor(() => {
        expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(true);
      });
    } finally {
      operationsAdapter.release();
      wrapper.unmount();
    }
  }, 30000);

  it("根据项目Compose生成整包与单服镜像选择项", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    configureActivityAdapter(new FixtureActivityAdapter());
    configureOperationsAdapter(new FixtureOperationsAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [pinia, router, i18n], stubs: { teleport: true } }
    });
    try {
      await flushPromises();
      expect(wrapper.find('[data-testid="release-directory-structure"]').exists()).toBe(false);
      expect(wrapper.text()).not.toContain("本地Release目录");
      const flowBar = wrapper.get(".operation-flow-bar");
      expect(flowBar.find(".operation-mode-bar").exists()).toBe(true);
      expect(flowBar.find(".process-stepper").exists()).toBe(true);
      expect(flowBar.findAll(".operation-mode-option")).toHaveLength(3);
      expect(flowBar.findAll(".process-step")).toHaveLength(4);
      expect(flowBar.findAll(".process-step-heading")).toHaveLength(4);
      expect(flowBar.findAll(".process-step-heading .step-line")).toHaveLength(3);
      for (const stepItem of flowBar.findAll(".process-step")) {
        expect(stepItem.get(".process-step-copy").find(".process-step-heading + small").exists()).toBe(true);
      }
      expect(flowBar.findAll(".process-step small").map((hint) => hint.text())).toEqual([
        "模式、范围和发布文件",
        "配置、连接和执行条件",
        "上传、执行和服务检查",
        "节点结果和共享记录"
      ]);
      expect(wrapper.find(".target-search").exists()).toBe(true);
      expect(wrapper.get('[data-testid="selected-node-count"]').classes()).toContain("metric-info");
      expect(wrapper.get('[data-testid="eligible-node-count"]').classes()).toContain("metric-operation");
      expect(wrapper.get('[data-testid="summary-target-count"]').classes()).toContain("metric-info");
      expect(wrapper.get('[data-testid="summary-image-count"]').classes()).toContain("metric-warning");
      expect(wrapper.get('[data-testid="summary-estimate"]').classes()).toContain("metric-warning");
      expect(wrapper.get(".operation-content").classes()).toContain("selection-scroll-owner");
      const selectedCheckboxes = wrapper.findAllComponents(NCheckbox)
        .filter((checkbox) => checkbox.props("checked"));
      expect(selectedCheckboxes).toHaveLength(4);
      selectedCheckboxes[0].vm.$emit("update:checked", false);
      selectedCheckboxes[1].vm.$emit("update:checked", false);
      await flushPromises();
      expect(wrapper.get('[data-testid="selected-node-count"]').text()).toBe("2");
      expect((wrapper.get('[data-testid="operation-batch-size"] input').element as HTMLInputElement).value).toBe("2");
      expect((wrapper.get('[data-testid="operation-concurrency"] input').element as HTMLInputElement).value).toBe("2");
      const firstNode = wrapper.get(".node-selection-list label");
      expect(firstNode.find(".node-selection-name").exists()).toBe(true);
      expect(firstNode.find(".node-selection-ip").exists()).toBe(true);
      expect(firstNode.find(".node-selection-location").exists()).toBe(true);
      expect(wrapper.findAll(".service-image-row")).toHaveLength(2);
      for (const row of wrapper.findAll(".service-image-row")) {
        const fields = row.get(".service-image-fields");
        expect(fields.find(".image-file-picker").exists()).toBe(true);
        expect(fields.findAll(".n-input").length).toBeGreaterThanOrEqual(2);
      }
      expect(wrapper.text()).toContain("rule-engine");

      await wrapper.get('[data-testid="operation-image-select-device-edge"]').trigger("click");
      await flushPromises();
      const parsedTag = wrapper.get('[data-testid="operation-image-tag-device-edge"] input');
      expect(parsedTag.attributes("readonly")).toBeDefined();
      expect((parsedTag.element as HTMLInputElement).value).toBe("inx/device-edge:fixture");

      await wrapper.get('[data-testid="operation-mode-service"]').trigger("click");
      await flushPromises();
      expect(wrapper.findAll(".service-image-row")).toHaveLength(1);
      expect(wrapper.text()).toContain("目标服务");
    } finally {
      wrapper.unmount();
    }
  }, 30000);

  it("检查结果摘要使用语义色突出服务、目标和并发数值", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    configureActivityAdapter(new FixtureActivityAdapter());
    configureOperationsAdapter(new FixtureOperationsAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n], stubs: { teleport: true } }
    });
    try {
      await flushPromises();
      await wrapper.get('[data-testid="operation-image-select-device-edge"]').trigger("click");
      await wrapper.get('[data-testid="operation-image-select-rule-engine"]').trigger("click");
      await wrapper.get('[data-testid="operation-preflight"]').trigger("click");
      await vi.waitFor(() => {
        expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(true);
      });

      const imageCount = wrapper.get('[data-testid="preflight-summary-image-count"]');
      const targetCount = wrapper.get('[data-testid="preflight-summary-target-count"]');
      const concurrencyCount = wrapper.get('[data-testid="preflight-summary-concurrency"]');
      expect(imageCount.text()).toBe("2");
      expect(imageCount.classes()).toContain("metric-operation");
      expect(targetCount.text()).toBe("4");
      expect(targetCount.classes()).toContain("metric-info");
      expect(concurrencyCount.text()).toBe("2");
      expect(concurrencyCount.classes()).toContain("metric-warning");
    } finally {
      wrapper.unmount();
    }
  }, 30000);

  it("未解析到RepoTag时允许手工填写并进入检查", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    configureActivityAdapter(new FixtureActivityAdapter());
    const operationsAdapter = new UntaggedFixtureOperationsAdapter();
    configureOperationsAdapter(operationsAdapter);
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [pinia, router, i18n], stubs: { teleport: true } }
    });
    try {
      await flushPromises();
      await wrapper.get('[data-testid="operation-mode-service"]').trigger("click");
      await wrapper.get('[data-testid="operation-image-select-device-edge"]').trigger("click");
      await flushPromises();
      const tagInput = wrapper.get('[data-testid="operation-image-tag-device-edge"] input');
      expect(tagInput.attributes("readonly")).toBeUndefined();
      await tagInput.setValue("registry.example/device-edge:manual");
      await flushPromises();
      expect((tagInput.element as HTMLInputElement).value).toBe("registry.example/device-edge:manual");
      const selectAll = wrapper.findAll("button").find((button) => button.text() === "选择全部匹配");
      expect(selectAll).toBeDefined();
      await selectAll!.trigger("click");
      const preflight = wrapper.get('[data-testid="operation-preflight"]');
      expect(preflight.attributes("disabled")).toBeUndefined();
      await preflight.trigger("click");
      await flushPromises();
      expect(operationsAdapter.inspectedExpectedImages).toEqual([undefined]);
      expect(operationsAdapter.preflightImageTags).toContain("registry.example/device-edge:manual");
      await vi.waitFor(() => {
        expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(true);
      });
      const statusTag = wrapper.get(".preflight-status-tag");
      expect(statusTag.text()).toBe("全部通过");
      expect(statusTag.get(".preflight-status-label").text()).toBe("全部通过");
    } finally {
      wrapper.unmount();
    }
  }, 30000);

  it("直接使用第一次检查快照创建任务并进入真实部署进度", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    const activityAdapter = new SubmittedTaskActivityAdapter();
    configureActivityAdapter(activityAdapter);
    const operationsAdapter = new PausedSubmitOperationsAdapter();
    configureOperationsAdapter(operationsAdapter);
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [pinia, router, i18n], stubs: { teleport: true } }
    });
    try {
      await flushPromises();
      await wrapper.get('[data-testid="operation-mode-service"]').trigger("click");
      await wrapper.get('[data-testid="operation-image-select-device-edge"]').trigger("click");
      await wrapper.get('[data-testid="operation-preflight"]').trigger("click");
      await vi.waitFor(() => {
        expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(true);
      });
      const workflow = useDeploymentWorkflowStore(pinia);
      const checkedTaskId = workflow.preflightTaskId;
      const checkedSnapshot = workflow.preflight?.executionSnapshot;
      const preflightCallsBeforeSubmit = operationsAdapter.preflightCalls;
      const inspectCallsBeforeSubmit = operationsAdapter.inspectCalls;

      const submit = wrapper.get('[data-testid="operation-submit"]');
      void submit.trigger("click");
      void submit.trigger("click");
      await operationsAdapter.started;
      await flushPromises();
      expect(operationsAdapter.submitCalls).toBe(1);
      expect(operationsAdapter.preflightCalls).toBe(preflightCallsBeforeSubmit);
      expect(operationsAdapter.inspectCalls).toBe(inspectCallsBeforeSubmit);
      expect(operationsAdapter.submittedProjectId).toBe(checkedSnapshot?.localProjectId);
      expect(operationsAdapter.submittedPreflightTaskId).toBe(checkedTaskId);
      expect(operationsAdapter.submittedExecutionSnapshot).toBe(checkedSnapshot);
      expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(true);
      expect(wrapper.get('[data-testid="operation-submit"]').text()).toContain("正在创建部署任务");
      expect(wrapper.find('[data-testid="operation-execution"]').exists()).toBe(false);

      activityAdapter.deploymentVisible = true;
      operationsAdapter.release();
      await vi.waitFor(() => {
        expect(wrapper.find('[data-testid="operation-execution"]').exists()).toBe(true);
      });
      expect(wrapper.get('[data-testid="operation-execution-percentage"]').text()).toBe("0%");
      expect(wrapper.get('[data-testid="operation-execution-result-count"]').text()).toBe("0/2 台形成最终结果");
      expect(wrapper.get('[data-testid="operation-execution-progress"]').findComponent(NProgress).props("percentage")).toBe(0);
      expect(wrapper.get(".execution-task-state").text()).toBe("排队中");
      expect(wrapper.findAll(".execution-target-state").map((tag) => tag.text())).toEqual(["待执行", "待执行"]);
      expect(wrapper.text()).not.toContain("queued");
      expect(wrapper.text()).not.toContain("pending");
      expect(wrapper.findComponent(NProgress).props("processing")).toBe(true);
      expect(useActivityStore(pinia).selectedTaskId).toBe("paused-submit-task");
    } finally {
      operationsAdapter.release();
      wrapper.unmount();
    }
  }, 30000);

  it("提交失败时显示Tauri返回的具体原因并使已丢失检查结果失效", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    configureActivityAdapter(new FixtureActivityAdapter());
    configureOperationsAdapter(new FailingSubmitOperationsAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [pinia, router, i18n], stubs: { teleport: true } }
    });
    try {
      await flushPromises();
      await wrapper.get('[data-testid="operation-mode-service"]').trigger("click");
      await wrapper.get('[data-testid="operation-image-select-device-edge"]').trigger("click");
      await wrapper.get('[data-testid="operation-preflight"]').trigger("click");
      await vi.waitFor(() => {
        expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(true);
      });

      await wrapper.get('[data-testid="operation-submit"]').trigger("click");
      await vi.waitFor(() => {
        expect(document.body.textContent).toContain("部署检查任务不存在：fixture-missing");
      });
      expect(useDeploymentWorkflowStore(pinia).preflight).toBeUndefined();
      expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(false);
      expect(wrapper.find('[data-testid="operation-preflight"]').exists()).toBe(true);
    } finally {
      wrapper.unmount();
    }
  }, 30000);

  it("检查通过但缺少执行快照时提示重新检查且不允许提交", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    configureActivityAdapter(new FixtureActivityAdapter());
    const operationsAdapter = new MissingSnapshotOperationsAdapter();
    configureOperationsAdapter(operationsAdapter);
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/operations");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n], stubs: { teleport: true } }
    });
    try {
      await flushPromises();
      await wrapper.get('[data-testid="operation-mode-service"]').trigger("click");
      await wrapper.get('[data-testid="operation-image-select-device-edge"]').trigger("click");
      await wrapper.get('[data-testid="operation-preflight"]').trigger("click");
      await vi.waitFor(() => {
        expect(wrapper.find('[data-testid="operation-preflight-report"]').exists()).toBe(true);
      });

      expect(wrapper.get('[data-testid="operation-preflight-snapshot-warning"]').text()).toBe("本次检查结果不完整，请返回重新检查");
      const submit = wrapper.get('[data-testid="operation-submit"]');
      expect(submit.attributes("disabled")).toBeDefined();
      await submit.trigger("click");
      expect(operationsAdapter.submitCalls).toBe(0);
    } finally {
      wrapper.unmount();
    }
  }, 30000);

  it.each(["succeeded", "failed"] as const)("restores a running AIO task and presents its %s result accurately", async (finalState) => {
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
    activityAdapter.finalState = finalState;
    operationsAdapter.finalState = finalState;
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
    expect(wrapper.get('[data-testid="operation-execution-percentage"]').text()).toBe("35%");
    expect(wrapper.get('[data-testid="operation-execution-result-count"]').text()).toBe("0/1 台形成最终结果");
    expect(wrapper.get('[data-testid="operation-execution-progress"]').findComponent(NProgress).props("percentage")).toBe(35);
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
    expect(wrapper.get('.result-hero-title').text()).toContain("已形成最终结果");
    expect(wrapper.get('.result-hero-summary').text()).toContain("成功");
    expect(wrapper.get('.result-hero-summary').text()).not.toContain("Task");
    expect(wrapper.get('.result-hero-summary').text()).not.toContain("执行摘要");
    const resultStep = wrapper.findAll('.process-step')[3]!;
    expect(wrapper.get('.result-list strong').text()).toBe("AIO-1F-弱电间");
    if (finalState === "succeeded") {
      expect(resultStep.classes()).toContain("result-success");
      expect(resultStep.find('.lucide-check').exists()).toBe(true);
      expect(wrapper.get('.result-list').text()).toContain("已完成");
      expect(wrapper.find('.result-hero.result-not-success').exists()).toBe(false);
    } else {
      expect(resultStep.classes()).toContain("result-warning");
      expect(resultStep.find('.lucide-circle-alert').exists()).toBe(true);
      expect(wrapper.get('.result-list').text()).toContain("失败");
      expect(wrapper.get('.result-list').text()).not.toContain("已完成");
      expect(wrapper.find('.result-hero.result-not-success').exists()).toBe(true);
      expect(wrapper.find('.result-list .lucide-circle-alert').exists()).toBe(true);
    }
    wrapper.unmount();
  }, 30000);
});
