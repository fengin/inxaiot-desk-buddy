export interface ScreenHomeState {
  protocolVersion: number;
  packageName: string;
  deviceOwner: boolean;
  persistentPreferred: boolean;
  resolvedHome: string | null;
}

export interface ScreenHomeEvidence {
  component: string;
  targetEnabled: boolean;
  before?: ScreenHomeState | null;
  after?: ScreenHomeState | null;
  save: "pending" | "unknown" | "applied" | "unchanged" | "rejected" | "verified";
  readback: "pending" | "unknown" | "succeeded" | "failed" | "not_required";
  activation?: "pending" | "unknown" | "activated" | "unchanged" | "verified" | "not_required";
  runtimeReadback?: "pending" | "unknown" | "succeeded" | "failed" | "not_required";
  runtimeBefore?: ScreenHomeRuntime | null;
  runtimeAfter?: ScreenHomeRuntime | null;
}

export interface ScreenHomeRuntime {
  homeTask: boolean;
  resumed: boolean;
  pid: number | null;
  taskId: number | null;
  component: string | null;
}

export function screenHomeStageLabel(stage: "save" | "readback" | "activation" | "runtimeReadback", value?: string) {
  if (stage === "save" && value === "applied") return "已提交";
  if (stage === "save" && value === "unchanged") return "无需修改";
  if (stage === "save" && value === "verified") return "已核实";
  if (stage === "activation") {
    if (value === undefined) return "未记录";
    if (value === "activated") return "已触发";
    if (value === "unchanged") return "已在主屏";
    if (value === "verified") return "已核实";
  }
  if (stage === "runtimeReadback" && value === undefined) return "未记录";
  return ({ pending: "等待处理", unknown: "待核实", rejected: "未保存", succeeded: "已确认", failed: "未通过", not_required: "无需执行" } as Record<string, string>)[value ?? "pending"] ?? "待核实";
}

export function screenHomeStagesSummary(value: ScreenHomeEvidence) {
  return `主屏保存：${screenHomeStageLabel("save", value.save)} · 主屏回读：${screenHomeStageLabel("readback", value.readback)}`
    + (value.targetEnabled ? ` · 激活主屏：${screenHomeStageLabel("activation", value.activation)} · 运行核验：${screenHomeStageLabel("runtimeReadback", value.runtimeReadback)}` : "");
}

export function screenHomeSummary(value?: ScreenHomeEvidence) {
  if (!value) return "未取得主屏状态";
  if (typeof value.targetEnabled !== "boolean") return "主屏目标未记录";
  const after = value.after;
  const policyConfirmed = value.save !== "rejected" && value.readback === "succeeded" && after?.deviceOwner && (value.targetEnabled ? after.persistentPreferred && after.resolvedHome === value.component : !after.persistentPreferred);
  if (value.targetEnabled && policyConfirmed && value.activation === undefined && value.runtimeReadback === undefined) return "固定小新为主屏 · 规则已保存，激活未核验";
  const runtime = value.runtimeAfter;
  const active = value.runtimeReadback === "succeeded" && ["activated", "unchanged", "verified"].includes(value.activation ?? "") && runtime?.homeTask && runtime.resumed && Number.isInteger(runtime.pid) && (runtime.pid ?? 0) > 0 && Number.isInteger(runtime.taskId) && (runtime.taskId ?? -1) >= 0 && runtime.component === value.component;
  const confirmed = policyConfirmed && (!value.targetEnabled || active);
  return `${value.targetEnabled ? "固定小新为主屏" : "解除主屏固定"} · ${confirmed ? "已确认" : "尚未确认"}`;
}
