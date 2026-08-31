import {
  cancelLocalTask,
  listLocalTasks,
  listTaskLogs,
  listenTaskEvents
} from "@/shared/api/activity";
import type { ActivityAdapter } from "@/shared/api/activityAdapter";

export class RealActivityAdapter implements ActivityAdapter {
  readonly real = true;
  listTasks(projectId: string, limit: number) { return listLocalTasks(projectId, limit); }
  listLogs(taskId: string, levels: Parameters<ActivityAdapter["listLogs"]>[1], keyword: string | null, offset: number, limit: number, newestFirst: boolean) {
    return listTaskLogs(taskId, levels, keyword, offset, limit, newestFirst);
  }
  cancelTask(taskId: string) { return cancelLocalTask(taskId); }
  listen(handler: Parameters<ActivityAdapter["listen"]>[0]) { return listenTaskEvents(handler); }
}
