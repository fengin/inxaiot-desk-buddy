import { formatDisplayDateTime } from "@/shared/format/dateTime";
export type AppEnvironment = "test" | "pre" | "prod";
export interface ScreenEnvironmentConfig {
  otaUrl: string; wsUrl: string | null; h5Url: string | null;
  h5ReadyCheckEnabled: boolean; otaWsUrl: string | null; otaH5Url: string | null;
  effectiveWsUrl: string | null; effectiveH5Url: string;
  wsSource: "manual" | "server" | "none"; h5Source: "manual" | "server" | "default";
}
export interface ScreenAppConfiguration {
  customDeviceName: string | null;
  /** 旧 APK 未提供此字段时保持未知，不能默认为关闭。 */
  autoStartVoiceEngine?: boolean;
  environments: { current: AppEnvironment } & Record<AppEnvironment, ScreenEnvironmentConfig>;
}
export interface ScreenAppConfigRead {
  screenId: string; readAt: string; config: ScreenAppConfiguration | null;
  capabilities: { appVersionName: string; appVersionCode: number; protocolVersion: number; supportedFields: string[]; ready: boolean } | null;
  message: string;
  home?: { supported: boolean; enabled?: boolean; active?: boolean; message?: string };
}
export interface ScreenAppConfigPatch { set: Record<string, string | boolean>; clear: string[]; homeEnabled?: boolean }
export interface ScreenAppConfigDraft {
  environment: AppEnvironment; switchEnvironment: boolean; readyMode: "keep" | "on" | "off";
  homeMode?: "keep" | "on" | "off";
  voiceMode?: "keep" | "on" | "off";
  edits: Record<string, { mode: "keep" | "set" | "clear"; value: string }>;
  names: Record<string, { mode: "keep" | "set" | "clear"; value: string }>;
  targetIds: string[]; includedIds: string[]; rows: ScreenAppConfigRead[]; savedAt?: string;
}
export const appEnvironments = [{ label: "测试环境", value: "test" }, { label: "预发环境", value: "pre" }, { label: "生产环境", value: "prod" }];
export const appConfigSources: Record<string, string> = { manual: "手动设置", server: "服务端下发", default: "默认网页", none: "尚未配置" };
export function appConfigReadTime(value: string) {
  return formatDisplayDateTime(value);
}
export function appConfigFieldLabel(field: string) {
  const parts=field.split('.'), key = parts.at(-1)!;
  const label=({ customDeviceName: "设备名", autoStartVoiceEngine: "语音引擎随应用启动", current: "选择环境", otaUrl: "OTA 地址", wsUrl: "WebSocket(可选)", h5Url: "H5地址(可选)", h5ReadyCheckEnabled: "H5检测", homeEnabled: "固定小新为主屏", h5ReadyRecord: "网页初始化记录" } as Record<string, string>)[key] ?? field;
  const environment=parts.length===3?appEnvironments.find(item=>item.value===parts[1])?.label:undefined;
  return environment?`${environment} · ${label}`:label;
}
export function appConfigValue(config: ScreenAppConfiguration, field: string): unknown {
  return field.split(".").reduce<unknown>((value, key) => value && typeof value === "object" ? (value as Record<string, unknown>)[key] : undefined, config);
}
export function appConfigDisplay(value: unknown, environment = false) {
  if (value === null || value === undefined || value === "") return "未设置";
  if (typeof value === "boolean") return value ? "开启" : "关闭";
  return (environment ? appEnvironments.find(e => e.value === value)?.label : undefined) ?? String(value);
}
export function appConfigChanges(config: ScreenAppConfiguration, patch: ScreenAppConfigPatch, home?: ScreenAppConfigRead["home"]) {
  const changes = [...Object.entries(patch.set), ...patch.clear.map(key => [key, null] as const)]
    .filter(([key, value]) => appConfigValue(config, key) !== value)
    .map(([key, value]) => ({ key, label: appConfigFieldLabel(key), before: appConfigDisplay(appConfigValue(config, key), key==='environments.current'), after: value === null ? "清空手动值" : appConfigDisplay(value,key==='environments.current') }));
  if (typeof patch.homeEnabled === "boolean" && (patch.homeEnabled !== home?.enabled || (patch.homeEnabled && home?.active !== true))) {
    changes.push({ key: "homeEnabled", label: appConfigFieldLabel("homeEnabled"), before: typeof home?.enabled === "boolean" ? home.enabled ? (patch.homeEnabled && home.active !== true ? "已固定，激活未确认" : "固定") : "未固定" : "未确认", after: patch.homeEnabled ? "固定并激活" : "解除固定" });
  }
  return changes;
}
export function appHomeDisplay(home?: ScreenAppConfigRead["home"]) {
  if (!home?.supported || typeof home.enabled !== "boolean") return home?.message || "未确认，当前不可修改";
  if (!home.enabled) return "未固定";
  return home.active === true ? "固定并已激活" : home.active === false ? "已固定，待激活" : "已固定，激活待核实";
}
export function appConfigVoiceSupported(row: ScreenAppConfigRead): boolean {
  return row.capabilities?.ready === true && Array.isArray(row.capabilities.supportedFields) && row.capabilities.supportedFields.includes("autoStartVoiceEngine")
    && typeof row.config?.autoStartVoiceEngine === "boolean";
}
export function appConfigVoiceDisplay(row: ScreenAppConfigRead): string {
  return appConfigVoiceSupported(row) ? appConfigDisplay(row.config!.autoStartVoiceEngine) : "未确认，当前不可修改";
}
export function appConfigRestartRequired(config: ScreenAppConfiguration, patch: ScreenAppConfigPatch) {
  return appConfigChanges(config, patch).some(({ key }) => key === "autoStartVoiceEngine" || key === "environments.current" || key.startsWith(`environments.${config.environments.current}.`));
}
export function appConfigEffectiveAfter(config: ScreenAppConfiguration, patch: ScreenAppConfigPatch, environment: AppEnvironment) {
  const current=config.environments[environment];
  const manual=(key:'wsUrl'|'h5Url')=>{const field=`environments.${environment}.${key}`;return patch.clear.includes(field)?null:typeof patch.set[field]==='string'?String(patch.set[field]):current[key];};
  const ws=manual('wsUrl'),h5=manual('h5Url');
  return {
    ws:{value:ws||current.otaWsUrl||'尚未配置',source:ws?'manual':current.otaWsUrl?'server':'none'},
    h5:{value:h5||current.otaH5Url||'默认网页',source:h5?'manual':current.otaH5Url?'server':'default'},
  };
}
