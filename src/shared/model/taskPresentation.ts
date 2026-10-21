import type { ActivityTask } from "./activity";

export interface TaskPresentation {
  stages: Record<string, string>;
  canRetryResult?: (task: ActivityTask) => boolean;
  labels?: (task: ActivityTask) => Partial<TaskDisplayLabels> | undefined;
}

export interface TaskDisplayLabels {
  name: string;
  state: string;
  stage: string;
  tone: "success" | "warning" | "error" | "info";
}

const presentations = new Map<string, TaskPresentation>();

export function registerTaskPresentation(domain: string, presentation: TaskPresentation): () => void {
  presentations.set(domain, presentation);
  return () => { if (presentations.get(domain) === presentation) presentations.delete(domain); };
}

function defaultStageLabel(task: ActivityTask): string {
  const completed: Partial<Record<ActivityTask["state"], string>> = {
    queued: "排队中", failed: "执行失败", cancelled: "已取消", interrupted: "已中断",
    succeeded: "已完成", partially_succeeded: "已完成", finalizing_failed: "等待补写结果"
  };
  if (completed[task.state]) return completed[task.state]!;
  const common: Record<string, string> = {
    draft: "准备任务", checking: "检查中", check_failed: "检查失败", ready: "等待执行",
    finalizing: "保存执行结果", cancelling: "正在取消", needs_reconcile: "等待结果确认"
  };
  return presentations.get(task.domainType)?.stages[task.stage] ?? common[task.stage]
    ?? (/[㐀-鿿]/u.test(task.stage) ? task.stage : "处理中");
}

export function taskDisplayLabels(task: ActivityTask): TaskDisplayLabels {
  const labels = presentations.get(task.domainType)?.labels?.(task);
  const states: Record<ActivityTask["state"], string> = {
    draft: "草稿", checking: "检查中", check_failed: "检查失败", ready: "等待入队",
    queued: "等待执行", running: "执行中", cancelling: "取消中", cancelled: "已取消",
    succeeded: "已成功", partially_succeeded: "部分成功", failed: "已失败",
    interrupted: "已中断", finalizing_failed: "结果待补写"
  };
  const tone = task.state === "succeeded" ? "success"
    : task.state === "partially_succeeded" || task.state === "finalizing_failed" ? "warning"
      : task.state === "failed" || task.state === "interrupted" ? "error" : "info";
  return { name: task.name, state: states[task.state], stage: defaultStageLabel(task), tone, ...labels };
}

export function taskStageLabel(task: ActivityTask): string {
  return taskDisplayLabels(task).stage;
}

export function canRetryTaskResult(task: ActivityTask | undefined): boolean {
  return Boolean(task?.state === "finalizing_failed" && presentations.get(task.domainType)?.canRetryResult?.(task));
}
