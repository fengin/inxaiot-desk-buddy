import type {
  ActivityLogLevel,
  ActivityLogPage,
  ActivityTask,
  TaskEventPayload
} from "@/shared/model/activity";

export interface ActivityAdapter {
  readonly real: boolean;
  listTasks(projectId: string, limit: number): Promise<ActivityTask[]>;
  listLogs(
    taskId: string,
    levels: ActivityLogLevel[],
    keyword: string | null,
    offset: number,
    limit: number,
    newestFirst: boolean
  ): Promise<ActivityLogPage>;
  cancelTask(taskId: string): Promise<ActivityTask>;
  listen(handler: (event: TaskEventPayload) => void): Promise<() => void>;
}

let adapter: ActivityAdapter | undefined;

export function configureActivityAdapter(next: ActivityAdapter) {
  adapter = next;
}

export function useActivityAdapter() {
  if (!adapter) throw new Error("Activity Adapter 尚未初始化");
  return adapter;
}
