import { effectiveScreenMac, normalizeScreenMac, screenTaskActive, validateLocalScreen } from "./screen";
import type { ScreenSize, ScreenSnapshot, SmartScreen } from "./screen";
import { getProjectSpacePath, projectSpacePath } from "./projectSpace";

/** 平台注册/更新允许提交的资料，不包含在线状态或应用版本。 */
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
export interface ScreenRegistrationDiff {
  field: ScreenPlatformField;
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
  platformId?: string;
  sourceFingerprint: string;
}

export const screenPlatformFieldLabels: Record<ScreenPlatformField, string> = {
  name: "名称", ip: "IP 地址", mac: "MAC 地址", size: "屏尺寸", spaceId: "所在空间", location: "详细位置"
};
const fields = Object.keys(screenPlatformFieldLabels) as ScreenPlatformField[];
export const validRegistrationMac = (value: string) => {
  const mac = normalizeScreenMac(value);
  return /^[0-9A-F]{12}$/.test(mac) && !/^(0{12}|F{12})$/.test(mac) && (Number.parseInt(mac.slice(0, 2), 16) & 1) === 0;
};

/** 只复制平台注册白名单字段，不能连带提交运行状态、版本或空间展示缓存。 */
export function screenPlatformFields(screen: Pick<SmartScreen, "name" | "ip" | "mac" | "size" | "spaceId" | "location">): ScreenPlatformFields {
  return { name: screen.name.trim(), ip: screen.ip.trim(), mac: screen.mac.trim(), size: screen.size, spaceId: screen.spaceId || null, location: screen.location.trim() };
}

/** 未编辑字段采用最新平台值，避免草稿把其他人员的修改整体覆盖。 */
export function screenPlatformDraftValues(screen: SmartScreen, draft?: ScreenPlatformDraft): ScreenPlatformFields {
  const current = screenPlatformFields(screen);
  if (!draft) return current;
  return Object.fromEntries(fields.map((key) => [key, draft.values[key] !== draft.base[key] ? draft.values[key] : current[key]])) as unknown as ScreenPlatformFields;
}

export function screenRegistrationFingerprint(screen: SmartScreen, draft?: ScreenPlatformDraft): string {
  return JSON.stringify({ id: screen.id, source: screen.source, revision: screen.revision, fields: screenPlatformFields(screen), observedMac: screen.observedMac, adbAvailable: screen.adbAvailable, draft: draft ?? null });
}

export function screenRegistrationDiffs(before: ScreenPlatformFields | null, after: ScreenPlatformFields, snapshot: ScreenSnapshot): ScreenRegistrationDiff[] {
  const display = (key: ScreenPlatformField, value: string | null | undefined) => key === "spaceId" ? projectSpacePath(snapshot.spaces, value) || value || "未关联"
    : key === "size" ? value === "unknown" ? "待确认" : value ? `${value} 寸` : "—" : value || "—";
  return fields.filter((key) => !before || before[key] !== after[key]).map((key) => ({
    field: key, label: screenPlatformFieldLabels[key], before: display(key, before?.[key]), after: display(key, after[key])
  }));
}

export interface RegistrationPreviewContext {
  platformAvailable: boolean;
  spacesAvailable: boolean;
  collectMac(screen: SmartScreen, target: ScreenPlatformFields): string | null;
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
    const item: ScreenRegistrationPreviewItem = {
      screenId, mode: before ? "update" : "create", state: "ready", reason: "资料已通过检查", before, after,
      diffs: [], expectedRevision: screen.revision, needsSpaceConfirmation: Boolean(before && before.spaceId !== after.spaceId),
      macSource: "unchanged", macMessage: "设备地址与身份未变更，无需连接设备", requiredMacConfirmation: null, duplicateIds: []
    };
    const errors: string[] = [];
    if (!context.platformAvailable) errors.push("平台不可用，请恢复连接后重新检查");
    if (!context.spacesAvailable) errors.push("空间目录不可用，请恢复后重新检查");
    if (snapshot.tasks.some((task) => screenTaskActive(task) && task.targets.some((target) => [screen.id, ...screen.aliases].includes(target.screenId)))) errors.push("存在活动任务或待核实结果，请先处理");
    if (before && draft) {
      const conflicts = fields.filter((key) => draft.values[key] !== draft.base[key] && before[key] !== draft.base[key] && before[key] !== draft.values[key]);
      if (conflicts.length) errors.push(`平台的${conflicts.map((key) => screenPlatformFieldLabels[key]).join("、")}已被其他操作修改，请重新编辑待提交资料`);
    }
    errors.push(...validateLocalScreen(after));
    if (!after.name) errors.push("名称不能为空");
    if (after.size === "unknown") errors.push("请明确选择 4 寸或 10 寸屏");
    if (!after.spaceId || !getProjectSpacePath(snapshot.spaces, after.spaceId)) errors.push("请选择当前项目的有效空间，待定空间不能提交平台");
    if (after.mac && !validRegistrationMac(after.mac)) errors.push("MAC 无效，不能使用全零、广播、组播或不完整地址");

    const changedIdentity = !before || before.ip !== after.ip || normalizeScreenMac(before.mac) !== normalizeScreenMac(after.mac);
    if (changedIdentity && context.platformAvailable) {
      const observed = context.collectMac(screen, after);
      const expected = validRegistrationMac(after.mac) ? after.mac : "";
      const known = screen.source === "platform" && validRegistrationMac(screen.mac) ? screen.mac : screen.observedMac || screen.mac;
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
    if (errors.length) { item.state = "blocked"; item.reason = [...new Set(errors)].join("；"); }
    else if (before && !item.diffs.length) { item.state = "skip"; item.reason = "平台资料无变更，无需提交"; }
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
