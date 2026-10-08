import {
  appConfigDisplay,
  appConfigFieldLabel,
  type AppEnvironment,
  type ScreenAppConfigDraft,
  type ScreenAppConfiguration,
  type ScreenAppConfigRead,
} from "@/shared/model/screenAppConfig";

type AppConfigValue = string | boolean | null | undefined;

export interface AppConfigValueSummary {
  value: AppConfigValue;
  mixed: boolean;
  tied: boolean;
  count: number;
  total: number;
}

export type AppConfigEditorField = "current" | "otaUrl" | "wsUrl" | "h5Url" | "h5ReadyCheckEnabled";
export type AppConfigSummaries = Record<AppConfigEditorField, AppConfigValueSummary>;

export interface AppConfigDifference {
  key: AppConfigEditorField;
  label: string;
  value: string;
  reference: string;
}

const editorFields: AppConfigEditorField[] = ["current", "otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"];

function normalizeValue(value: AppConfigValue): string | boolean | null {
  return value === "" || value === undefined ? null : value;
}

/** 代表值只用于表单展示，不能据此为未编辑的字段生成批量修改。 */
export function summarizeAppConfigValues(values: AppConfigValue[]): AppConfigValueSummary {
  const counts = new Map<string | boolean | null, number>();
  for (const value of values) {
    const normalized = normalizeValue(value);
    counts.set(normalized, (counts.get(normalized) ?? 0) + 1);
  }
  const count = Math.max(0, ...counts.values());
  const leaders = [...counts].filter(([, frequency]) => frequency === count);
  return {
    value: leaders.length === 1 ? leaders[0]![0] : undefined,
    mixed: counts.size > 1,
    tied: leaders.length > 1,
    count,
    total: values.length,
  };
}

function fieldValue(config: ScreenAppConfiguration, field: AppConfigEditorField, environment: AppEnvironment): AppConfigValue {
  return field === "current" ? config.environments.current : config.environments[environment][field];
}

/** 调用方传入已勾选的读取结果；读取失败的屏不参与代表值统计。 */
export function summarizeAppConfigs(rows: ScreenAppConfigRead[], environment: AppEnvironment): AppConfigSummaries {
  const configurations = rows.flatMap(row => row.config ? [row.config] : []);
  return Object.fromEntries(editorFields.map(field => [
    field,
    summarizeAppConfigValues(configurations.map(config => fieldValue(config, field, environment))),
  ])) as AppConfigSummaries;
}

export function appConfigDifferences(
  config: ScreenAppConfiguration,
  summaries: AppConfigSummaries,
  environment: AppEnvironment,
): AppConfigDifference[] {
  return editorFields.flatMap(key => {
    const summary = summaries[key];
    const value = fieldValue(config, key, environment);
    if (summary.total === 0 || (!summary.tied && normalizeValue(value) === summary.value)) return [];
    return [{
      key,
      label: appConfigFieldLabel(key),
      value: appConfigDisplay(value, key === "current"),
      reference: summary.tied ? "多个值，无多数值" : appConfigDisplay(summary.value, key === "current"),
    }];
  });
}

/** 仅在用户输入后调用；初始展示及重新读取不能自动标记为修改。 */
export function appConfigTextEdit(
  value: string,
  baseline: AppConfigValueSummary,
  clearable: boolean,
): ScreenAppConfigDraft["edits"][string] {
  const trimmed = value.trim();
  if (!baseline.tied && baseline.total > 0 && normalizeValue(trimmed) === baseline.value) {
    return { mode: "keep", value };
  }
  return { mode: trimmed === "" && clearable ? "clear" : "set", value };
}
