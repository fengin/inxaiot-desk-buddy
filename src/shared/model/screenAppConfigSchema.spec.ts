import { describe, expect, it } from "vitest";
import { appConfigChanges, appConfigRestartRequired, type AppConfigFieldDefinition, type ScreenAppConfigRead } from "./screenAppConfig";
import { appConfigFieldIssue, appConfigSchema, appConfigVoiceEdit, appConfigVoiceFields, safeAppConfigFieldKey } from "./screenAppConfigSchema";

export function voiceDefinition(key = "audioConfig.minSilenceDuration", overrides: Partial<AppConfigFieldDefinition> = {}): AppConfigFieldDefinition {
  return { key, type: "number", label: "静音判定时长", group: "voice", section: "语音检测", order: 30, writable: true, clearable: false,
    defaultValue: 0.6, min: 0.3, max: 2, step: 0.1, unit: "秒", restartPolicy: "appRestart", ...overrides };
}
export function schemaRow(id = "one", fields = [voiceDefinition()], audio = { minSilenceDuration: 0.6 }): ScreenAppConfigRead {
  const env = { otaUrl: "https://fixture.invalid/ota", wsUrl: null, h5Url: null, h5ReadyCheckEnabled: true, otaWsUrl: null, otaH5Url: null,
    effectiveWsUrl: null, effectiveH5Url: "https://fixture.invalid/h5", wsSource: "none" as const, h5Source: "default" as const };
  return { screenId: id, readAt: "2026-10-22T00:00:00Z", message: "已读取", config: { customDeviceName: null, autoStartVoiceEngine: false, audioConfig: { ...audio }, environments: { current: "pre", test: { ...env }, pre: { ...env }, prod: { ...env } } },
    capabilities: { appVersionName: "fixture", appVersionCode: 1, protocolVersion: 1, ready: true, supportedFields: fields.map(field => field.key), configSchema: { version: 1, fields } } };
}

describe("屏端字段定义与语音编辑", () => {
  it("动态显示未在工作台硬编码的新音频叶子及声明标签、单位和重启策略", () => {
    const field = voiceDefinition("audioConfig.futurePause", { label: "后续停顿参数", restartPolicy: "none" });
    const row = schemaRow("one", [field], { futurePause: 0.6 } as unknown as { minSilenceDuration: number });
    const states = appConfigVoiceFields([row]);
    expect(states).toHaveLength(1);
    expect(states[0]!.definition.label).toBe("后续停顿参数");
    const patch = { set: { "audioConfig.futurePause": 0.8 }, clear: [] };
    expect(appConfigChanges(row.config!, patch, undefined, [field])).toEqual([{ key: field.key, label: field.label, before: "0.6 秒", after: "0.8 秒" }]);
    expect(appConfigRestartRequired(row.config!, patch, [field])).toBe(false);
  });

  it("已有值缺失或类型错误不能用默认值补齐成可修改", () => {
    for (const value of [undefined, "0.6", null, Number.NaN]) {
      const row = schemaRow();
      row.config!.audioConfig!.minSilenceDuration = value as number;
      const state = appConfigVoiceFields([row])[0]!;
      expect(state.editable).toBe(false);
      expect(state.summary.total).toBe(0);
      expect(state.summary.value).not.toBe(0.6);
    }
  });

  it("当前数值越过新范围仍显示真实值并允许纠正，新值须通过约束", () => {
    const row = schemaRow("old-value", [voiceDefinition()], { minSilenceDuration: 0.1 });
    const state = appConfigVoiceFields([row])[0]!;
    expect(state.editable).toBe(true);
    expect(state.summary.value).toBe(0.1);
    expect(appConfigFieldIssue(state.definition, 0.1)).toContain("允许范围");
    expect(appConfigVoiceEdit(state, undefined, 0.1).mode).toBe("keep");
    expect(appConfigVoiceEdit(state, undefined, 0.3)).toEqual({ mode: "set", value: 0.3 });
    expect(appConfigFieldIssue(state.definition, 0.3)).toBeUndefined();
  });

  it("step只影响输入增量，合法0.62保留；2和2.0不构成差异", () => {
    const state = appConfigVoiceFields([schemaRow("one", [voiceDefinition()], { minSilenceDuration: 0.62 })])[0]!;
    expect(state.editable).toBe(true);
    expect(appConfigFieldIssue(state.definition, 0.62)).toBeUndefined();
    expect(appConfigVoiceEdit(state, undefined, 0.62).mode).toBe("keep");
    const row = schemaRow("two", [voiceDefinition()], { minSilenceDuration: 2 });
    expect(appConfigChanges(row.config!, { set: { "audioConfig.minSilenceDuration": 2.0 }, clear: [] })).toHaveLength(0);
  });

  it("多屏混合值只在明确输入后统一，清空恢复各屏原值", () => {
    const state = appConfigVoiceFields([schemaRow("one"), schemaRow("two", [voiceDefinition()], { minSilenceDuration: 1 })])[0]!;
    expect(state.summary.mixed).toBe(true);
    expect(appConfigVoiceEdit(state, undefined, 0.6)).toEqual({ mode: "set", value: 0.6 });
    expect(appConfigVoiceEdit(state, { mode: "set", value: 0.6 }, null)).toEqual({ mode: "keep", value: null });
  });

  it("默认值可按设备不同，约束冲突则只禁该参数", () => {
    const first = schemaRow("one"), second = schemaRow("two", [voiceDefinition(undefined, { defaultValue: 1 })]);
    expect(appConfigVoiceFields([first, second])[0]).toMatchObject({ editable: true, defaultsVary: true });
    const constrained = schemaRow("three", [voiceDefinition(undefined, { min: 0.5 })]);
    expect(appConfigVoiceFields([first, constrained])[0]).toMatchObject({ editable: false, incompatible: true });
  });

  it("旧APP保留有效启动开关能力，但不会凭音频值或默认值开放新参数", () => {
    const old = schemaRow("old");
    delete old.capabilities!.configSchema;
    old.capabilities!.supportedFields = ["autoStartVoiceEngine", "audioConfig.minSilenceDuration"];
    const states = appConfigVoiceFields([old, schemaRow("new")]);
    expect(states.find(field => field.definition.key === "audioConfig.minSilenceDuration")).toMatchObject({ editable: false, unavailableIds: ["old"] });
    expect(appConfigVoiceFields([old])).toHaveLength(1);
    expect(appConfigVoiceFields([old])[0]).toMatchObject({ editable: true, summary: { value: false } });
  });

  it("混合bool三态循环可恢复keep，统一bool改回基线即取消", () => {
    const definition = voiceDefinition("autoStartVoiceEngine", { type: "boolean", label: "语音启动", defaultValue: false, min: undefined, max: undefined, step: undefined, unit: undefined });
    const one = schemaRow("one", [definition]), two = schemaRow("two", [definition]);
    two.config!.autoStartVoiceEngine = true;
    const state = appConfigVoiceFields([one, two])[0]!;
    const on = appConfigVoiceEdit(state, undefined, true), off = appConfigVoiceEdit(state, on, false);
    expect(on).toEqual({ mode: "set", value: true });
    expect(off).toEqual({ mode: "set", value: false });
    expect(appConfigVoiceEdit(state, off, true).mode).toBe("keep");
    const same = appConfigVoiceFields([one])[0]!;
    expect(appConfigVoiceEdit(same, on, false).mode).toBe("keep");
  });

  it("坏schema、危险键和重复定义不进入动态编辑", () => {
    for (const key of ["audioConfig.__proto__", "audioConfig.constructor", "audioConfig.prototype", "authCode", "audioConfig.x.y"]) {
      expect(safeAppConfigFieldKey(key)).toBe(false);
      expect(appConfigSchema(schemaRow("bad", [voiceDefinition(key)]))).toBeNull();
    }
    const duplicate = schemaRow("same", [voiceDefinition(), voiceDefinition()]);
    expect(appConfigSchema(duplicate)).toBeNull();
    const unknown = schemaRow();
    unknown.capabilities!.configSchema!.version = 2 as 1;
    expect(appConfigSchema(unknown)).toBeNull();
  });

  it.each([33, 64, 65])("枚举选项数量 %s 与APP及Rust契约一致，上限64", count => {
    const field = voiceDefinition("audioConfig.futureChoice", { options: Array.from({ length: count }, (_, value) => ({ label: `选项${value}`, value })), defaultValue: 0, min: 0, max: 100 });
    const row = schemaRow("options", [field]);
    expect(Boolean(appConfigSchema(row))).toBe(count <= 64);
  });

  it("定义文案长度、控制字符和枚举重复值遵循后端范围", () => {
    const good = voiceDefinition(undefined, { label: '字'.repeat(80), section: '组'.repeat(80), unit: '字'.repeat(16), description: '字'.repeat(400) });
    expect(appConfigSchema(schemaRow('good', [good]))).not.toBeNull();
    for (const overrides of [{ label: '字'.repeat(81) }, { unit: '字'.repeat(17) }, { description: '字'.repeat(401) }, { section: '' }, { label: '字段\n名称' }, { order: 10001 }, { options: [{ label: '一', value: 1 }, { label: '二', value: 1.0 }] }]) {
      expect(appConfigSchema(schemaRow('bad', [voiceDefinition(undefined, overrides)]))).toBeNull();
    }
  });
});
