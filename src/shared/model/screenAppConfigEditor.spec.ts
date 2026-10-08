import { describe, expect, it } from "vitest";
import type { ScreenAppConfiguration, ScreenAppConfigRead } from "@/shared/model/screenAppConfig";
import { appConfigFieldLabel } from "@/shared/model/screenAppConfig";
import {
  appConfigDifferences,
  appConfigTextEdit,
  summarizeAppConfigs,
  summarizeAppConfigValues,
} from "@/shared/model/screenAppConfigEditor";

function configuration(): ScreenAppConfiguration {
  const environment = {
    otaUrl: "https://config.example.org", wsUrl: null, h5Url: null,
    h5ReadyCheckEnabled: true, otaWsUrl: null, otaH5Url: null,
    effectiveWsUrl: null, effectiveH5Url: "assets/web/demo.html",
    wsSource: "none" as const, h5Source: "default" as const,
  };
  return {
    customDeviceName: "小新",
    environments: { current: "test", test: { ...environment }, pre: { ...environment }, prod: { ...environment } },
  };
}

function row(config: ScreenAppConfiguration | null, screenId = "screen-1"): ScreenAppConfigRead {
  return { screenId, readAt: "2026-10-04T04:00:00Z", config, capabilities: null, message: config ? "读取完成" : "读取超时" };
}

describe("批量配置代表值", () => {
  it("使用唯一最高频值，并记录与其它屏是否不同", () => {
    expect(summarizeAppConfigValues(["test", "prod", "test", "pre"])).toEqual({
      value: "test", mixed: true, tied: false, count: 2, total: 4,
    });
  });

  it("最高频并列时不任意选择第一台的值", () => {
    expect(summarizeAppConfigValues(["prod", "test", "test", "prod", "pre"])).toEqual({
      value: undefined, mixed: true, tied: true, count: 2, total: 5,
    });
  });

  it("空字符串、null、undefined 均作为未设置，但布尔关闭是实际值", () => {
    expect(summarizeAppConfigValues(["", null, undefined])).toEqual({
      value: null, mixed: false, tied: false, count: 3, total: 3,
    });
    expect(summarizeAppConfigValues([false, false, true])).toEqual({
      value: false, mixed: true, tied: false, count: 2, total: 3,
    });
  });

  it("未读取不会被当成未设置", () => {
    expect(summarizeAppConfigValues([])).toEqual({
      value: undefined, mixed: false, tied: false, count: 0, total: 0,
    });
    const summaries = summarizeAppConfigs([row(null)], "test");
    expect(Object.values(summaries).every(summary => summary.total === 0 && summary.value === undefined)).toBe(true);
    expect(appConfigDifferences(configuration(), summaries, "test")).toEqual([]);
  });

  it("忽略读取失败，运行环境与所选环境的地址分别统计", () => {
    const first = configuration();
    const second = configuration();
    second.environments.current = "prod";
    first.environments.pre.otaUrl = second.environments.pre.otaUrl = "https://pre.example.org";
    const summaries = summarizeAppConfigs([row(first), row(second, "screen-2"), row(null, "screen-3")], "pre");
    expect(summaries.current).toMatchObject({ tied: true, total: 2 });
    expect(summaries.otaUrl).toMatchObject({ value: "https://pre.example.org", total: 2, mixed: false });
  });
});

describe("屏配置差异", () => {
  it("仅列不同的字段，不把每台不同的名称当成配置异常", () => {
    const usual = configuration();
    const other = configuration();
    other.customDeviceName = "会议室小新";
    other.environments.current = "prod";
    other.environments.test.wsUrl = "wss://voice.example.org";
    other.environments.test.h5ReadyCheckEnabled = false;
    const summaries = summarizeAppConfigs([row(usual), row(usual, "screen-2"), row(other, "screen-3")], "test");
    expect(appConfigDifferences(usual, summaries, "test")).toEqual([]);
    expect(appConfigDifferences(other, summaries, "test")).toEqual([
      { key: "current", label: appConfigFieldLabel("current"), value: "生产环境", reference: "测试环境" },
      { key: "wsUrl", label: appConfigFieldLabel("wsUrl"), value: "wss://voice.example.org", reference: "未设置" },
      { key: "h5ReadyCheckEnabled", label: appConfigFieldLabel("h5ReadyCheckEnabled"), value: "关闭", reference: "开启" },
    ]);
  });

  it("并列值对每台都标记差异，说明没有多数值", () => {
    const first = configuration();
    const second = configuration();
    second.environments.test.h5Url = "https://screen.example.org";
    const summaries = summarizeAppConfigs([row(first), row(second, "screen-2")], "test");
    expect(appConfigDifferences(first, summaries, "test")).toEqual([
      { key: "h5Url", label: appConfigFieldLabel("h5Url"), value: "未设置", reference: "多个值，无多数值" },
    ]);
    expect(appConfigDifferences(second, summaries, "test")[0]).toMatchObject({ value: "https://screen.example.org", reference: "多个值，无多数值" });
  });
});

describe("直接输入配置修改", () => {
  it("改回代表值后恢复不修改，即使其它屏的当前值不同", () => {
    const baseline = summarizeAppConfigValues(["https://a.example.org", "https://a.example.org", "https://b.example.org"]);
    expect(appConfigTextEdit("https://c.example.org", baseline, true)).toEqual({ mode: "set", value: "https://c.example.org" });
    expect(appConfigTextEdit("https://a.example.org", baseline, true)).toEqual({ mode: "keep", value: "https://a.example.org" });
  });

  it("可清空字段删除原值时明确清空，原本为空则不修改", () => {
    expect(appConfigTextEdit("", summarizeAppConfigValues(["wss://voice.example.org"]), true)).toEqual({ mode: "clear", value: "" });
    expect(appConfigTextEdit("", summarizeAppConfigValues([null]), true)).toEqual({ mode: "keep", value: "" });
  });

  it("必填字段删空保留为待校验的设置，不转成清空", () => {
    expect(appConfigTextEdit("", summarizeAppConfigValues(["https://config.example.org"]), false)).toEqual({ mode: "set", value: "" });
  });

  it("比较使用与提交一致的去空格值，不因只加空格而统一少数屏", () => {
    const baseline = summarizeAppConfigValues(["https://a.example.org", "https://a.example.org", "https://b.example.org"]);
    expect(appConfigTextEdit(" https://a.example.org ", baseline, false)).toEqual({ mode: "keep", value: " https://a.example.org " });
    expect(appConfigTextEdit("   ", baseline, true)).toEqual({ mode: "clear", value: "   " });
    expect(appConfigTextEdit("   ", baseline, false)).toEqual({ mode: "set", value: "   " });
    expect(appConfigTextEdit("   ", summarizeAppConfigValues([null]), true)).toEqual({ mode: "keep", value: "   " });
  });

  it("并列值由用户实际输入决定修改；主动输入空值可明确清空", () => {
    const baseline = summarizeAppConfigValues([null, "wss://voice.example.org"]);
    expect(appConfigTextEdit("", baseline, true)).toEqual({ mode: "clear", value: "" });
    expect(appConfigTextEdit("wss://voice.example.org", baseline, true)).toEqual({ mode: "set", value: "wss://voice.example.org" });
  });
});
