import { effectiveScreenMac, normalizeScreenMac } from "./screen";
import type { SmartScreen } from "./screen";
import { screenPlatformDraftFieldEdited, screenPlatformDraftValues, screenPlatformFieldEqual, screenPlatformFieldLabels, screenPlatformFields } from "./screenRegistration";
import type { ScreenPlatformDraft, ScreenPlatformField, ScreenPlatformFields } from "./screenRegistration";
import type { createScreenSpaceIndex, ScreenSpaceAssignment } from "./screenSpace";

/** 供列表查询和详情资料展示；不能作为编辑、注册或设备操作的目标。 */
export interface ScreenListProjection {
  values: ScreenPlatformFields;
  changedFields: ReadonlySet<ScreenPlatformField>;
  titles: Record<ScreenPlatformField, string>;
  spaceAssignment: ScreenSpaceAssignment;
  spaceLabel: string;
  mac: string;
  macExplicitlyEmpty: boolean;
  searchValues: string[];
  searchMacs: string[];
  confirmedIpNote: string;
}

export function createScreenListProjection(
  screen: SmartScreen,
  draft: ScreenPlatformDraft | undefined,
  spaceIndex: ReturnType<typeof createScreenSpaceIndex>,
  spacesAvailable = true,
  platformAvailable = true
): ScreenListProjection {
  const current = screenPlatformFields(screen);
  const applicableDraft = screen.source === "platform" ? draft : undefined;
  const values = screenPlatformDraftValues(screen, applicableDraft);
  const fields = Object.keys(screenPlatformFieldLabels) as ScreenPlatformField[];
  const changedFields = new Set(fields.filter((field) => !screenPlatformFieldEqual(field, values[field], current[field])));
  // 新空间不能沿用已确认地址上的旧路径，离线时尤其不能伪装为旧空间。
  const spaceAssignment = { spaceId: values.spaceId, spacePath: values.spaceId === current.spaceId ? screen.spacePath : "" };
  const resolvedSpaceLabel = spaceIndex.label(spaceAssignment, spacesAvailable);
  const spaceLabel = !spacesAvailable && changedFields.has("spaceId") && resolvedSpaceLabel === "已关联空间（待核验）"
    ? `待提交空间 ${values.spaceId}（待核验）` : resolvedSpaceLabel;
  const currentSpaceLabel = spaceIndex.label(screen, spacesAvailable);
  const display = (field: ScreenPlatformField, value: string | null, proposed: boolean) => field === "spaceId"
    ? proposed ? spaceLabel : currentSpaceLabel
    : field === "size" ? value === "unknown" ? "待确认" : `${value} 寸` : value || "未填写";
  const titles = Object.fromEntries(fields.map((field) => [field, changedFields.has(field)
    ? `${screenPlatformFieldLabels[field]}待提交：${display(field, values[field], true)}\n${platformAvailable ? "最新平台值" : "已确认值（缓存）"}：${display(field, current[field], false)}`
    : display(field, values[field], true)])) as Record<ScreenPlatformField, string>;
  const macEdited = screenPlatformDraftFieldEdited(applicableDraft, "mac");
  // 明确清空的草稿必须显示空值，不能被旧地址采集的 MAC 填回。
  const mac = values.mac || (macEdited ? "" : effectiveScreenMac(screen));
  const confirmedIpNote = changedFields.has("ip")
    ? `平台状态、已记录版本及本机检查仍对应已确认 IP ${screen.ip}，待提交 IP 尚未用于这些结果。`
    : "";
  if (!values.mac && mac) titles.mac = `已确认 IP ${screen.ip} 的采集 MAC：${mac}${screen.source === "platform" ? "\n平台 MAC 未填写；采集值尚未保存为待提交修改" : ""}`;
  return {
    values, changedFields, titles, spaceAssignment, spaceLabel, mac, macExplicitlyEmpty: Boolean(macEdited && !values.mac), confirmedIpNote,
    searchValues: [values.name, values.ip, values.mac, values.location, spaceLabel,
      screen.name, screen.ip, screen.mac, screen.observedMac, screen.location, currentSpaceLabel],
    searchMacs: [values.mac, screen.mac, screen.observedMac].map(normalizeScreenMac)
  };
}
