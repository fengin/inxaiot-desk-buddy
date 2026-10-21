const DATE_TIME_PATTERN = String.raw`(\d{4})-(\d{2})-(\d{2})[T\s](\d{1,2}):(\d{2})(?::(\d{2})(?:[.,](\d+))?)?(?:\s*(Z|[+-]\d{2}(?::?\d{2})?(?::\d{2})?))?`;
const SOURCE_DATE_TIME = new RegExp(`^${DATE_TIME_PATTERN}$`, "i");
const SOURCE_DATE_TIME_IN_TEXT = new RegExp(DATE_TIME_PATTERN, "gi");
const EPOCH = /^-?\d+$/;

function pad(value: number) {
  return String(value).padStart(2, "0");
}

function localDateTime(milliseconds: number): string | undefined {
  const date = new Date(milliseconds);
  if (Number.isNaN(date.getTime())) return undefined;
  return `${String(date.getFullYear()).padStart(4, "0")}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

function localEpochDateTime(value: string, numeric: boolean): string | undefined {
  const digits = value.replace(/^-/, "").length;
  if (!EPOCH.test(value) || (!numeric && digits < 10)) return undefined;
  // 保留现有秒、毫秒、微秒和纳秒来源的位数约定；BigInt 避免纳秒先转浮点丢失精度。
  const epoch = BigInt(value);
  const divisor = digits >= 19 ? 1_000_000n : digits >= 16 ? 1_000n : 1n;
  const milliseconds = digits < 13 ? epoch * 1_000n
    : epoch / divisor - (epoch < 0n && epoch % divisor !== 0n ? 1n : 0n);
  return localDateTime(Number(milliseconds));
}

function normalizeSourceDateTime(value: string): string | undefined {
  const match = value.match(SOURCE_DATE_TIME);
  if (!match) return undefined;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText = "00", fraction = "", zone] = match;
  const [year, month, day, hour, minute, second] = [yearText, monthText, dayText, hourText, minuteText, secondText].map(Number);
  const date = new Date(0);
  // setUTCFullYear 支持 0000–0099 年，不采用 Date.UTC 的 1900 年偏移规则。
  date.setUTCFullYear(year!, month! - 1, day);
  date.setUTCHours(hour!, minute!, second!, Number(fraction.slice(0, 3).padEnd(3, "0")));
  if (date.getUTCFullYear() !== year || date.getUTCMonth() + 1 !== month || date.getUTCDate() !== day
    || date.getUTCHours() !== hour || date.getUTCMinutes() !== minute || date.getUTCSeconds() !== second) return undefined;

  // 没有时区的值只表达来源墙上时间，不能推断为 UTC 或让 Date 猜测当地夏令时。
  if (!zone) return `${yearText}-${monthText}-${dayText} ${pad(hour!)}:${minuteText}:${secondText}`;
  if (zone.toUpperCase() === "Z") return localDateTime(date.getTime());
  const offset = zone.match(/^([+-])(\d{2})(?::?(\d{2}))?(?::(\d{2}))?$/);
  if (!offset) return undefined;
  const hours = Number(offset[2]), minutes = Number(offset[3] ?? 0), seconds = Number(offset[4] ?? 0);
  if (hours > 23 || minutes > 59 || seconds > 59) return undefined;
  const offsetSeconds = (hours * 3_600 + minutes * 60 + seconds) * (offset[1] === "+" ? 1 : -1);
  return localDateTime(date.getTime() - offsetSeconds * 1_000);
}

/**
 * 时间瞬间（epoch、带 Z/偏移的 ISO 或数据库文本）统一按电脑当地时区显示。
 * 无时区文本只规范原年月日钟点；未知或非法文本原样保留，不猜测其时区。
 */
export function formatDisplayDateTime(value: string | number | bigint | null | undefined, fallback = "—") {
  const source = value == null ? "" : String(value).trim();
  if (!source) return fallback;
  return normalizeSourceDateTime(source) ?? localEpochDateTime(source, typeof value !== "string") ?? source;
}

/** 文案中的日期沿用相同语义，例如无时区的导入标题保留原钟点。 */
export function formatDisplayDateTimesInText(value: string) {
  return value.replace(SOURCE_DATE_TIME_IN_TEXT, (matched: string, ...args: unknown[]) => {
    const position = args.at(-2) as number;
    const following = value.slice(position + matched.length);
    // 不把非法日期/时区的前半段当成一个有效时间替换。
    if (/[\dA-Za-z]/.test(value[position - 1] ?? "") || /^(?:[\dA-Za-z:+-]|[.,]\d)/.test(following)
      || /^\s+(?:[+-]\d|[A-Z]{2,5}\b)/i.test(following)) return matched;
    return formatDisplayDateTime(matched);
  });
}
