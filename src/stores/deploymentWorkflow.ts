import { defineStore } from "pinia";
import { ref } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useOperationsAdapter } from "@/shared/api/operationsAdapter";
import type {
  DeploymentExecutionSnapshot,
  DeploymentPreflightReport,
  DeploymentTaskSubmission,
  DeploymentTaskView,
  OperationHistoryDetail,
  OperationHistoryPage,
  OperationHistoryQuery
} from "@/shared/model/deploymentWorkflow";
import type { DeploymentPlanInput } from "@/shared/model/release";

const emptyHistory = (): OperationHistoryPage => ({
  items: [],
  total: 0,
  page: 1,
  pageSize: 20
});

export const useDeploymentWorkflowStore = defineStore("deployment-workflow", () => {
  const currentTask = ref<DeploymentTaskView>();
  const currentTaskProjectId = ref<string>();
  const preflight = ref<DeploymentPreflightReport>();
  const preflightProjectId = ref<string>();
  const preflightTaskId = ref<string>();
  const submission = ref<DeploymentTaskSubmission>();
  const history = ref<OperationHistoryPage>(emptyHistory());
  const historyDetail = ref<OperationHistoryDetail>();
  const taskLoading = ref(false);
  const historyLoading = ref(false);
  const error = ref("");
  let taskRequest = 0;
  let preflightRequest = 0;
  let historyRequest = 0;
  let historyDetailRequest = 0;
  let pendingTaskRequests = 0;
  let pendingHistoryRequests = 0;

  function startTaskLoading() {
    pendingTaskRequests += 1;
    taskLoading.value = true;
  }

  function stopTaskLoading() {
    pendingTaskRequests = Math.max(0, pendingTaskRequests - 1);
    taskLoading.value = pendingTaskRequests > 0;
  }

  function startHistoryLoading() {
    pendingHistoryRequests += 1;
    historyLoading.value = true;
  }

  function stopHistoryLoading() {
    pendingHistoryRequests = Math.max(0, pendingHistoryRequests - 1);
    historyLoading.value = pendingHistoryRequests > 0;
  }

  function bindProject(projectId?: string) {
    preflightRequest += 1;
    historyRequest += 1;
    historyDetailRequest += 1;
    preflight.value = undefined;
    preflightProjectId.value = projectId;
    preflightTaskId.value = undefined;
    history.value = emptyHistory();
    historyDetail.value = undefined;
    error.value = "";
  }

  async function runPreflight(
    projectId: string,
    taskId: string,
    plan: DeploymentPlanInput
  ) {
    const request = ++preflightRequest;
    preflight.value = undefined;
    preflightTaskId.value = undefined;
    startTaskLoading();
    error.value = "";
    try {
      const result = await useOperationsAdapter().preflight(projectId, taskId, plan);
      if (request === preflightRequest) {
        preflight.value = result;
        preflightProjectId.value = projectId;
        preflightTaskId.value = taskId;
      }
      return result;
    } catch (cause) {
      if (request === preflightRequest) {
        error.value = commandErrorText(cause, "部署预检失败");
      }
      throw cause;
    } finally {
      stopTaskLoading();
    }
  }

  async function submit(
    projectId: string,
    checkedTaskId: string,
    executionSnapshot: DeploymentExecutionSnapshot
  ) {
    const request = ++taskRequest;
    startTaskLoading();
    error.value = "";
    try {
      const submitted = await useOperationsAdapter().submit(
        projectId,
        checkedTaskId,
        executionSnapshot
      );
      if (request === taskRequest) {
        submission.value = submitted;
      }
      try {
        const task = await useOperationsAdapter().getTask(
          projectId,
          submitted.taskId
        );
        if (request === taskRequest) {
          currentTask.value = task;
          currentTaskProjectId.value = projectId;
        }
      } catch (cause) {
        if (request === taskRequest) {
          error.value = commandErrorText(cause, "部署任务已提交，但任务详情读取失败");
        }
      }
      return submitted;
    } catch (cause) {
      if (request === taskRequest) {
        error.value = commandErrorText(cause, "部署任务提交失败");
      }
      throw cause;
    } finally {
      stopTaskLoading();
    }
  }

  async function loadTask(projectId: string, taskId: string) {
    const request = ++taskRequest;
    startTaskLoading();
    error.value = "";
    try {
      const task = await useOperationsAdapter().getTask(projectId, taskId);
      if (request === taskRequest) {
        currentTask.value = task;
        currentTaskProjectId.value = projectId;
      }
      return task;
    } catch (cause) {
      if (request === taskRequest) {
        error.value = commandErrorText(cause, "部署任务读取失败");
      }
      throw cause;
    } finally {
      stopTaskLoading();
    }
  }

  async function loadHistory(
    projectId: string,
    query: OperationHistoryQuery = { page: 1, pageSize: 20 }
  ) {
    const request = ++historyRequest;
    startHistoryLoading();
    error.value = "";
    try {
      const result = await useOperationsAdapter().listHistory(projectId, query);
      if (request === historyRequest) history.value = result;
      return result;
    } catch (cause) {
      if (request === historyRequest) {
        error.value = commandErrorText(cause, "部署历史读取失败");
      }
      throw cause;
    } finally {
      stopHistoryLoading();
    }
  }

  async function loadHistoryDetail(projectId: string, operationId: string) {
    const request = ++historyDetailRequest;
    startHistoryLoading();
    error.value = "";
    try {
      const result = await useOperationsAdapter().getHistoryDetail(
        projectId,
        operationId
      );
      if (request === historyDetailRequest) historyDetail.value = result;
      return result;
    } catch (cause) {
      if (request === historyDetailRequest) {
        error.value = commandErrorText(cause, "部署历史详情读取失败");
      }
      throw cause;
    } finally {
      stopHistoryLoading();
    }
  }

  function clearTask() {
    taskRequest += 1;
    currentTask.value = undefined;
    currentTaskProjectId.value = undefined;
    submission.value = undefined;
  }

  function clearPreflight() {
    preflightRequest += 1;
    preflight.value = undefined;
    preflightProjectId.value = undefined;
    preflightTaskId.value = undefined;
  }

  function clearHistoryDetail() {
    historyDetailRequest += 1;
    historyDetail.value = undefined;
  }

  return {
    currentTask,
    currentTaskProjectId,
    preflight,
    preflightProjectId,
    preflightTaskId,
    submission,
    history,
    historyDetail,
    taskLoading,
    historyLoading,
    error,
    runPreflight,
    submit,
    loadTask,
    loadHistory,
    loadHistoryDetail,
    clearTask,
    clearPreflight,
    clearHistoryDetail,
    bindProject
  };
});
