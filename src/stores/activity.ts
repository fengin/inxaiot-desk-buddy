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

const activeStates = new Set(["queued", "running", "cancelling", "finalizing_failed"]);

export const useActivityStore = defineStore("activity", () => {
  const tasks = ref<ActivityTask[]>([]);
  const logs = ref<ActivityLogEntry[]>([]);
  const selectedTaskId = ref("");
  const projectId = ref("");
  const loading = ref(false);
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

  const selectedTask = computed(
    () => tasks.value.find((task) => task.id === selectedTaskId.value) ?? tasks.value[0]
  );
  const activeTaskCount = computed(
    () => tasks.value.filter((task) => activeStates.has(task.state)).length
  );

  function openPanel(tab: "tasks" | "logs") {
    panelTab.value = tab;
    panelOpen.value = true;
  }

  async function refreshTasks(nextProjectId = projectId.value) {
    if (!nextProjectId) {
      tasks.value = [];
      logs.value = [];
      return;
    }
    projectId.value = nextProjectId;
    loading.value = true;
    error.value = "";
    try {
      tasks.value = await useActivityAdapter().listTasks(nextProjectId, 100);
      if (!tasks.value.some((task) => task.id === selectedTaskId.value)) {
        selectedTaskId.value = tasks.value[0]?.id ?? "";
      }
      if (selectedTaskId.value) await refreshLogs();
      else logs.value = [];
    } catch (cause) {
      error.value = commandErrorText(cause, "任务数据读取失败");
    } finally {
      loading.value = false;
    }
  }

  async function refreshLogs() {
    if (!selectedTaskId.value) {
      logs.value = [];
      return;
    }
    try {
      const page = await useActivityAdapter().listLogs(
        selectedTaskId.value,
        logLevels.value,
        logKeyword.value.trim() || null,
        0,
        500,
        true
      );
      logs.value = page.items;
      logOffset.value = page.nextOffset;
      logHasMore.value = page.hasMore;
    } catch (cause) {
      error.value = commandErrorText(cause, "任务日志读取失败");
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
    started.value = false;
  }

  return {
    tasks,
    logs,
    selectedTaskId,
    selectedTask,
    activeTaskCount,
    loading,
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
    cancelSelectedTask
  };
});
