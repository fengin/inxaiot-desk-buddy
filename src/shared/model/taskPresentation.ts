import type { ActivityTask } from "./activity";

export interface TaskPresentation {
  stages: Record<string, string>;
  canRetryResult?: (task: ActivityTask) => boolean;
}

const presentations = new Map<string, TaskPresentation>();

export function registerTaskPresentation(domain: string, presentation: TaskPresentation): () => void {
  presentations.set(domain, presentation);
  return () => { if (presentations.get(domain) === presentation) presentations.delete(domain); };
}

export function taskStageLabel(task: ActivityTask): string {
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

export function canRetryTaskResult(task: ActivityTask | undefined): boolean {
  return Boolean(task?.state === "finalizing_failed" && presentations.get(task.domainType)?.canRetryResult?.(task));
}
