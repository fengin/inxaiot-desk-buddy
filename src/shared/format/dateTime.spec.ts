import { describe, expect, it, vi } from "vitest";

import { formatDisplayDateTime, formatDisplayDateTimesInText } from "@/shared/format/dateTime";

describe("display date time", () => {
  it("normalizes source timestamp text without changing its clock", () => {
    expect(formatDisplayDateTime("2026-09-03T10:57:59.6737586Z")).toBe("2026-09-03 10:57:59");
    expect(formatDisplayDateTime("2026-09-03T10:44:44")).toBe("2026-09-03 10:44:44");
    expect(formatDisplayDateTime("2026-08-25 18:42")).toBe("2026-08-25 18:42:00");
    expect(formatDisplayDateTime("2026-09-03 4:42:10.665234 +00:00:00")).toBe("2026-09-03 04:42:10");
    expect(formatDisplayDateTimesInText("导入一体机清单 · 2026-09-03T10:44:44")).toBe("导入一体机清单 · 2026-09-03 10:44:44");
  });

  it("formats epoch values in the local task and log time zone", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-03T10:00:00Z"));
    expect(formatDisplayDateTime("1756893479673758600")).toMatch(/^2025-09-03 \d{2}:57:59$/);
    vi.useRealTimers();
  });

  it("keeps user-facing status text and accepts a caller fallback", () => {
    expect(formatDisplayDateTime("尚未注册")).toBe("尚未注册");
    expect(formatDisplayDateTime(undefined, "尚未完成")).toBe("尚未完成");
  });
});
