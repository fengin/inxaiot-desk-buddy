import type { ActivityAdapter } from "@/shared/api/activityAdapter";
import { demoLogs, demoTasks } from "@/shared/fixtures/demoData";
import type { ActivityLogEntry, ActivityTask } from "@/shared/model/activity";

const tasks: ActivityTask[] = demoTasks.map((task) => ({
  id: task.id,
  projectId: task.projectId,
  name: task.name,
  state: task.state,
  stage: task.stage,
  progress: task.progress,
  targetCount: task.targetCount,
  completedCount: task.completedCount,
  updatedAt: task.updatedAt,
  cancellable: false
}));

const logs: ActivityLogEntry[] = demoLogs.map((entry, sequence) => ({
  id: entry.id,
  taskId: entry.taskId,
  sequence,
  timestamp: entry.time,
  level: entry.level,
  source: entry.source,
  message: entry.message
}));

export class FixtureActivityAdapter implements ActivityAdapter {
  readonly real = false;
  async listTasks(projectId: string, limit: number) {
    return structuredClone(tasks.filter((task) => task.projectId === projectId).slice(0, limit));
  }
  async listLogs(taskId: string, levels: Parameters<ActivityAdapter["listLogs"]>[1], keyword: string | null, offset: number, limit: number) {
    const normalized = keyword?.toLocaleLowerCase();
    const filtered = logs
      .filter((entry) => entry.taskId === taskId)
      .filter((entry) => !levels.length || levels.includes(entry.level))
      .filter((entry) => !normalized || `${entry.source} ${entry.message}`.toLocaleLowerCase().includes(normalized));
    return { items: structuredClone(filtered.slice(offset, offset + limit)), nextOffset: filtered.length, hasMore: false };
  }
  async cancelTask(taskId: string) {
    const task = tasks.find((item) => item.id === taskId);
    if (!task) throw new Error(`Fixture 任务不存在：${taskId}`);
    return structuredClone(task);
  }
  async listen() { return () => undefined; }
}
