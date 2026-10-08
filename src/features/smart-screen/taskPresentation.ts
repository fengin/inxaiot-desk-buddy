import type { TaskPresentation } from "@/shared/model/taskPresentation";

export const screenTaskPresentation: TaskPresentation = {
  stages: { screen_operation: "设备操作", screen_preflight: "操作检查", screen_save_result: "保存操作结果" },
  canRetryResult: () => true
};
