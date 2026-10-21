import type { TaskPresentation } from "@/shared/model/taskPresentation";

export const screenTaskPresentation: TaskPresentation = {
  stages: { screen_operation: "设备操作", screen_preflight: "操作检查", screen_save_result: "保存操作结果" },
  canRetryResult: () => true,
  labels: (task) => {
    if (task.operationType === "screen_registration_preview") {
      const checked = task.state === "succeeded" || task.state === "partially_succeeded";
      const failed = ["failed", "check_failed", "interrupted"].includes(task.state);
      if (task.state === "finalizing_failed") return { name: "注册/更新检查", state: "检查结果待核实", stage: "检查结果待核实，尚未提交注册", tone: "warning" };
      return {
        name: "注册/更新检查",
        state: checked ? "检查完成" : failed ? "检查未通过" : task.state === "cancelled" ? "检查已取消" : "检查中",
        stage: checked ? "检查完成，尚未提交注册" : failed ? "检查未通过，尚未提交注册" : "尚未提交注册或更新",
        tone: failed ? "warning" : "info"
      };
    }
    if (task.operationType !== "register") return;
    if (task.state === "succeeded") return { state: "注册/更新成功", stage: "平台写入已确认" };
    if (task.state === "partially_succeeded") return { state: "部分注册/更新成功", stage: "部分目标已写入平台，请核对其余结果" };
    if (task.state === "finalizing_failed") return { state: "结果待核实", stage: "平台写入或操作记录待核实，请勿重复提交" };
    if (task.state === "failed") return { state: "注册/更新未完成", stage: "请核对逐屏结果，重新检查后再提交" };
    if (task.state === "queued") return { stage: "等待提交平台" };
    if (task.state === "running") return { stage: "正在提交并确认平台写入" };
  }
};
