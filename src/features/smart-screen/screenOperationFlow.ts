import type { ScreenAction } from "@/shared/model/screen";

export interface ScreenOperationStep { title: string; hint: string }
export interface ScreenOperationFlow { steps: ScreenOperationStep[]; executeLabel: string; readOnly: boolean }
const inspection = (result: string): ScreenOperationFlow => ({
  steps: [{ title: "选屏检查", hint: "选择检查范围" }, { title: result, hint: "查看逐台结果" }], executeLabel: "开始检查", readOnly: true
});
export const screenOperationFlows: Record<ScreenAction, ScreenOperationFlow> = {
  ntp: { steps: [{ title: "选择智能屏", hint: "读取现有授时设置" }, { title: "NTP 设置", hint: "核对地址和生效方式" }, { title: "设置结果", hint: "保存、生效与授时验证" }], executeLabel: "确认设置", readOnly: false },
  app_config: { steps: [{ title: "选择智能屏", hint: "读取屏端当前配置" }, { title: "配置修改", hint: "只修改明确选择的字段" }, { title: "修改结果", hint: "保存、重启和回读" }], executeLabel: "确认修改", readOnly: false },
  register: { steps: [{ title: "选择设备与资料", hint: "补齐本次提交资料" }, { title: "检查并预览", hint: "逐屏核对登记与差异" }, { title: "提交结果", hint: "查看平台回读结果" }], executeLabel: "确认提交", readOnly: false },
  ping: inspection("在线结果"), inspect: inspection("设备检查结果"), mac: inspection("MAC 核对结果"), diagnostics: inspection("诊断结果"),
  install: { steps: [{ title: "选屏与安装包", hint: "选择应用包与目标" }, { title: "安装检查", hint: "核对安装包与范围" }, { title: "安装结果", hint: "查看逐台执行结果" }], executeLabel: "开始安装", readOnly: false },
  time: { steps: [{ title: "选屏与校时", hint: "确认时间基准" }, { title: "校时检查", hint: "核对设备与权限" }, { title: "校时结果", hint: "回读时间偏差" }], executeLabel: "确认校准", readOnly: false },
  adb: { steps: [{ title: "选择设备", hint: "支持的 10 寸屏" }, { title: "端口检查", hint: "核对当前连接" }, { title: "重启确认", hint: "确认短时不可用" }, { title: "恢复验证", hint: "验证 5555 与启动" }], executeLabel: "确认设置并重启", readOnly: false },
  reboot: { steps: [{ title: "选择设备", hint: "选择重启范围" }, { title: "重启确认", hint: "核对设备和影响" }, { title: "恢复结果", hint: "确认系统与连接" }], executeLabel: "确认重启屏", readOnly: false },
  restart: { steps: [{ title: "选择设备", hint: "已安装小新的屏" }, { title: "启动结果", hint: "确认应用恢复" }], executeLabel: "确认重启小新", readOnly: false }
};
