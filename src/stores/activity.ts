import { defineStore } from "pinia";
import { computed, ref } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useActivityAdapter } from "@/shared/api/activityAdapter";
import type {
  ActivityLogEntry,
  ActivityLogLevel,
  ActivityTask,
  TaskEventPayload
} from "@/shared/model/activity";

const activeStates = new Set(["checking", "queued", "running", "cancelling", "finalizing_failed"]);
const terminalStates = new Set(["cancelled", "succeeded", "partially_succeeded", "failed", "interrupted"]);

export const useActivityStore = defineStore("activity", () => {
  const tasks = ref<ActivityTask[]>([]);
  const logs = ref<ActivityLogEntry[]>([]);
  const selectedTaskId = ref("");
  const projectId = ref("");
  const loading = ref(false);
  const finalizationRetrying = ref(false);
  const error = ref("");
  const logLevels = ref<ActivityLogLevel[]>([]);
  const logKeyword = ref("");
  const logOffset = ref(0);
  const logHasMore = ref(false);
  const started = ref(false);
  const panelOpen = ref(false);
  const panelTab = ref<"tasks" | "logs">("tasks");
  const lastEvent = ref<TaskEventPayload>();
  let unlisten: (() => void) | undefined;
  let refreshTimer: number | undefined;
  let taskRefreshRequest = 0;
  let logRefreshRequest = 0;

  const selectedTask = computed(
    () => tasks.value.find((task) => task.id === selectedTaskId.value) ?? tasks.value[0]
  );
  const activeTaskCount = computed(
    () => tasks.value.filter((task) => activeStates.has(task.state)).length
  );
  const canClearFinishedTasks = computed(() => tasks.value.some((task) =>
    terminalStates.has(task.state)
    && !(task.operationType === "deployment_preflight" && task.state === "succeeded")
  ));
  const canClearSelectedTaskLogs = computed(
    () => Boolean(selectedTask.value && terminalStates.has(selectedTask.value.state))
  );

  function openPanel(tab: "tasks" | "logs") {
    panelTab.value = tab;
    panelOpen.value = true;
  }

  async function refreshTasks(nextProjectId = projectId.value) {
    const request = ++taskRefreshRequest;
    if (!nextProjectId) {
      projectId.value = "";
      selectedTaskId.value = "";
      tasks.value = [];
      logs.value = [];
      logRefreshRequest += 1;
      loading.value = false;
      return;
    }
    if (projectId.value !== nextProjectId) {
      selectedTaskId.value = "";
      tasks.value = [];
      logs.value = [];
      logRefreshRequest += 1;
    }
    projectId.value = nextProjectId;
    loading.value = true;
    error.value = "";
    try {
      const nextTasks = await useActivityAdapter().listTasks(nextProjectId, 100);
      if (request !== taskRefreshRequest || projectId.value !== nextProjectId) return;
      tasks.value = nextTasks;
      if (!tasks.value.some((task) => task.id === selectedTaskId.value)) {
        selectedTaskId.value = tasks.value[0]?.id ?? "";
      }
      if (selectedTaskId.value) await refreshLogs();
      else logs.value = [];
    } catch (cause) {
      if (request === taskRefreshRequest && projectId.value === nextProjectId) {
        error.value = commandErrorText(cause, "任务数据读取失败");
      }
    } finally {
      if (request === taskRefreshRequest) loading.value = false;
    }
  }

  async function refreshLogs() {
    const request = ++logRefreshRequest;
    const taskId = selectedTaskId.value;
    if (!taskId) {
      logs.value = [];
      return;
    }
    try {
      const page = await useActivityAdapter().listLogs(
        taskId,
        logLevels.value,
        logKeyword.value.trim() || null,
        0,
        500,
        true
      );
      if (request !== logRefreshRequest || selectedTaskId.value !== taskId) return;
      logs.value = page.items;
      logOffset.value = page.nextOffset;
      logHasMore.value = page.hasMore;
    } catch (cause) {
      if (request === logRefreshRequest && selectedTaskId.value === taskId) {
        error.value = commandErrorText(cause, "任务日志读取失败");
      }
    }
  }

  async function loadOlderLogs() {
    if (!selectedTaskId.value || !logHasMore.value) return;
    try {
      const page = await useActivityAdapter().listLogs(
        selectedTaskId.value,
        logLevels.value,
        logKeyword.value.trim() || null,
        logOffset.value,
        500,
        true
      );
      logs.value = [...page.items, ...logs.value];
      logOffset.value = page.nextOffset;
      logHasMore.value = page.hasMore;
    } catch (cause) {
      error.value = commandErrorText(cause, "更早日志读取失败");
    }
  }

  async function selectTask(taskId: string) {
    selectedTaskId.value = taskId;
    await refreshLogs();
  }

  async function cancelSelectedTask() {
    const task = selectedTask.value;
    if (!task?.cancellable) return;
    try {
      const updated = await useActivityAdapter().cancelTask(task.id);
      const index = tasks.value.findIndex((item) => item.id === updated.id);
      if (index >= 0) tasks.value[index] = updated;
      await refreshLogs();
    } catch (cause) {
      error.value = commandErrorText(cause, "取消任务失败");
    }
  }

  async function retrySelectedFinalization(forceTakeover = false) {
    const task = selectedTask.value;
    if (task?.state !== "finalizing_failed" || finalizationRetrying.value) return null;
    finalizationRetrying.value = true;
    error.value = "";
    try {
      const result = await useActivityAdapter().retryFinalization(task.id, forceTakeover);
      const index = tasks.value.findIndex((item) => item.id === result.task.id);
      if (index >= 0) tasks.value[index] = result.task;
      await refreshLogs();
      return result;
    } catch (cause) {
      error.value = commandErrorText(cause, "补写部署结果失败");
      throw cause;
    } finally {
      finalizationRetrying.value = false;
    }
  }

  async function clearFinishedTasks() {
    if (!projectId.value || !canClearFinishedTasks.value) return 0;
    try {
      const cleared = await useActivityAdapter().clearFinishedTasks(projectId.value);
      await refreshTasks();
      return cleared;
    } catch (cause) {
      error.value = commandErrorText(cause, "清空任务记录失败");
      throw cause;
    }
  }

  async function clearSelectedTaskLogs() {
    const task = selectedTask.value;
    if (!task || !terminalStates.has(task.state)) return false;
    try {
      await useActivityAdapter().clearTaskLogs(task.id);
      logs.value = [];
      logOffset.value = 0;
      logHasMore.value = false;
      return true;
    } catch (cause) {
      error.value = commandErrorText(cause, "清空任务日志失败");
      throw cause;
    }
  }

  function scheduleRefresh() {
    if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
    refreshTimer = window.setTimeout(() => void refreshTasks(), 120);
  }

  async function start(nextProjectId: string) {
    if (started.value) {
      if (projectId.value !== nextProjectId) await refreshTasks(nextProjectId);
      return;
    }
    started.value = true;
    await refreshTasks(nextProjectId);
    unlisten = await useActivityAdapter().listen((event) => {
      lastEvent.value = event;
      scheduleRefresh();
    });
  }

  function dispose() {
    unlisten?.();
    unlisten = undefined;
    if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
    refreshTimer = undefined;
    taskRefreshRequest += 1;
    logRefreshRequest += 1;
    started.value = false;
  }

  return {
    tasks,
    logs,
    selectedTaskId,
    selectedTask,
    activeTaskCount,
    canClearFinishedTasks,
    canClearSelectedTaskLogs,
    loading,
    finalizationRetrying,
    error,
    logLevels,
    logKeyword,
    logHasMore,
    panelOpen,
    panelTab,
    lastEvent,
    openPanel,
    start,
    dispose,
    refreshTasks,
    refreshLogs,
    loadOlderLogs,
    selectTask,
    cancelSelectedTask,
    retrySelectedFinalization,
    clearFinishedTasks,
    clearSelectedTaskLogs
  };
});
