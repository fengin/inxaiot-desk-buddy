const SOURCE_DATE_TIME = /^(\d{4}-\d{2}-\d{2})[T\s](\d{1,2}:\d{2}(?::\d{2})?)(?:[.,]\d+)?(?:\s*(?:Z|[+-]\d{2}(?::?\d{2})?(?::\d{2})?))?$/i;
const SOURCE_DATE_TIME_IN_TEXT = /(\d{4}-\d{2}-\d{2})[T\s](\d{1,2}:\d{2}(?::\d{2})?)(?:[.,]\d+)?(?:\s*(?:Z|[+-]\d{2}(?::?\d{2})?(?::\d{2})?))?/gi;
const EPOCH = /^\d{10,}$/;

function pad(value: number) {
  return String(value).padStart(2, "0");
}

function localEpochDateTime(value: string): string | undefined {
  if (!EPOCH.test(value)) return undefined;
  const milliseconds = value.length >= 19
    ? Number(value.slice(0, -6))
    : value.length >= 16
      ? Number(value.slice(0, -3))
      : value.length >= 13
        ? Number(value)
        : Number(value) * 1000;
  const date = new Date(milliseconds);
  if (Number.isNaN(date.getTime())) return undefined;
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function normalizeSourceDateTime(value: string): string | undefined {
  const match = value.match(SOURCE_DATE_TIME);
  if (!match) return undefined;
  const [hour, minute, second = "00"] = match[2]!.split(":");
  return `${match[1]} ${hour!.padStart(2, "0")}:${minute}:${second}`;
}

/**
 * 统一用户可见日期时间格式。带时区的文本保留其原始钟点，只去掉 ISO 分隔符、
 * 小数秒和时区后缀；纯 epoch 按本机时区显示，以保持任务与日志的既有时间语义。
 */
export function formatDisplayDateTime(value: string | null | undefined, fallback = "—") {
  const source = value?.trim();
  if (!source) return fallback;
  return normalizeSourceDateTime(source) ?? localEpochDateTime(source) ?? source;
}

/** 格式化夹在用户文案中的源日期，例如“导入一体机清单 · 2026-09-03T10:44:44”。 */
export function formatDisplayDateTimesInText(value: string) {
  return value.replace(SOURCE_DATE_TIME_IN_TEXT, (_matched, date: string, time: string) => {
    const [hour, minute, second = "00"] = time.split(":");
    return `${date} ${hour.padStart(2, "0")}:${minute}:${second}`;
  });
}
