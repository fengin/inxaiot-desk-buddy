import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NMessageProvider } from "naive-ui";
import { h } from "vue";

import ActivityPanel from "@/shell/ActivityPanel.vue";
import { useActivityStore } from "@/stores/activity";

describe("activity panel", () => {
  it("renders the shared task DTO and switches to filtered local logs", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const activity = useActivityStore();
    activity.openPanel("tasks");
    const wrapper = mount(
      { render: () => h(NMessageProvider, () => h(ActivityPanel)) },
      { global: { plugins: [pinia] } }
    );
    await flushPromises();
    expect(wrapper.get("[data-testid='activity-panel']").text()).toContain("A栋一体机整包升级");
    const taskRow = wrapper.get(".task-row");
    expect(taskRow.get(".task-name .task-target-count").text()).toMatch(/^\d+ 个目标$/);
    const taskProgress = taskRow.get(".task-progress");
    expect(taskProgress.element.firstElementChild?.classList.contains("task-progress-value")).toBe(true);
    expect(taskProgress.find(".n-progress").exists()).toBe(true);
    expect(taskRow.get(".task-time").element.tagName).toBe("TIME");
    expect(wrapper.get("[title='清空已结束任务记录']").attributes("disabled")).toBeUndefined();
    await taskRow.trigger("click");
    await flushPromises();
    expect(activity.panelTab).toBe("logs");
    expect(wrapper.get("[title='清空当前任务日志']").attributes("disabled")).toBeUndefined();
    expect(wrapper.findAll(".log-line")).toHaveLength(3);
    expect(wrapper.findAll(".log-time")).toHaveLength(3);
    expect(wrapper.get(".log-time").text()).toMatch(/^(?:\d{4}-\d{2}-\d{2} )?\d{2}:\d{2}:\d{2}$/);

    activity.logLevels = ["WARN"];
    await activity.refreshLogs();
    await flushPromises();
    expect(wrapper.findAll(".log-line")).toHaveLength(0);
    wrapper.unmount();
  });

  it("renders queued and terminal deployment stages in Chinese with result-aware wording", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const activity = useActivityStore();
    activity.openPanel("tasks");
    const wrapper = mount(
      { render: () => h(NMessageProvider, () => h(ActivityPanel)) },
      { global: { plugins: [pinia] } }
    );
    await flushPromises();
    activity.tasks = [
      {
        id: "queued-task", projectId: "project", domainType: "aio", operationType: "first_deploy",
        name: "首次部署", state: "queued", stage: "queued", progress: 0,
        targetCount: 2, completedCount: 0, updatedAt: "2026-09-05 17:37:07", cancellable: true
      },
      {
        id: "failed-task", projectId: "project", domainType: "aio", operationType: "first_deploy",
        name: "首次部署", state: "failed", stage: "completed", progress: 100,
        targetCount: 2, completedCount: 2, updatedAt: "2026-09-05 17:38:03", cancellable: false
      }
    ];
    await flushPromises();

    expect(wrapper.text()).toContain("排队中");
    expect(wrapper.text()).toContain("执行失败");
    expect(wrapper.text()).not.toContain("queued");
    expect(wrapper.text()).not.toContain("completed");
    wrapper.unmount();
  });

  it("translates every deployment pipeline stage code without exposing English enums", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const activity = useActivityStore();
    activity.openPanel("tasks");
    const stages = [
      ["draft", "准备任务"],
      ["checking", "检查中"],
      ["check_failed", "检查失败"],
      ["ready", "等待执行"],
      ["queued", "排队中"],
      ["prepare_config", "读取发布配置"],
      ["prepare_artifact", "准备镜像文件"],
      ["prepare_release", "生成发布包"],
      ["prepare_target", "准备一体机配置"],
      ["prepare_lease", "确认任务可执行"],
      ["prepare_lease_failed", "任务执行条件确认失败"],
      ["prepare_local", "准备发布文件"],
      ["prepare_remote", "准备一体机"],
      ["lease_acquired", "已确认可以执行"],
      ["ssh_connect", "连接一体机"],
      ["ssh_connected", "连接完成"],
      ["upload", "上传发布文件"],
      ["prepare_agent", "准备执行脚本"],
      ["precheck", "检查运行环境"],
      ["backup", "备份现有数据"],
      ["install", "安装发布包"],
      ["register", "确认平台注册"],
      ["health", "检查服务状态"],
      ["service_check", "检查目标服务"],
      ["service_upgrade", "升级目标服务"],
      ["service_health", "检查目标服务状态"],
      ["remote_complete", "一体机操作完成"],
      ["finalizing", "保存执行结果"],
      ["completed", "已完成"],
      ["cancelling", "正在取消"],
      ["cancelled", "已取消"],
      ["failed", "执行失败"],
      ["preparation_failed", "准备失败"],
      ["submission_failed", "任务提交失败"],
      ["handler_failed", "任务处理失败"],
      ["needs_reconcile", "等待结果确认"],
      ["interrupted", "执行中断"],
      ["preflight_target", "检查一体机"]
    ] as const;
    const tasks = stages.map(([stage], index) => ({
      id: `stage-${index}`,
      projectId: "project",
      domainType: "aio",
      operationType: "full_upgrade",
      name: `阶段测试 ${index + 1}`,
      state: "running" as const,
      stage,
      progress: 50,
      targetCount: 1,
      completedCount: 0,
      updatedAt: "2026-09-05 17:38:03",
      cancellable: true
    }));
    const wrapper = mount(
      { render: () => h(NMessageProvider, () => h(ActivityPanel)) },
      { global: { plugins: [pinia] } }
    );
    await flushPromises();
    activity.tasks = tasks;
    await flushPromises();

    for (const [stage, label] of stages) {
      expect(wrapper.text()).toContain(label);
      expect(wrapper.text()).not.toContain(stage);
    }
    wrapper.unmount();
  });

  it("asks the user before a finalization retry forcefully takes over another task", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const activity = useActivityStore();
    activity.openPanel("logs");
    const wrapper = mount(
      { render: () => h(NMessageProvider, () => h(ActivityPanel)) },
      { global: { plugins: [pinia] } }
    );
    await flushPromises();
    const task: ActivityTask = {
      id: "finalization-task",
      projectId: "project",
      domainType: "aio",
      operationType: "full_upgrade",
      name: "整包升级",
      state: "finalizing_failed",
      stage: "finalizing",
      progress: 100,
      targetCount: 1,
      completedCount: 1,
      updatedAt: "2026-09-06 15:00:00",
      cancellable: false
    };
    activity.tasks = [task];
    activity.selectedTaskId = task.id;
    const retry = vi
      .spyOn(activity, "retrySelectedFinalization")
      .mockResolvedValueOnce({
        task,
        takeoverRequired: true,
        message: "检测到其他电脑占用：电脑=现场电脑，任务=operation-b"
      })
      .mockResolvedValueOnce({
        task: { ...task, state: "succeeded" },
        takeoverRequired: false,
        message: "已强制接管并补写部署结果"
      });
    const confirm = vi.fn(() => true);
    vi.stubGlobal("confirm", confirm);
    await flushPromises();

    await wrapper.get(".log-scope .n-button").trigger("click");
    await flushPromises();

    expect(confirm).toHaveBeenCalledWith(expect.stringContaining("现场电脑"));
    expect(retry).toHaveBeenNthCalledWith(1, false);
    expect(retry).toHaveBeenNthCalledWith(2, true);
    vi.unstubAllGlobals();
    wrapper.unmount();
  });
});
