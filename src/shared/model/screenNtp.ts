export interface ScreenNtpConfig {
  /** 空值表示没有自定义地址，系统使用固件默认服务器。 */
  server: string;
  autoTime: boolean;
  autoTimeZone: boolean;
  timeZone: string;
}

export interface ScreenNtpRead {
  screenId: string;
  readAt: string;
  config?: ScreenNtpConfig | null;
  capabilities?: { activation: string; rebootRequired: boolean } | null;
  message: string;
}

export interface ScreenNtpPatch { server: string }
export interface ScreenNtpEvidence {
  before?: ScreenNtpConfig;
  after?: ScreenNtpConfig;
  targetServer: string;
  save: string;
  activation: string;
  sync: string;
  rebootRequired: boolean;
  syncEvidence?: { server?: string | null; clockOffsetSeconds?: number | null; cacheAgeMillis?: number; sourceConfirmedBy?: string | null; sourceType?: string };
  message?: string;
}

export function validateNtpServer(server: string): string {
  const value = server.trim();
  if (!value) return "";
  if (value.length > 253 || /[\s/?#@]/.test(value)) return "请输入 IP 地址或主机名，不包含协议、端口或路径";
  if (value.includes(":")) {
    try { if (new URL(`http://[${value}]/`).hostname.startsWith("[")) return ""; } catch { /* 原始 IPv6 地址可以包含冒号，主机名与端口不可以。 */ }
    return "请输入有效的 IPv6 地址，或不带端口的 IP 地址、主机名";
  }
  if (/^[\d.]+$/.test(value)) {
    return /^(\d{1,3}\.){3}\d{1,3}$/.test(value) && value.split(".").every(part => Number(part) <= 255 && (part === "0" || !part.startsWith("0"))) ? "" : "请输入有效的 IPv4 地址";
  }
  return value.replace(/\.$/, "").split(".").every(part => /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(part)) ? "" : "请输入有效的服务器主机名";
}

export const ntpServerDisplay = (server?: string | null) => server ? server : "未设置（固件默认）";
export const ntpStageLabel = (state?: string) => ({ succeeded: "已确认", failed: "未完成", unknown: "待核实", pending: "等待处理", not_started: "未开始", not_required: "无需执行", unchanged: "无需写入" })[state ?? "pending"] ?? "待核实";
