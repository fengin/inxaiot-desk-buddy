import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  ActivityLogPage,
  ActivityTask,
  TaskEventPayload
} from "@/shared/model/activity";

export const isTauriRuntime = () => typeof window.__TAURI_INTERNALS__ !== "undefined";

export async function listLocalTasks(localProjectId: string, limit = 100) {
  return invoke<ActivityTask[]>("list_local_tasks", { localProjectId, limit });
}

export async function listTaskLogs(
  taskId: string,
  levels: string[],
  keyword: string | null,
  offset: number,
  limit: number,
  newestFirst = true
) {
  return invoke<ActivityLogPage>("list_task_logs", {
    taskId,
    levels,
    keyword,
    offset,
    limit,
    newestFirst
  });
}

export async function cancelLocalTask(taskId: string) {
  return invoke<ActivityTask>("cancel_local_task", { taskId });
}

export async function clearFinishedLocalTasks(localProjectId: string) {
  return invoke<number>("clear_finished_local_tasks", { localProjectId });
}

export async function clearTaskLogs(taskId: string) {
  return invoke<void>("clear_task_logs", { taskId });
}

export async function listenTaskEvents(
  handler: (payload: TaskEventPayload) => void
): Promise<UnlistenFn> {
  return listen<TaskEventPayload>("task-event", (event) => handler(event.payload));
}
