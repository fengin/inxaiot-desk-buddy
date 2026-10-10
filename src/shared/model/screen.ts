import type { ScreenSpaceNode } from "./screenSpace";
import { getProjectSpacePath } from "./projectSpace";
import type { ScreenPlatformDraft, ScreenRegistrationExecutionRecord } from "./screenRegistration";
import type { ScreenVersionSyncRecord } from "./screenMaintenance";

export type ScreenSize = "4" | "10" | "unknown";
export type ScreenOnline = "online" | "offline" | "unknown";
export type ScreenAction = "install" | "ping" | "inspect" | "mac" | "time" | "ntp" | "reboot" | "restart" | "adb" | "diagnostics" | "register" | "app_config";
export type ScreenTargetState = "queued" | "running" | "succeeded" | "failed" | "cancelled" | "needs_review";

export interface SmartScreen {
  id: string;
  source: "platform" | "local";
  name: string;
  ip: string;
  mac: string;
  observedMac: string;
  size: ScreenSize;
  spaceId?: string | null;
  spacePath?: string;
  /** 从实际空间祖先派生的显示缓存；不是独立空间关联。 */
  building: string;
  floor: string;
  buildingId?: string;
  floorId?: string;
  location: string;
  platformStatus: ScreenOnline;
  revision: number;
  ping: ScreenOnline | null;
  checkedAt: string | null;
  appVersion: string | null;
  observedAppVersion?: string | null;
  versionCheckedAt?: string;
  versionCheckedIp?: string;
  android: string;
  abi: string;
  appVersionCode?: number | null;
  adbAvailable: boolean | null;
  adbStatus?: "uninspected" | "available" | "unauthorized" | "unavailable";
  adbCheckedAt?: string;
  persistentAdb: boolean | null;
  freeSpaceMb: number | null;
  clockOffsetSeconds: number | null;
  deviceModel?: string | null;
  firmware?: string | null;
  macSource?: string | null;
  macCandidates?: string[];
  macCheckedAt?: string;
  appInstalled?: boolean | null;
  appRunning?: boolean | null;
  deviceTime?: string | null;
  computerTime?: string | null;
  timezone?: string | null;
  automaticTime?: boolean | null;
  automaticTimezone?: boolean | null;
  inspectionErrors?: string[];
  aliases: string[];
}

export interface LocalScreenInput {
  name: string;
  ip: string;
  size: ScreenSize;
  mac: string;
  spaceId?: string | null;
  spacePath?: string;
  /** 仅兼容旧原型数据；新输入使用 spaceId。 */
  building?: string;
  floor?: string;
  buildingId?: string;
  floorId?: string;
  location: string;
}

export interface ScreenApkSelection {
  name: string;
  size: number;
  lastModified: number;
  path?: string;
  sha256?: string;
  packageId?: string;
  appVersion?: string;
  appVersionCode?: number;
  minSdk?: number;
  abis?: string[];
  activity?: string;
}

/** 原型只校验用户所选文件的基本信息，不把后缀检查当作 APK 内容校验。 */
export function validateScreenApk(apk?: ScreenApkSelection): string[] {
  if (!apk) return ["请先选择本地 APK 文件"];
  const errors: string[] = [];
  if (!/\.apk$/i.test(apk.name)) errors.push("请选择 .apk 格式的应用包");
  if (!Number.isFinite(apk.size) || apk.size <= 0) errors.push("应用包不能为空文件");
  return errors;
}

/** 安装批次只允许同一种明确尺寸；尺寸规则不代替 APK 和设备架构兼容性检查。 */
export function screenInstallSizeWarning(screens: readonly Pick<SmartScreen, "size">[]): string {
  const knownSizes = new Set(screens.map((screen) => screen.size).filter((size) => size === "4" || size === "10"));
  if (knownSizes.size > 1) return "一批只能操作同规格尺寸屏";
  if (screens.some((screen) => screen.size !== "4" && screen.size !== "10")) return "请先确认所选屏尺寸";
  return "";
}

export interface ScreenOperationInput {
  retryOfOperationId?: string;
  action: ScreenAction;
  targetIds: string[];
  applicationId?: "xiaoxin";
  apk?: ScreenApkSelection;
  /** 原型本地 APK 尚未解析元数据时为空，不可据此更新版本记录。 */
  appVersion: string;
  abi: "universal" | "arm64-v8a" | "armeabi-v7a";
  reinstall: boolean;
  concurrency: number;
  /** 确认时的实际连接目标；草稿不参与设备寻址。 */
  expectedTargets?: Record<string, string>;
}

export interface ScreenPreflightItem {
  screenId: string;
  name: string;
  ip: string;
  state: "ready" | "blocked" | "skip";
  reason: string;
  observation?: ScreenInspection | null;
}

/** 后端已保存的设备读取结果；未取得的字段保留空值。 */
export interface ScreenInspection {
  operationType?: string;
  id: string; observedIp: string; observedAt: string;
  ping?: ScreenOnline | null;
  adbAvailable: boolean; android?: string | null; abis: string[];
  deviceModel?: string | null; firmware?: string | null;
  observedMac?: string | null; macSource?: string | null; macCandidates?: string[];
  observedAppVersion?: string | null; appVersionCode?: number | null;
  appInstalled?: boolean | null; appRunning?: boolean | null;
  freeSpaceMb?: number | null; clockOffsetSeconds?: number | null;
  persistentAdb?: boolean | null; deviceTime?: string | null; computerTime?: string | null;
  timezone?: string | null; automaticTime?: boolean | null; automaticTimezone?: boolean | null;
  errors: string[];
}

export interface ScreenTaskTarget {
  screenId: string;
  name: string;
  ip: string;
  state: ScreenTargetState;
  progress: number;
  message: string;
  result?: {
    device: "not_required" | "pending" | "succeeded" | "failed" | "unknown" | "skipped" | "cancelled";
    business: "not_required" | "pending" | "succeeded" | "failed" | "unknown" | "skipped" | "cancelled";
    shared: "not_required" | "pending" | "succeeded" | "failed" | "unknown" | "skipped" | "cancelled";
    observation?: ScreenInspection | null;
    evidence?: Record<string, unknown>;
  };
}

export interface ScreenTask {
  mode?: "prototype" | "real";
  id: string;
  projectId: string;
  action: ScreenAction | "status" | "version_sync" | "merge";
  state: "running" | "cancelling" | "succeeded" | "partially_succeeded" | "failed" | "cancelled" | "needs_review";
  createdAt: string;
  updatedAt: string;
  targets: ScreenTaskTarget[];
  logs: { time: string; level: "INFO" | "WARN" | "ERROR"; message: string }[];
  input?: ScreenOperationInput;
  registration?: { previewId: string; records: ScreenRegistrationExecutionRecord[] };
  versionSync?: { previewId: string; records: ScreenVersionSyncRecord[] };
}

export interface ScreenSnapshot {
  mode?: "prototype" | "real";
  localOnly?: boolean;
  businessProjectId?: string | null;
  availableProjects?: { id: string; name: string }[];
  platformMessage?: string | null;
  screens: SmartScreen[];
  spaces: ScreenSpaceNode[];
  spacesAvailable?: boolean;
  tasks: ScreenTask[];
  ignoredPairs: string[];
  platformAvailable: boolean;
  mergeHistory?: ScreenMergeAudit[];
  platformDrafts?: Record<string, ScreenPlatformDraft>;
  platformReadAt?: string;
}

export interface ScreenMergeCandidate {
  key: string;
  local: SmartScreen;
  platform: SmartScreen;
  matchIp: boolean;
  matchMac: boolean;
  conflict: boolean;
}

export type ScreenMergeSource = "local" | "platform";
export type ScreenMergeField = "name" | "ip" | "mac" | "size" | "space" | "location" | "appVersion";
export type ScreenMergeChoices = Record<ScreenMergeField, ScreenMergeSource>;
export type ScreenMergeFields = LocalScreenInput & { appVersion: string | null };
export type ScreenMergeDecision = { kind: "ignore" } | { kind: "merge"; choices: ScreenMergeChoices; identityConfirmed: boolean };
export interface ScreenMergeResult { localId: string; platformId: string; fields: ScreenMergeFields; taskId?: string; resultPending?: boolean }
export interface ScreenMergeAudit extends ScreenMergeResult {
  time: string;
  choices: ScreenMergeChoices;
  beforeLocal: SmartScreen;
  beforePlatform: SmartScreen;
}

export const screenMergeFields: { key: ScreenMergeField; label: string }[] = [
  { key: "name", label: "名称" }, { key: "ip", label: "IP 地址" }, { key: "mac", label: "MAC 地址" },
  { key: "size", label: "尺寸" }, { key: "space", label: "所在空间" },
  { key: "location", label: "详细位置" }, { key: "appVersion", label: "小新版本" }
];

export function createScreenMergeChoices(source: ScreenMergeSource = "platform"): ScreenMergeChoices {
  return Object.fromEntries(screenMergeFields.map(({ key }) => [key, source])) as ScreenMergeChoices;
}

export function screenMergeSourceValue(candidate: ScreenMergeCandidate, source: ScreenMergeSource, field: ScreenMergeField): string | null {
  const screen = candidate[source];
  if (field === "mac") return source === "local" ? effectiveScreenMac(screen) : screen.mac;
  if (field === "appVersion") return source === "local" ? screen.observedAppVersion ?? screen.appVersion : screen.appVersion;
  if (field === "space") return screen.spacePath || (screen.spaceId ? `空间节点 ${screen.spaceId}` : "");
  return screen[field];
}

export function buildScreenMergeFields(candidate: ScreenMergeCandidate, choices: ScreenMergeChoices): ScreenMergeFields {
  for (const { key } of screenMergeFields) {
    if (choices[key] !== "local" && choices[key] !== "platform") throw new Error("每个合并字段必须选择本机记录或平台记录");
  }
  const value = (key: ScreenMergeField) => screenMergeSourceValue(candidate, choices[key], key);
  return {
    name: value("name")!.trim(), ip: value("ip")!.trim(), mac: (value("mac") ?? "").trim(), size: value("size") as ScreenSize,
    spaceId: candidate[choices.space].spaceId ?? null, spacePath: candidate[choices.space].spacePath ?? "",
    location: value("location") ?? "", appVersion: value("appVersion")
  };
}

export function validateScreenMergeFields(fields: ScreenMergeFields, spaces: readonly ScreenSpaceNode[], available = true): string[] {
  const errors = validateLocalScreen(fields);
  if (!fields.name) errors.push("合并后的名称不能为空");
  const mac = normalizeScreenMac(fields.mac);
  if (fields.mac && /^(0{12}|F{12})$/.test(mac)) errors.push("MAC 不能使用全零或全 F 的占位值");
  if (fields.spaceId && !available) errors.push("空间目录暂不可用，请恢复后重新核对");
  else if (fields.spaceId && !getProjectSpacePath(spaces, fields.spaceId)) errors.push("所选空间已失效，请调整空间来源或先修正原记录");
  return errors;
}

export interface ScreenStatusChange {
  id: string;
  ip: string;
  expected: ScreenOnline;
  next: ScreenOnline;
  revision: number;
}

export interface ScreenStatusResult { id: string; name: string; ok: boolean; message: string }

export const screenActions: { value: ScreenAction; label: string; description: string }[] = [
  { value: "install", label: "安装/升级小新", description: "安装/升级端屏的智能小新应用" },
  { value: "register", label: "注册/更新到平台", description: "登记新屏或更新平台资料" },
  { value: "app_config", label: "修改小新配置", description: "读取和修改小新配置，按需重启并回读" },
  { value: "time", label: "校准时间", description: "以当前电脑时间校准并回读偏差" },
  { value: "ntp", label: "设置NTP服务器", description: "读取和修改网络授时地址，启用自动校时并验证生效" },
  { value: "ping", label: "检查屏在离线", description: "检查屏IP连通性" },
  { value: "inspect", label: "检查设备", description: "读取管理连接、系统、应用" },
  { value: "mac", label: "获取/核对 MAC", description: "采集网卡地址，核对设备身份" },
  { value: "adb", label: "保持ADB端口", description: "10 寸屏重启仍保持 5555端口" },
  { value: "restart", label: "重启小新应用", description: "保留应用数据，只恢复小新应用" },
  { value: "reboot", label: "重启屏", description: "重启智能屏系统" },
  { value: "diagnostics", label: "采集诊断", description: "采集屏信息和应用检查信息" }
];

export const screenActionLabel = (action: ScreenTask["action"]) => action === "status" ? "覆盖平台状态" : action === "version_sync" ? "同步平台应用版本" : action === "merge" ? "合并屏记录" : screenActions.find((item) => item.value === action)?.label ?? action;
export const screenStateLabel: Record<ScreenTask["state"] | ScreenTargetState, string> = {
  queued: "等待执行", running: "执行中", cancelling: "取消中", succeeded: "已成功",
  partially_succeeded: "部分成功", failed: "失败", cancelled: "已取消", needs_review: "结果待核实"
};
export const normalizeScreenMac = (value: string) => value.replace(/[:-]/g, "").toUpperCase();
export const effectiveScreenMac = (screen: SmartScreen) => screen.source === "platform" ? screen.mac || screen.observedMac : screen.observedMac || screen.mac;
export const screenTaskActive = (task: ScreenTask) => ["running", "cancelling", "needs_review"].includes(task.state);

export function screenMergeCandidates(snapshot: ScreenSnapshot): ScreenMergeCandidate[] {
  return snapshot.screens.filter((s) => s.source === "local").flatMap((local) =>
    snapshot.screens.filter((s) => s.source === "platform").flatMap((platform) => {
      const left = normalizeScreenMac(effectiveScreenMac(local));
      const right = normalizeScreenMac(effectiveScreenMac(platform));
      const matchIp = local.ip === platform.ip;
      const validMac = (value: string) => /^[0-9A-F]{12}$/.test(value) && !/^(0{12}|F{12})$/.test(value);
      const matchMac = validMac(left) && validMac(right) && left === right;
      const key = `${local.id}|${platform.id}|${local.ip}|${platform.ip}|${left}|${right}`;
      return (matchIp || matchMac) && !snapshot.ignoredPairs.includes(key)
        ? [{ key, local, platform, matchIp, matchMac, conflict: matchIp && validMac(left) && validMac(right) && left !== right }]
        : [];
    })
  );
}

export function validateLocalScreen(input: LocalScreenInput): string[] {
  const errors: string[] = [];
  if (!/^(\d{1,3}\.){3}\d{1,3}$/.test(input.ip) || input.ip.split(".").some((part) => Number(part) > 255)) errors.push("请输入有效的 IPv4 地址");
  if (input.mac && !/^[0-9A-F]{12}$/.test(normalizeScreenMac(input.mac))) errors.push("MAC 应包含 12 位十六进制字符");
  if (!["4", "10", "unknown"].includes(input.size)) errors.push("尺寸请选择 4 寸、10 寸或待确认");
  if ([...input.name].length > 32) errors.push("名称不能超过 32 个字符");
  if ([...input.location].length > 128) errors.push("安装位置不能超过 128 个字符");
  return errors;
}
