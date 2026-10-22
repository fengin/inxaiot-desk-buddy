import { effectiveScreenMac, normalizeScreenMac, screenTaskActive, validateLocalScreen } from "./screen";
import type { ScreenSize, ScreenSnapshot, SmartScreen } from "./screen";
import { getProjectSpacePath, projectSpacePath } from "./projectSpace";

/** 可人工编辑的登记资料；在线状态和设备读取的小新版本不属于资料草稿。 */
export interface ScreenPlatformFields {
  name: string;
  ip: string;
  mac: string;
  size: ScreenSize;
  spaceId: string | null;
  location: string;
}

/** 已注册屏待提交资料，与平台屏记录分开保存，不生成本机资产行。 */
export interface ScreenPlatformDraft {
  revision?: number;
  screenId: string;
  baseRevision: number;
  base: ScreenPlatformFields;
  values: ScreenPlatformFields;
  updatedAt: string;
}

export type ScreenRegistrationMacConfirmation = "existing" | "empty";
export type ScreenPlatformField = keyof ScreenPlatformFields;
export type ScreenRegistrationField = ScreenPlatformField | "appVersion";
export interface ScreenRegistrationDiff {
  field: ScreenRegistrationField;
  label: string;
  before: string;
  after: string;
}

export interface ScreenRegistrationPreviewItem {
  screenId: string;
  mode: "create" | "update";
  state: "ready" | "blocked" | "skip";
  reason: string;
  before: ScreenPlatformFields | null;
  after: ScreenPlatformFields;
  diffs: ScreenRegistrationDiff[];
  /** 平台当前版本与本次设备核对后的拟提交版本；不能由编辑资料表单指定。 */
  platformAppVersion?: string | null;
  proposedAppVersion?: string | null;
  versionMessage?: string | null;
  expectedRevision: number;
  needsSpaceConfirmation: boolean;
  macSource: "collected" | "unchanged" | "history" | "empty" | "conflict";
  macMessage: string;
  /** 采集失败时必须逐屏确认；冲突项始终 blocked，不能用空 MAC 绕过。 */
  requiredMacConfirmation: ScreenRegistrationMacConfirmation | null;
  /** 重复候选仅供跳转现有合并流程，不能继续新登记。 */
  duplicateIds: string[];
}

export interface ScreenRegistrationPreview {
  mode?: "prototype" | "real";
  id: string;
  projectId: string;
  createdAt: string;
  items: ScreenRegistrationPreviewItem[];
}

export interface ScreenRegistrationSubmission {
  previewId: string;
  screenIds: string[];
  macConfirmations?: Record<string, ScreenRegistrationMacConfirmation>;
  spaceConfirmations?: string[];
}

/** 保存提交时的资料与回读依据；重载后只核实，不重放创建。 */
export interface ScreenRegistrationExecutionRecord {
  screenId: string;
  mode: "create" | "update";
  before: ScreenPlatformFields | null;
  after: ScreenPlatformFields;
  platformAppVersion?: string | null;
  proposedAppVersion?: string | null;
  platformId?: string;
  sourceFingerprint: string;
}

export const screenPlatformFieldLabels: Record<ScreenPlatformField, string> = {
  name: "名称", ip: "IP 地址", mac: "MAC 地址", size: "屏尺寸", spaceId: "所在空间", location: "详细位置"
};
const fields = Object.keys(screenPlatformFieldLabels) as ScreenPlatformField[];
export const validRegistrationMac = (value: string) => {
  const mac = normalizeScreenMac(value.trim());
  return /^[0-9A-F]{12}$/.test(mac) && !/^(0{12}|F{12})$/.test(mac) && (Number.parseInt(mac.slice(0, 2), 16) & 1) === 0;
};

/** 只复制可编辑资料；设备读取版本由独立核对结果提交，不能混入草稿。 */
export function screenPlatformFields(screen: Pick<SmartScreen, "name" | "ip" | "mac" | "size" | "spaceId" | "location">): ScreenPlatformFields {
  return { name: screen.name.trim(), ip: screen.ip.trim(), mac: screen.mac.trim(), size: screen.size, spaceId: screen.spaceId || null, location: screen.location.trim() };
}

/** MAC 按身份比较；其余字段沿用资料保存的首尾空白与空空间规则。 */
export function screenPlatformFieldEqual(field: ScreenPlatformField, left: string | null | undefined, right: string | null | undefined): boolean {
  const a = (left ?? "").trim(), b = (right ?? "").trim();
  return field === "mac" ? normalizeScreenMac(a) === normalizeScreenMac(b) : a === b;
}

export function screenPlatformDraftFieldEdited(draft: ScreenPlatformDraft | undefined, field: ScreenPlatformField): boolean {
  return Boolean(draft && !screenPlatformFieldEqual(field, draft.values[field], draft.base[field]));
}

/** 未编辑字段采用最新平台值，避免草稿把其他人员的修改整体覆盖。 */
export function screenPlatformDraftValues(screen: SmartScreen, draft?: ScreenPlatformDraft): ScreenPlatformFields {
  const current = screenPlatformFields(screen);
  if (!draft) return current;
  const proposed = screenPlatformFields(draft.values);
  return Object.fromEntries(fields.map((key) => [key, screenPlatformDraftFieldEdited(draft, key) ? proposed[key] : current[key]])) as unknown as ScreenPlatformFields;
}

export function screenRegistrationFingerprint(screen: SmartScreen, draft?: ScreenPlatformDraft): string {
  return JSON.stringify({ id: screen.id, source: screen.source, revision: screen.revision, fields: screenPlatformFields(screen), appVersion: screen.appVersion, observedAppVersion: screen.observedAppVersion, versionCheckedIp: screen.versionCheckedIp, appInstalled: screen.appInstalled, observedMac: screen.observedMac, adbAvailable: screen.adbAvailable, draft: draft ?? null });
}

export function screenRegistrationDiffs(before: ScreenPlatformFields | null, after: ScreenPlatformFields, snapshot: ScreenSnapshot): ScreenRegistrationDiff[] {
  const display = (key: ScreenPlatformField, value: string | null | undefined) => key === "spaceId" ? projectSpacePath(snapshot.spaces, value) || value || "未关联"
    : key === "size" ? value === "unknown" ? "待确认" : value ? `${value} 寸` : "—" : value || "—";
  return fields.filter((key) => !before || !screenPlatformFieldEqual(key, before[key], after[key])).map((key) => ({
    field: key, label: screenPlatformFieldLabels[key], before: display(key, before?.[key]), after: display(key, after[key])
  }));
}

export interface RegistrationPreviewContext {
  platformAvailable: boolean;
  spacesAvailable: boolean;
  /** 只返回本次目标 IP 的可信现场采集值；未连通或不能确定唯一 MAC 时返回 null，旧观察另走历史确认。 */
  collectMac(screen: SmartScreen, target: ScreenPlatformFields): string | null;
  /** 仅返回本次目标地址已确认安装的小新版本；缓存和可编辑资料不能充当本次读取结果。 */
  collectVersion?(screen: SmartScreen, target: ScreenPlatformFields): string | null;
}

/** 纯规则：预览不改资产、平台草稿或任务，采集由适配器提供。 */
export function buildScreenRegistrationItems(snapshot: ScreenSnapshot, ids: string[], context: RegistrationPreviewContext): ScreenRegistrationPreviewItem[] {
  const selected = [...new Set(ids)];
  const items = selected.map((screenId): ScreenRegistrationPreviewItem => {
    const screen = snapshot.screens.find((item) => item.id === screenId);
    const empty: ScreenPlatformFields = { name: "", ip: "", mac: "", size: "unknown", spaceId: null, location: "" };
    if (!screen) return { screenId, mode: "create", state: "blocked", reason: "屏记录已变化，请重新选择", before: null, after: empty, diffs: [], expectedRevision: 0, needsSpaceConfirmation: false, macSource: "empty", macMessage: "没有可核对的设备", requiredMacConfirmation: null, duplicateIds: [] };
    const draft = snapshot.platformDrafts?.[screenId];
    const before = screen.source === "platform" ? screenPlatformFields(screen) : null;
    const after = screen.source === "platform" ? screenPlatformDraftValues(screen, draft) : screenPlatformFields(screen);
    const platformAppVersion = before ? screen.appVersion?.trim() || null : null;
    const item: ScreenRegistrationPreviewItem = {
      screenId, mode: before ? "update" : "create", state: "ready", reason: "资料已通过检查", before, after,
      platformAppVersion, proposedAppVersion: null, versionMessage: "未要求核对设备版本，保留平台已有版本",
      diffs: [], expectedRevision: screen.revision, needsSpaceConfirmation: Boolean(before && before.spaceId !== after.spaceId),
      macSource: "unchanged", macMessage: "设备地址与身份未改变，沿用平台已确认 MAC", requiredMacConfirmation: null, duplicateIds: []
    };
    const errors: string[] = [];
    if (!context.platformAvailable) errors.push("平台不可用，请恢复连接后重新检查");
    if (!context.spacesAvailable) errors.push("空间目录不可用，请恢复后重新检查");
    if (snapshot.tasks.some((task) => screenTaskActive(task) && task.targets.some((target) => [screen.id, ...screen.aliases].includes(target.screenId)))) errors.push("存在活动任务或待核实结果，请先处理");
    if (before && draft) {
      const conflicts = fields.filter((key) => screenPlatformDraftFieldEdited(draft, key) && !screenPlatformFieldEqual(key, before[key], draft.base[key]) && !screenPlatformFieldEqual(key, before[key], draft.values[key]));
      if (conflicts.length) errors.push(`平台的${conflicts.map((key) => screenPlatformFieldLabels[key]).join("、")}已被其他操作修改，请重新编辑待提交资料`);
    }
    errors.push(...validateLocalScreen(after));
    if (!after.name) errors.push("名称不能为空");
    if (after.size === "unknown") errors.push("请明确选择 4 寸或 10 寸屏");
    if (!after.spaceId || !getProjectSpacePath(snapshot.spaces, after.spaceId)) errors.push("请选择当前项目的有效空间，待定空间不能提交平台");
    if (after.mac && !validRegistrationMac(after.mac)) errors.push("MAC 无效，不能使用全零、广播、组播或不完整地址");

    const changedIdentity = !before || !before.mac || !screenPlatformFieldEqual("ip", before.ip, after.ip) || !screenPlatformFieldEqual("mac", before.mac, after.mac);
    const observedVersion = screen.observedAppVersion?.trim();
    const versionCandidate = Boolean(before && observedVersion && screen.appInstalled !== false
      && screen.versionCheckedIp === after.ip && observedVersion !== platformAppVersion);
    const readVersion = (changedIdentity || versionCandidate) && context.platformAvailable;
    const observed = readVersion ? context.collectMac(screen, after) : null;
    if (changedIdentity && context.platformAvailable) {
      const expected = validRegistrationMac(after.mac) ? after.mac : "";
      const known = screen.source === "platform" && validRegistrationMac(screen.mac) ? screen.mac : screen.ip === after.ip ? screen.observedMac || screen.mac : "";
      if (observed && validRegistrationMac(observed)) {
        if (before && validRegistrationMac(before.mac) && normalizeScreenMac(before.mac) !== normalizeScreenMac(observed)) {
          item.macSource = "conflict"; item.macMessage = `平台已有 MAC ${before.mac} 与采集 MAC ${observed} 不一致`;
          errors.push("物理身份与平台注册记录冲突，请先核实，普通资料更新不能替换设备身份");
        } else if (expected && normalizeScreenMac(expected) !== normalizeScreenMac(observed)) {
          item.macSource = "conflict"; item.macMessage = `采集 MAC ${observed} 与待提交 MAC ${expected} 不一致`;
          errors.push("MAC 冲突，请核实设备并修正资料，不能用空值跳过");
        } else { after.mac = observed; item.macSource = "collected"; item.macMessage = `已模拟采集网卡 MAC：${observed}`; }
      } else {
        if (expected && validRegistrationMac(known) && normalizeScreenMac(expected) !== normalizeScreenMac(known)) {
          item.macSource = "conflict"; item.macMessage = "待提交 MAC 与已有设备采集记录冲突";
          errors.push("MAC 冲突尚未核实，采集失败也不能按空值提交");
        } else if (before && expected && !validRegistrationMac(known) && normalizeScreenMac(expected) !== normalizeScreenMac(before.mac)) {
          item.macSource = "conflict"; item.macMessage = "新增 MAC 未能采集核实，不能当作历史地址使用";
          errors.push("MAC 变更尚未核实，请恢复采集或保留原有身份资料");
        } else {
          const history = validRegistrationMac(known) ? known : "";
          after.mac = history; item.macSource = history ? "history" : "empty";
          item.requiredMacConfirmation = history ? "existing" : "empty";
          item.macMessage = history ? `采集失败，需逐屏确认沿用历史 MAC：${history}` : "采集失败且无有效历史 MAC，需逐屏确认以空 MAC 继续";
        }
      }
    }
    // 即使使用空 MAC，也不能绕过地址重复、已有物理身份或已知冲突。
    const macValues = [after.mac, screen.ip === after.ip ? effectiveScreenMac(screen) : ""].filter(validRegistrationMac).map(normalizeScreenMac);
    item.duplicateIds = snapshot.screens.filter((other) => other.id !== screen.id && (
      other.ip === after.ip || (validRegistrationMac(effectiveScreenMac(other)) && macValues.includes(normalizeScreenMac(effectiveScreenMac(other))))
    )).map((other) => other.id);
    if (item.duplicateIds.length) errors.push("IP 或 MAC 已有重复候选，请先通过疑似重复合并处理，不能直接登记或覆盖");
    item.diffs = screenRegistrationDiffs(before, after, snapshot);
    let versionReadFailed = false;
    if (readVersion) {
      const currentVersion = context.collectVersion?.(screen, after)?.trim() || null;
      const trustedIdentity = Boolean(observed && validRegistrationMac(observed) && validRegistrationMac(after.mac)
        && normalizeScreenMac(observed) === normalizeScreenMac(after.mac) && item.macSource !== "conflict");
      if (!trustedIdentity || !currentVersion) {
        versionReadFailed = true;
        item.versionMessage = !trustedIdentity ? "本次设备身份未核实，保留平台已有版本" : "本次未读到可信的小新应用版本，保留平台已有版本";
      } else if (currentVersion === platformAppVersion) {
        item.versionMessage = "本次设备版本与平台一致，无需更新版本";
      } else {
        item.proposedAppVersion = currentVersion;
        item.versionMessage = "将本次读取的小新应用版本与登记资料一并提交";
        item.diffs.push({ field: "appVersion", label: "小新版本", before: platformAppVersion || "—", after: currentVersion });
      }
    }
    if (versionCandidate && versionReadFailed && !item.diffs.length) errors.push(item.versionMessage!);
    if (errors.length) { item.state = "blocked"; item.reason = [...new Set(errors)].join("；"); }
    else if (before && !item.diffs.length) { item.state = "skip"; item.reason = "平台资料和应用版本无变更，无需提交"; }
    else if (item.requiredMacConfirmation) item.reason = "资料检查通过，提交前需确认 MAC 采集失败的处理方式";
    return item;
  });
  // 同批次的两个草稿也可能改成同一地址/身份，不能依赖提交顺序消除冲突。
  for (const item of items) {
    const matches = items.filter((other) => other.screenId !== item.screenId && (other.after.ip === item.after.ip || (validRegistrationMac(item.after.mac) && normalizeScreenMac(other.after.mac) === normalizeScreenMac(item.after.mac))));
    if (matches.length) { item.state = "blocked"; item.reason = "本批次待提交资料存在重复 IP 或 MAC，请先核对合并"; item.duplicateIds = [...new Set([...item.duplicateIds, ...matches.map((other) => other.screenId)])]; }
  }
  return items;
}
