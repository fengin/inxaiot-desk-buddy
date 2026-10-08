import type { ActivityAdapter } from "@/shared/api/activityAdapter";
import { demoLogs, demoTasks } from "@/shared/fixtures/demoData";
import type { ActivityLogEntry, ActivityTask, TaskEventPayload } from "@/shared/model/activity";
import type { ScreenAdapter } from "@/shared/api/screenAdapter";
import { screenActionLabel } from "@/shared/model/screen";
import type { ScreenTask } from "@/shared/model/screen";

function screenActivity(task: ScreenTask): ActivityTask {
  const completed = task.targets.filter((t) => !["queued", "running"].includes(t.state)).length;
  return {
    id: task.id, projectId: task.projectId, domainType: "screen", operationType: task.action,
    name: `${screenActionLabel(task.action)} · 原型`, state: task.state === "needs_review" ? "interrupted" : task.state,
    stage: task.state === "needs_review" ? "等待结果核实" : "模拟屏端操作", targetCount: task.targets.length,
    completedCount: completed, progress: task.targets.length ? Math.round(task.targets.reduce((sum, t) => sum + (t.state === "cancelled" ? 100 : t.progress), 0) / task.targets.length) : 0,
    updatedAt: task.updatedAt, cancellable: task.state === "running"
  };
}

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
  private readonly screenProjects = new Map<string, string>();
  private readonly hiddenScreenTasks = new Set<string>();
  private readonly clearedScreenLogs = new Set<string>();
  constructor(private readonly screen?: ScreenAdapter) {}

  async listTasks(projectId: string, limit: number): Promise<ActivityTask[]> {
    const screenTasks = this.screen ? (await this.screen.load(projectId)).tasks.map(screenActivity) : [];
    for (const task of screenTasks) this.screenProjects.set(task.id, projectId);
    return structuredClone([...screenTasks.filter((task) => !this.hiddenScreenTasks.has(task.id)), ...this.tasks.filter((task) => task.projectId === projectId)].slice(0, limit))
      .map((task) => ({ ...task, clearable: !(task.operationType === "deployment_preflight" && task.state === "succeeded") }));
  }
  async listLogs(taskId: string, levels: Parameters<ActivityAdapter["listLogs"]>[1], keyword: string | null, offset: number, limit: number) {
    const normalized = keyword?.toLocaleLowerCase();
    const screenProject = this.screenProjects.get(taskId);
    if (screenProject && this.screen) {
      const task = (await this.screen.load(screenProject)).tasks.find((t) => t.id === taskId);
      const entries: ActivityLogEntry[] = this.clearedScreenLogs.has(taskId) ? [] : (task?.logs ?? []).map((entry, index) => ({
        id: `${taskId}-${index}`, taskId, sequence: index, timestamp: entry.time, level: entry.level, source: "智能屏原型", message: entry.message
      }));
      const filtered = entries.filter((e) => (!levels.length || levels.includes(e.level)) && (!normalized || e.message.toLocaleLowerCase().includes(normalized)));
      return { items: filtered.slice(offset, offset + limit), nextOffset: Math.min(filtered.length, offset + limit), hasMore: offset + limit < filtered.length };
    }
    const filtered = this.logs
      .filter((entry) => entry.taskId === taskId)
      .filter((entry) => !levels.length || levels.includes(entry.level))
      .filter((entry) => !normalized || `${entry.source} ${entry.message}`.toLocaleLowerCase().includes(normalized));
    return { items: structuredClone(filtered.slice(offset, offset + limit)), nextOffset: filtered.length, hasMore: false };
  }
  async cancelTask(taskId: string) {
    const projectId = this.screenProjects.get(taskId);
    if (projectId && this.screen) {
      await this.screen.cancel(projectId, taskId);
      return screenActivity((await this.screen.load(projectId)).tasks.find((task) => task.id === taskId)!);
    }
    const task = this.tasks.find((item) => item.id === taskId);
    if (!task) throw new Error(`Fixture 任务不存在：${taskId}`);
    return structuredClone(task);
  }
  async retryFinalization(taskId: string) {
    const task = this.tasks.find((item) => item.id === taskId);
    if (!task) throw new Error(`Fixture 任务不存在：${taskId}`);
    return {
      task: structuredClone(task),
      takeoverRequired: false,
      message: "Fixture 部署结果已补写完成"
    };
  }
  async clearFinishedTasks(projectId: string) {
    const terminal = new Set(["cancelled", "succeeded", "partially_succeeded", "failed", "interrupted"]);
    const clearedIds = new Set(
      this.tasks.filter((task) => task.projectId === projectId && terminal.has(task.state)).map((task) => task.id)
    );
    this.tasks = this.tasks.filter((task) => !clearedIds.has(task.id));
    let screenCount = 0;
    if (this.screen) for (const task of (await this.screen.load(projectId)).tasks) {
      if (terminal.has(task.state) && !this.hiddenScreenTasks.has(task.id)) { this.hiddenScreenTasks.add(task.id); screenCount++; }
    }
    return clearedIds.size + screenCount;
  }
  async clearTaskLogs(taskId: string) {
    if (this.screenProjects.has(taskId)) this.clearedScreenLogs.add(taskId);
    this.logs = this.logs.filter((entry) => entry.taskId !== taskId);
  }
  async listen(handler: (event: TaskEventPayload) => void) {
    const listener = (event: TaskEventPayload) => {
      this.applyTaskEvent(event);
      handler(event);
    };
    taskEventListeners.add(listener);
    const offScreen = this.screen?.subscribe((projectId, task) => {
      if (!task) return;
      this.screenProjects.set(task.id, projectId);
      const projected = screenActivity(task);
      handler({
        eventId: `${task.id}-${task.updatedAt}-${task.logs.length}`, localTaskId: task.id, localProjectId: projectId, domainType: "screen",
        sequence: Date.now(), stage: projected.stage, status: projected.state, progressCurrent: projected.completedCount, progressTotal: projected.targetCount,
        level: "info", messageCode: "SCREEN_PROTOTYPE_CHANGED", messageParams: {}, timestamp: task.updatedAt
      });
    });
    return () => { taskEventListeners.delete(listener); offScreen?.(); };
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
