import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { ModuleKind, ScriptTarget, transpileModule } from "typescript";
import { describe, expect, it } from "vitest";

import { formatDisplayDateTime, formatDisplayDateTimesInText } from "@/shared/format/dateTime";

// 独立进程使用真正的当地 Date/DST 规则，不修改 Vitest worker 的 TZ，也不依赖开发机时区。
const formatterModule = transpileModule(readFileSync(resolve("src/shared/format/dateTime.ts"), "utf8"), {
  compilerOptions: { module: ModuleKind.ESNext, target: ScriptTarget.ES2022 }
}).outputText;
function inTimeZone<T>(timeZone: string, expression: string): T {
  return JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e",
    `${formatterModule}\nprocess.stdout.write(JSON.stringify(${expression}));`
  ], { env: { ...process.env, TZ: timeZone }, encoding: "utf8" })) as T;
}

describe("display date time", () => {
  it.each([
    ["UTC", "2026-10-21 05:39:11"],
    ["Asia/Shanghai", "2026-10-21 13:39:11"],
    ["America/New_York", "2026-10-21 01:39:11"],
    ["Asia/Kathmandu", "2026-10-21 11:24:11"]
  ])("同一瞬间的本机 epoch、共享数据库文本和 ISO 在 %s 一致", (zone, expected) => {
    const values = inTimeZone<string[]>(zone, `(() => {
      const milliseconds = Date.UTC(2026, 9, 21, 5, 39, 11);
      return [
        String(milliseconds / 1000), milliseconds, String(milliseconds),
        String(BigInt(milliseconds) * 1000n), String(BigInt(milliseconds) * 1000000n + 999999n),
        BigInt(milliseconds) * 1000000n,
        '2026-10-21T05:39:11.999999999Z', '2026-10-21 5:39:11.0 +00:00:00',
        '2026-10-21T13:39:11+08:00', '2026-10-21T13:39:11+08',
        '2026-10-21T11:09:11+0530', '2026-10-21T02:09:11-03:30',
        '2026-10-21 5:39:41.9 +00:00:30'
      ].map(value => formatDisplayDateTime(value));
    })()`);
    expect(values).toEqual(Array(13).fill(expected));
  });

  it("保留无时区来源的墙上时间，不推断 UTC 或当地 DST", () => {
    expect(formatDisplayDateTime("2026-09-03T10:44:44")).toBe("2026-09-03 10:44:44");
    expect(formatDisplayDateTime("2026-08-25 18:42")).toBe("2026-08-25 18:42:00");
    expect(formatDisplayDateTime("2024-02-29 4:42:10,665234")).toBe("2024-02-29 04:42:10");
    expect(inTimeZone("America/New_York", `['2026-03-08T02:30:00', '2026-11-01T01:30:00'].map(value => formatDisplayDateTime(value))`))
      .toEqual(["2026-03-08 02:30:00", "2026-11-01 01:30:00"]);
    expect(formatDisplayDateTimesInText("导入一体机清单 · 2026-09-03T10:44:44")).toBe("导入一体机清单 · 2026-09-03 10:44:44");
  });

  it("跨越纽约夏令时开始与结束时按该瞬间选择偏移", () => {
    expect(inTimeZone("America/New_York", `[
      '2026-03-08T06:59:59Z', '2026-03-08T07:00:00Z',
      '2026-11-01T05:59:59Z', '2026-11-01T06:00:00Z'
    ].map(value => formatDisplayDateTime(value))`)).toEqual([
      "2026-03-08 01:59:59", "2026-03-08 03:00:00",
      "2026-11-01 01:59:59", "2026-11-01 01:00:00"
    ]);
    expect(inTimeZone("Australia/Lord_Howe", `['2026-10-03T15:29:59Z', '2026-10-03T15:30:00Z'].map(value => formatDisplayDateTime(value))`))
      .toEqual(["2026-10-04 01:59:59", "2026-10-04 02:30:00"]);
  });

  it("转换会正确跨日、跨年及进入闰日", () => {
    expect(inTimeZone("UTC", `['2026-01-01T00:15:00+14:00', '2024-03-01T00:30:00+01:00'].map(value => formatDisplayDateTime(value))`))
      .toEqual(["2025-12-31 10:15:00", "2024-02-29 23:30:00"]);
    expect(inTimeZone("Pacific/Kiritimati", `formatDisplayDateTime('2025-12-31T10:15:00Z')`)).toBe("2026-01-01 00:15:00");
    expect(inTimeZone("America/Los_Angeles", `formatDisplayDateTime('2026-01-01T02:15:00Z')`)).toBe("2025-12-31 18:15:00");
  });

  it("文案中的明确时区按当地转换，无时区日期保留原钟点", () => {
    expect(inTimeZone("Asia/Shanghai", `formatDisplayDateTimesInText('记录：2026-10-21T05:39:11Z；数据库 2026-10-21 5:39:11.0 +00:00:00；导入 2026-10-21T05:39:11。')`))
      .toBe("记录：2026-10-21 13:39:11；数据库 2026-10-21 13:39:11；导入 2026-10-21 05:39:11。");
  });

  it.each([
    "2026-02-29T10:00:00Z", "2026-02-30T10:00:00Z", "2026-13-01T10:00:00Z",
    "2026-10-21T24:00:00Z", "2026-10-21T10:60:00Z", "2026-10-21T10:00:60Z",
    "2026-10-21T10:00:00+24:00", "2026-10-21T10:00:00+00:60", "2026-10-21T10:00:00+00:00:60",
    "2026-10-21T10:00:00+8:00", "2026-10-21T10:00:00+080", "2026-10-21T10:00:00ZZ",
    "2026-10-21T10:00:00 CST", "2026-10-21T10:00:00 cst", "2026-10-21 05:39.5", "2026-10-21 05:39:11.123.4",
    "October 21, 2026 10:00:00", "2026", "99999999999999999999999999999999"
  ])("未知或非法来源保留原文，不让 Date 自动修正：%s", value => {
    expect(formatDisplayDateTime(value)).toBe(value);
    expect(formatDisplayDateTimesInText(`记录：${value}；`)).toBe(`记录：${value}；`);
  });

  it("支持数值零和负 epoch，并保留纳秒在秒边界的精度", () => {
    expect(inTimeZone("UTC", `[0, '-1000000000000000001'].map(value => formatDisplayDateTime(value))`))
      .toEqual(["1970-01-01 00:00:00", "1938-04-24 22:13:19"]);
  });

  it("keeps user-facing status text and accepts a caller fallback", () => {
    expect(formatDisplayDateTime("尚未注册")).toBe("尚未注册");
    expect(formatDisplayDateTime(undefined, "尚未完成")).toBe("尚未完成");
    expect(formatDisplayDateTime(null)).toBe("—");
    expect(formatDisplayDateTime("  ", "尚未读取")).toBe("尚未读取");
  });
});
