import { defineStore } from "pinia";
import { ref } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useOperationsAdapter } from "@/shared/api/operationsAdapter";
import type {
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
  const preflight = ref<DeploymentPreflightReport>();
  const submission = ref<DeploymentTaskSubmission>();
  const history = ref<OperationHistoryPage>(emptyHistory());
  const historyDetail = ref<OperationHistoryDetail>();
  const taskLoading = ref(false);
  const historyLoading = ref(false);
  const error = ref("");

  async function runPreflight(projectId: string, plan: DeploymentPlanInput) {
    taskLoading.value = true;
    error.value = "";
    try {
      preflight.value = await useOperationsAdapter().preflight(projectId, plan);
      return preflight.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "部署预检失败");
      throw cause;
    } finally {
      taskLoading.value = false;
    }
  }

  async function submit(projectId: string, plan: DeploymentPlanInput) {
    taskLoading.value = true;
    error.value = "";
    try {
      submission.value = await useOperationsAdapter().submit(projectId, plan);
      currentTask.value = await useOperationsAdapter().getTask(
        projectId,
        submission.value.taskId
      );
      return submission.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "部署任务提交失败");
      throw cause;
    } finally {
      taskLoading.value = false;
    }
  }

  async function loadTask(projectId: string, taskId: string) {
    taskLoading.value = true;
    error.value = "";
    try {
      currentTask.value = await useOperationsAdapter().getTask(projectId, taskId);
      return currentTask.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "部署任务读取失败");
      throw cause;
    } finally {
      taskLoading.value = false;
    }
  }

  async function loadHistory(
    projectId: string,
    query: OperationHistoryQuery = { page: 1, pageSize: 20 }
  ) {
    historyLoading.value = true;
    error.value = "";
    try {
      history.value = await useOperationsAdapter().listHistory(projectId, query);
      return history.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "部署历史读取失败");
      throw cause;
    } finally {
      historyLoading.value = false;
    }
  }

  async function loadHistoryDetail(projectId: string, operationId: string) {
    historyLoading.value = true;
    error.value = "";
    try {
      historyDetail.value = await useOperationsAdapter().getHistoryDetail(
        projectId,
        operationId
      );
      return historyDetail.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "部署历史详情读取失败");
      throw cause;
    } finally {
      historyLoading.value = false;
    }
  }

  function clearTask() {
    currentTask.value = undefined;
    submission.value = undefined;
  }

  function clearPreflight() {
    preflight.value = undefined;
  }

  function clearHistoryDetail() {
    historyDetail.value = undefined;
  }

  return {
    currentTask,
    preflight,
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
    clearHistoryDetail
  };
});
