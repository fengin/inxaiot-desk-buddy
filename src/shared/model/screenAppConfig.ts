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
  environments: { current: AppEnvironment } & Record<AppEnvironment, ScreenEnvironmentConfig>;
}
export interface ScreenAppConfigRead {
  screenId: string; readAt: string; config: ScreenAppConfiguration | null;
  capabilities: { appVersionName: string; appVersionCode: number; protocolVersion: number; supportedFields: string[]; ready: boolean } | null;
  message: string;
}
export interface ScreenAppConfigPatch { set: Record<string, string | boolean>; clear: string[] }
export interface ScreenAppConfigDraft {
  environment: AppEnvironment; switchEnvironment: boolean; readyMode: "keep" | "on" | "off";
  edits: Record<string, { mode: "keep" | "set" | "clear"; value: string }>;
  names: Record<string, { mode: "keep" | "set" | "clear"; value: string }>;
  targetIds: string[]; includedIds: string[]; rows: ScreenAppConfigRead[]; savedAt?: string;
}
export const appEnvironments = [{ label: "测试环境", value: "test" }, { label: "预发环境", value: "pre" }, { label: "生产环境", value: "prod" }];
export const appConfigSources: Record<string, string> = { manual: "手动设置", server: "服务端下发", default: "默认网页", none: "尚未配置" };
export function appConfigReadTime(value: string) {
  const milliseconds = Date.parse(value);
  return formatDisplayDateTime(Number.isFinite(milliseconds) ? String(milliseconds) : value);
}
export function appConfigFieldLabel(field: string) {
  const parts=field.split('.'), key = parts.at(-1)!;
  const label=({ customDeviceName: "设备名", current: "选择环境", otaUrl: "OTA 地址", wsUrl: "WebSocket(可选)", h5Url: "H5地址(可选)", h5ReadyCheckEnabled: "H5检测", h5ReadyRecord: "网页初始化记录" } as Record<string, string>)[key] ?? field;
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
export function appConfigChanges(config: ScreenAppConfiguration, patch: ScreenAppConfigPatch) {
  return [...Object.entries(patch.set), ...patch.clear.map(key => [key, null] as const)]
    .filter(([key, value]) => appConfigValue(config, key) !== value)
    .map(([key, value]) => ({ key, label: appConfigFieldLabel(key), before: appConfigDisplay(appConfigValue(config, key), key==='environments.current'), after: value === null ? "清空手动值" : appConfigDisplay(value,key==='environments.current') }));
}
export function appConfigRestartRequired(config: ScreenAppConfiguration, patch: ScreenAppConfigPatch) {
  return appConfigChanges(config, patch).some(({ key }) => key === "environments.current" || key.startsWith(`environments.${config.environments.current}.`));
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
