import type { ActivityAdapter } from "@/shared/api/activityAdapter";
import { demoLogs, demoTasks } from "@/shared/fixtures/demoData";
import type { ActivityLogEntry, ActivityTask, TaskEventPayload } from "@/shared/model/activity";

const taskEventListeners = new Set<(event: TaskEventPayload) => void>();

export function publishFixtureTaskEvent(event: TaskEventPayload) {
  for (const listener of taskEventListeners) listener(structuredClone(event));
}

const fixtureTasks: ActivityTask[] = demoTasks.map((task) => ({
  id: task.id,
  projectId: task.projectId,
  domainType: "aio",
  operationType: task.mode,
  name: task.name,
  state: task.state,
  stage: task.stage,
  progress: task.progress,
  targetCount: task.targetCount,
  completedCount: task.completedCount,
  updatedAt: task.updatedAt,
  cancellable: false
}));

const fixtureLogs: ActivityLogEntry[] = demoLogs.map((entry, sequence) => ({
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
  private tasks = structuredClone(fixtureTasks);
  private logs = structuredClone(fixtureLogs);
  private readonly completedPreflightTargets = new Map<string, Set<string>>();

  async listTasks(projectId: string, limit: number) {
    return structuredClone(this.tasks.filter((task) => task.projectId === projectId).slice(0, limit));
  }
  async listLogs(taskId: string, levels: Parameters<ActivityAdapter["listLogs"]>[1], keyword: string | null, offset: number, limit: number) {
    const normalized = keyword?.toLocaleLowerCase();
    const filtered = this.logs
      .filter((entry) => entry.taskId === taskId)
      .filter((entry) => !levels.length || levels.includes(entry.level))
      .filter((entry) => !normalized || `${entry.source} ${entry.message}`.toLocaleLowerCase().includes(normalized));
    return { items: structuredClone(filtered.slice(offset, offset + limit)), nextOffset: filtered.length, hasMore: false };
  }
  async cancelTask(taskId: string) {
    const task = this.tasks.find((item) => item.id === taskId);
    if (!task) throw new Error(`Fixture 任务不存在：${taskId}`);
    return structuredClone(task);
  }
  async clearFinishedTasks(projectId: string) {
    const terminal = new Set(["cancelled", "succeeded", "partially_succeeded", "failed", "interrupted"]);
    const clearedIds = new Set(
      this.tasks.filter((task) => task.projectId === projectId && terminal.has(task.state)).map((task) => task.id)
    );
    this.tasks = this.tasks.filter((task) => !clearedIds.has(task.id));
    return clearedIds.size;
  }
  async clearTaskLogs(taskId: string) {
    this.logs = this.logs.filter((entry) => entry.taskId !== taskId);
  }
  async listen(handler: (event: TaskEventPayload) => void) {
    const listener = (event: TaskEventPayload) => {
      this.applyTaskEvent(event);
      handler(event);
    };
    taskEventListeners.add(listener);
    return () => taskEventListeners.delete(listener);
  }

  private applyTaskEvent(event: TaskEventPayload) {
    if (event.messageParams.operationType === "service_inspection") {
      const task: ActivityTask = {
        id: event.localTaskId, projectId: event.localProjectId, domainType: event.domainType,
        operationType: "service_inspection", name: event.messageParams.taskName ?? "检查服务",
        state: event.status === "succeeded" ? "succeeded" : event.status === "failed" ? "failed" : "running",
        stage: event.stage, progress: event.status === "succeeded" ? 100 : 0, targetCount: 1,
        completedCount: ["succeeded", "failed"].includes(event.status) ? 1 : 0,
        updatedAt: event.timestamp, cancellable: false
      };
      const existing = this.tasks.find((item) => item.id === task.id);
      if (existing) Object.assign(existing, task);
      else this.tasks.unshift(task);
      if (event.message && !this.logs.some((entry) => entry.id === event.eventId)) this.logs.push({
        id: event.eventId, taskId: event.localTaskId, sequence: event.sequence, timestamp: event.timestamp,
        level: event.level.toUpperCase() as ActivityLogEntry["level"], source: event.resourceKey ?? "服务检查", message: event.message
      });
      return;
    }
    const total = Math.max(0, event.progressTotal ?? 0);
    const current = Math.max(0, Math.min(event.progressCurrent ?? 0, total || Number.MAX_SAFE_INTEGER));
    const state = event.messageCode === "PREFLIGHT_SUCCEEDED"
      ? "succeeded"
      : event.messageCode === "PREFLIGHT_FAILED"
        ? "failed"
        : "checking";
    const existing = this.tasks.find((task) => task.id === event.localTaskId);
    const declaredTargetCount = Number.parseInt(event.messageParams.targetCount ?? "", 10);
    const targetCount = Number.isFinite(declaredTargetCount) && declaredTargetCount >= 0
      ? declaredTargetCount
      : existing?.targetCount ?? total;
    const completedTargets = this.completedPreflightTargets.get(event.localTaskId) ?? new Set<string>();
    const targetFinished = [
      "PREFLIGHT_TARGET_RENDER_FINISHED",
      "PREFLIGHT_TARGET_RENDER_FAILED",
      "PREFLIGHT_TARGET_RENDER_SKIPPED"
    ].includes(event.messageCode)
      || (event.messageParams.deploymentMode === "service_upgrade"
        && ["PREFLIGHT_TARGET_RUNTIME_FINISHED", "PREFLIGHT_TARGET_RUNTIME_FAILED"].includes(event.messageCode));
    if (targetFinished && event.resourceKey) completedTargets.add(event.resourceKey);
    if (state !== "checking") {
      this.completedPreflightTargets.delete(event.localTaskId);
    } else {
      this.completedPreflightTargets.set(event.localTaskId, completedTargets);
    }
    const next: ActivityTask = {
      id: event.localTaskId,
      projectId: event.localProjectId,
      domainType: event.domainType,
      operationType: "deployment_preflight",
      name: event.messageParams.taskName ?? "部署执行条件检查",
      state,
      stage: event.stage,
      progress: total > 0 ? Math.min(100, Math.round(current * 100 / total)) : null,
      targetCount,
      completedCount: state === "succeeded" ? targetCount : completedTargets.size,
      updatedAt: event.timestamp,
      cancellable: false
    };
    if (existing) Object.assign(existing, next);
    else this.tasks.unshift(next);
    if (event.message && !this.logs.some((entry) => entry.id === event.eventId)) {
      this.logs.push({
        id: event.eventId,
        taskId: event.localTaskId,
        sequence: event.sequence,
        timestamp: event.timestamp,
        level: event.level.toUpperCase() as ActivityLogEntry["level"],
        source: event.resourceKey ?? "部署检查",
        message: event.message
      });
    }
  }
}
