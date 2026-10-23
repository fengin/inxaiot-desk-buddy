import { formatDisplayDateTime } from "@/shared/format/dateTime";
export type AppEnvironment = "test" | "pre" | "prod";
export type AppConfigScalar = string | boolean | number;
export interface AppConfigFieldDefinition {
  key: string; type: "boolean" | "integer" | "number" | "string";
  label: string; group: "general" | "environment" | "voice"; section?: string; order: number;
  writable: boolean; clearable: boolean; defaultValue?: AppConfigScalar | null;
  options?: { label: string; value: AppConfigScalar }[];
  min?: number; max?: number; step?: number; unit?: string; description?: string;
  maxLength?: number; format?: "httpUrl" | "websocketUrl";
  restartPolicy: "none" | "appRestart" | "currentEnvironmentRestart";
}
export interface AppConfigSchema { version: 1; fields: AppConfigFieldDefinition[] }
export interface AppConfigVoiceEdit { mode: "keep" | "set"; value: AppConfigScalar | null }
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
  /** 只保留屏端能力定义明确返回的叶子值；缺失值保持未知。 */
  audioConfig?: Record<string, AppConfigScalar>;
  environments: { current: AppEnvironment } & Record<AppEnvironment, ScreenEnvironmentConfig>;
}
export interface ScreenAppConfigRead {
  screenId: string; readAt: string; config: ScreenAppConfiguration | null;
  capabilities: { appVersionName: string; appVersionCode: number; protocolVersion: number; supportedFields: string[]; ready: boolean; configSchema?: AppConfigSchema; configSchemaError?: string } | null;
  message: string;
  home?: { supported: boolean; enabled?: boolean; active?: boolean; message?: string };
  owner?: { state: "self" | "none" | "other" | "unknown"; component?: string | null; message: string };
  kiosk?: { supported: boolean; configured?: boolean; enabled?: boolean; active?: boolean | null; activityReady?: boolean; lockTaskMode?: "none" | "locked" | "pinned"; message: string };
}
export interface ScreenAppConfigPatch { set: Record<string, AppConfigScalar>; clear: string[]; homeEnabled?: boolean; ensureDeviceOwner?: true; kioskEnabled?: boolean }
export interface ScreenAppConfigDraft {
  environment: AppEnvironment; switchEnvironment: boolean; readyMode: "keep" | "on" | "off";
  homeMode?: "keep" | "on" | "off";
  homeActivation?: boolean;
  ensureDeviceOwner?: true;
  kioskMode?: "keep" | "on" | "off";
  voiceMode?: "keep" | "on" | "off";
  voiceEdits?: Record<string, AppConfigVoiceEdit>;
  edits: Record<string, { mode: "keep" | "set" | "clear"; value: string }>;
  names: Record<string, { mode: "keep" | "set" | "clear"; value: string }>;
  targetIds: string[]; includedIds: string[]; rows: ScreenAppConfigRead[]; savedAt?: string;
}
export const appEnvironments = [{ label: "测试环境", value: "test" }, { label: "预发环境", value: "pre" }, { label: "生产环境", value: "prod" }];
export const appConfigSources: Record<string, string> = { manual: "手动设置", server: "服务端下发", default: "默认网页", none: "尚未配置" };
export function appConfigReadTime(value: string) {
  return formatDisplayDateTime(value);
}
export function appConfigFieldLabel(field: string, definitions?: readonly AppConfigFieldDefinition[]) {
  const parts=field.split('.'), key = parts.at(-1)!;
  const label=definitions?.find(item => item.key === field)?.label ?? ({ customDeviceName: "设备名", autoStartVoiceEngine: "语音引擎随应用启动", current: "选择环境", otaUrl: "OTA 地址", wsUrl: "WebSocket(可选)", h5Url: "H5地址(可选)", h5ReadyCheckEnabled: "H5检测", homeEnabled: "固定小新为主屏", ensureDeviceOwner: "设备OWNER权限", kioskEnabled: "锁定应用", h5ReadyRecord: "网页初始化记录" } as Record<string, string>)[key] ?? field;
  const environment=parts.length===3?appEnvironments.find(item=>item.value===parts[1])?.label:undefined;
  return environment?`${environment} · ${label}`:label;
}
export function appConfigValue(config: ScreenAppConfiguration, field: string): unknown {
  return field.split(".").reduce<unknown>((value, key) => value && typeof value === "object" ? (value as Record<string, unknown>)[key] : undefined, config);
}
export function appConfigDisplay(value: unknown, environment = false, definition?: AppConfigFieldDefinition) {
  if (value === null || value === undefined || value === "") return "未设置";
  if (typeof value === "boolean") return value ? "开启" : "关闭";
  const option = definition?.options?.find(item => item.value === value);
  if (option) return option.label;
  if (typeof value === "number" && definition?.unit) return `${value} ${definition.unit}`;
  return (environment ? appEnvironments.find(e => e.value === value)?.label : undefined) ?? String(value);
}
export function appConfigChanges(config: ScreenAppConfiguration, patch: ScreenAppConfigPatch, home?: ScreenAppConfigRead["home"], definitions?: readonly AppConfigFieldDefinition[], owner?: ScreenAppConfigRead["owner"], kiosk?: ScreenAppConfigRead["kiosk"]) {
  const changes = [...Object.entries(patch.set), ...patch.clear.map(key => [key, null] as const)]
    .filter(([key, value]) => appConfigValue(config, key) !== value)
    .map(([key, value]) => { const definition = definitions?.find(item => item.key === key); return { key, label: appConfigFieldLabel(key, definitions), before: appConfigDisplay(appConfigValue(config, key), key==='environments.current', definition), after: value === null ? "清空手动值" : appConfigDisplay(value,key==='environments.current', definition) }; });
  if (typeof patch.homeEnabled === "boolean" && (patch.homeEnabled !== home?.enabled || (patch.homeEnabled && home?.active !== true))) {
    changes.push({ key: "homeEnabled", label: appConfigFieldLabel("homeEnabled"), before: typeof home?.enabled === "boolean" ? home.enabled ? (patch.homeEnabled && home.active !== true ? "已固定，激活未确认" : "固定") : "未固定" : "未确认", after: patch.homeEnabled ? "固定并激活" : "解除固定" });
  }
  if (patch.ensureDeviceOwner === true && owner?.state === "none") {
    changes.push({ key: "ensureDeviceOwner", label: appConfigFieldLabel("ensureDeviceOwner"), before: "尚未设置设备OWNER", after: "设置小新为设备OWNER" });
  }
  if (typeof patch.kioskEnabled === "boolean" && kiosk?.supported && (kiosk.configured === false || kiosk.enabled !== patch.kioskEnabled || kiosk.active !== patch.kioskEnabled)) {
    const before = kiosk.configured === false ? "尚未保存" : kiosk.enabled === true ? kiosk.active === false ? "已开启，未锁定" : "开启" : kiosk.active === true ? "已关闭，仍锁定" : "关闭";
    changes.push({ key: "kioskEnabled", label: appConfigFieldLabel("kioskEnabled"), before, after: patch.kioskEnabled ? "开启并锁定" : "关闭并解锁" });
  }
  return changes;
}
export function appOwnerDisplay(owner?: ScreenAppConfigRead["owner"]) {
  if (!owner) return "当前小新缺少OWNER状态能力，请升级后重新读取";
  return owner.state === "self" ? "已具备设备OWNER权限" : owner.state === "none" ? "尚未设置设备OWNER"
    : owner.state === "other" ? "设备OWNER由其他应用持有" : "设备OWNER状态未确认";
}
export function appOwnerAllowsDependent(row: ScreenAppConfigRead, ensureDeviceOwner = false) {
  return row.owner?.state === "self" || (row.owner?.state === "none" && ensureDeviceOwner);
}
export function appKioskConfigurationKnown(row: ScreenAppConfigRead) {
  return row.kiosk?.supported === true && (row.kiosk.configured === false || (row.kiosk.configured === true && typeof row.kiosk.enabled === "boolean"));
}
export function appKioskTarget(row: ScreenAppConfigRead, mode: "keep" | "on" | "off"): boolean | undefined {
  if (!appKioskConfigurationKnown(row) || typeof row.kiosk?.active !== "boolean") return undefined;
  if (mode !== "keep") return mode === "on";
  if (row.kiosk.configured === false) return true;
  return row.kiosk.active !== row.kiosk.enabled ? row.kiosk.enabled : undefined;
}
export function appKioskDisplay(kiosk?: ScreenAppConfigRead["kiosk"]) {
  if (!kiosk?.supported) return kiosk?.message || "当前小新不支持应用锁定，请升级后重新读取";
  if (kiosk.configured === false) return "尚未保存（默认目标：开启）";
  if (kiosk.configured !== true || typeof kiosk.enabled !== "boolean") return "锁定设置未确认";
  return kiosk.enabled ? kiosk.active === false ? "已开启，未锁定" : "开启" : kiosk.active === true ? "已关闭，仍锁定" : "关闭";
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
export function appConfigRestartRequired(config: ScreenAppConfiguration, patch: ScreenAppConfigPatch, definitions?: readonly AppConfigFieldDefinition[]) {
  const afterEnvironment = typeof patch.set["environments.current"] === "string" ? patch.set["environments.current"] : config.environments.current;
  return appConfigChanges(config, patch).some(({ key }) => {
    const policy = definitions?.find(item => item.key === key)?.restartPolicy;
    return policy ? policy === "appRestart" || (policy === "currentEnvironmentRestart" && key.startsWith(`environments.${afterEnvironment}.`)) : key === "autoStartVoiceEngine" || key === "environments.current" || key.startsWith(`environments.${afterEnvironment}.`);
  });
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
