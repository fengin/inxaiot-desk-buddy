import { appConfigValue, appEnvironments, type AppConfigFieldDefinition, type AppConfigScalar, type AppConfigSchema, type AppConfigVoiceEdit, type ScreenAppConfigRead } from "./screenAppConfig";
import { summarizeAppConfigValues, type AppConfigValueSummary } from "./screenAppConfigEditor";

const forbidden = new Set(["__proto__", "constructor", "prototype"]);
const policies = new Set(["none", "appRestart", "currentEnvironmentRestart"]);
const groups = new Set(["general", "environment", "voice"]);
const schemaCache = new WeakMap<object, AppConfigSchema | null>();

export function safeAppConfigFieldKey(key: string): boolean {
  if (key.length > 160 || key.split(".").some(part => forbidden.has(part))) return false;
  return ["customDeviceName", "autoStartVoiceEngine", "environments.current"].includes(key)
    || /^environments\.(test|pre|prod)\.(otaUrl|wsUrl|h5Url|h5ReadyCheckEnabled)$/.test(key)
    || /^audioConfig\.[A-Za-z][A-Za-z0-9_]{0,63}$/.test(key);
}

function scalarMatches(value: unknown, type: AppConfigFieldDefinition["type"]): value is AppConfigScalar {
  return type === "boolean" ? typeof value === "boolean" : type === "string" ? typeof value === "string"
    : typeof value === "number" && Number.isFinite(value) && (type !== "integer" || Number.isInteger(value));
}
function schemaText(value: unknown, maximum: number, allowEmpty = false): value is string {
  return typeof value === "string" && (allowEmpty || Boolean(value.trim())) && Array.from(value).length <= maximum && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
}

/** 桌面后端已规范化元数据；前端仍拒绝坏草稿、未知类型和危险字段路径。 */
export function appConfigSchema(row: ScreenAppConfigRead): AppConfigSchema | null {
  const capabilities = row.capabilities;
  if (!capabilities) return null;
  const cached = schemaCache.get(capabilities);
  if (cached !== undefined) return cached;
  const raw = capabilities.configSchema;
  let valid = raw?.version === 1 && Array.isArray(raw.fields) && raw.fields.length > 0 && raw.fields.length <= 64;
  const keys = new Set<string>();
  if (valid) for (const field of raw!.fields) {
    if (!field || typeof field.key !== "string" || !safeAppConfigFieldKey(field.key) || keys.has(field.key)
      || !["boolean", "integer", "number", "string"].includes(field.type)
      || !schemaText(field.label, 80)
      || !groups.has(field.group) || !Number.isInteger(field.order) || Math.abs(field.order) > 10000 || typeof field.writable !== "boolean" || typeof field.clearable !== "boolean"
      || !policies.has(field.restartPolicy)
      || (field.section !== undefined && !schemaText(field.section, 80))
      || (field.unit !== undefined && !schemaText(field.unit, 16, true))
      || (field.description !== undefined && !schemaText(field.description, 400, true))
      || ([field.min, field.max, field.step].some(value => value !== undefined && (typeof value !== "number" || !Number.isFinite(value))))
      || (field.min !== undefined && field.max !== undefined && field.min > field.max)
      || (field.step !== undefined && field.step <= 0)
      || (field.maxLength !== undefined && (!Number.isInteger(field.maxLength) || field.maxLength <= 0 || field.maxLength > 4096))
      || (field.format !== undefined && !["httpUrl", "websocketUrl"].includes(field.format))
      || (field.options !== undefined && (!Array.isArray(field.options) || field.options.length === 0 || field.options.length > 64 || field.options.some((option, index) => !option || !schemaText(option.label, 80) || !scalarMatches(option.value, field.type) || field.options!.slice(0, index).some(previous => previous.value === option.value))))
      || (field.defaultValue !== undefined && field.defaultValue !== null && !scalarMatches(field.defaultValue, field.type))) {
      valid = false; break;
    }
    keys.add(field.key);
  }
  const result = valid ? raw! : null;
  schemaCache.set(capabilities, result);
  return result;
}

const legacyFields: AppConfigFieldDefinition[] = [
  { key: "customDeviceName", type: "string", label: "设备名", group: "general", order: 0, writable: true, clearable: true, maxLength: 128, restartPolicy: "none" },
  { key: "environments.current", type: "string", label: "选择环境", group: "general", order: 1, writable: true, clearable: false, options: appEnvironments, restartPolicy: "appRestart" },
  { key: "autoStartVoiceEngine", type: "boolean", label: "语音引擎随应用启动", group: "voice", section: "启动", order: 0, writable: true, clearable: false, defaultValue: false, restartPolicy: "appRestart" },
  ...(["test", "pre", "prod"] as const).flatMap(environment => ([
    { key: "otaUrl", label: "OTA 地址", clearable: false, format: "httpUrl" },
    { key: "wsUrl", label: "WebSocket(可选)", clearable: true, format: "websocketUrl" },
    { key: "h5Url", label: "H5地址(可选)", clearable: true, format: "httpUrl" },
  ] as const).map((item, order): AppConfigFieldDefinition => ({ ...item, key: `environments.${environment}.${item.key}`, type: "string", group: "environment", order, writable: true, maxLength: 4096, restartPolicy: "currentEnvironmentRestart" })).concat([
    { key: `environments.${environment}.h5ReadyCheckEnabled`, type: "boolean", label: "检查H5地址", group: "environment", order: 3, writable: true, clearable: false, defaultValue: true, restartPolicy: "currentEnvironmentRestart" },
  ])),
];

export function appConfigDefinitions(row: ScreenAppConfigRead): readonly AppConfigFieldDefinition[] {
  return appConfigSchema(row)?.fields ?? legacyFields;
}

export function appConfigDefinition(row: ScreenAppConfigRead, key: string): AppConfigFieldDefinition | undefined {
  return appConfigDefinitions(row).find(field => field.key === key);
}

export function appConfigCurrentValueKnown(definition: AppConfigFieldDefinition, value: unknown): boolean {
  return (value === null && definition.clearable) || scalarMatches(value, definition.type);
}

export function appConfigFieldIssue(definition: AppConfigFieldDefinition, value: unknown): string | undefined {
  if (value === null && definition.clearable) return undefined;
  if (!scalarMatches(value, definition.type)) return "参数值尚未确认或类型不正确";
  if (definition.options && !definition.options.some(option => option.value === value)) return "请选择屏端支持的值";
  if (typeof value === "number") {
    if ((definition.min !== undefined && value < definition.min) || (definition.max !== undefined && value > definition.max)) return `允许范围 ${definition.min ?? "不限"}～${definition.max ?? "不限"}${definition.unit ? ` ${definition.unit}` : ""}`;
  }
  if (typeof value === "string") {
    if (definition.maxLength !== undefined && value.length > definition.maxLength) return `最多 ${definition.maxLength} 个字符`;
    if (!definition.clearable && !value.trim()) return "此项不能为空";
    if (definition.format && value) {
      try {
        const url = new URL(value);
        const protocols = definition.format === "websocketUrl" ? ["ws:", "wss:"] : ["http:", "https:"];
        if (!protocols.includes(url.protocol) || !url.hostname || /\s/.test(value)) return "地址格式不正确";
      } catch { return "地址格式不正确"; }
    }
  }
  return undefined;
}

function constraintKey(field: AppConfigFieldDefinition): string {
  // 标签、说明及设备自适应默认值不影响合批；实际允许值和生效规则必须相容。
  return JSON.stringify([field.type, field.clearable, field.restartPolicy, field.min, field.max, field.step, field.unit, field.maxLength, field.format,
    field.options?.map(option => option.value).sort((a, b) => String(a).localeCompare(String(b)))]);
}

export interface AppConfigSchemaFieldState {
  definition: AppConfigFieldDefinition;
  summary: AppConfigValueSummary;
  editable: boolean;
  unavailableIds: string[];
  incompatible: boolean;
  defaultsVary: boolean;
}

export function appConfigVoiceFields(rows: ScreenAppConfigRead[]): AppConfigSchemaFieldState[] {
  const definitions = new Map<string, AppConfigFieldDefinition>();
  for (const row of rows) for (const field of appConfigDefinitions(row)) if (field.group === "voice") definitions.set(field.key, definitions.get(field.key) ?? field);
  if (!definitions.size) definitions.set("autoStartVoiceEngine", legacyFields.find(field => field.key === "autoStartVoiceEngine")!);
  return [...definitions.values()].sort((a, b) => a.order - b.order || a.key.localeCompare(b.key)).map(definition => {
    const available = rows.filter(row => {
      const currentDefinition = appConfigDefinition(row, definition.key);
      return row.capabilities?.ready === true && row.capabilities.supportedFields.includes(definition.key)
        && Boolean(currentDefinition?.writable) && Boolean(row.config)
        && appConfigCurrentValueKnown(currentDefinition!, appConfigValue(row.config!, definition.key));
    });
    const availableRows = new Set(available);
    const unavailableIds = rows.filter(row => !availableRows.has(row)).map(row => row.screenId);
    const incompatible = available.some(row => constraintKey(appConfigDefinition(row, definition.key)!) !== constraintKey(definition));
    const defaultsVary = new Set(available.map(row => JSON.stringify(appConfigDefinition(row, definition.key)?.defaultValue))).size > 1;
    return { definition, summary: summarizeAppConfigValues(available.map(row => appConfigValue(row.config!, definition.key) as AppConfigScalar)),
      editable: rows.length > 0 && unavailableIds.length === 0 && !incompatible, unavailableIds, incompatible, defaultsVary };
  });
}

/** 普通二态回到原值即保留；多个屏的值不同时用第三态恢复各自原值。 */
export function nextAppConfigBooleanMode(mode: "keep" | "on" | "off", value: boolean, baseline: boolean | undefined, mixed: boolean): "keep" | "on" | "off" {
  if (mixed) return mode === "keep" ? "on" : mode === "on" ? "off" : "keep";
  return value === baseline ? "keep" : value ? "on" : "off";
}

export function appConfigVoiceEdit(state: AppConfigSchemaFieldState, previous: AppConfigVoiceEdit | undefined, value: AppConfigScalar | null): AppConfigVoiceEdit {
  if (value === null) return { mode: "keep", value: null };
  if (state.definition.type === "boolean") {
    const mode = nextAppConfigBooleanMode(previous?.mode === "set" ? previous.value === true ? "on" : "off" : "keep", value === true,
      typeof state.summary.value === "boolean" ? state.summary.value : undefined, state.summary.mixed);
    return { mode: mode === "keep" ? "keep" : "set", value: mode === "keep" ? null : mode === "on" };
  }
  if (!state.summary.mixed && value === state.summary.value) return { mode: "keep", value: null };
  return { mode: "set", value };
}
