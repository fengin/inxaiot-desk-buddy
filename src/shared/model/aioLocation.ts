import { getProjectSpacePath, type ProjectSpaceNode } from "./projectSpace";

export const AIO_ADDRESS_MAX_LENGTH = 128;

export function hasAioSpace(buildingId?: string | null): boolean {
  return !!buildingId?.trim() && buildingId.trim() !== "0";
}

/** 具体位置默认只包含已确认的楼栋、楼层，不把项目、区域或编号当成安装位置。 */
export function aioAddressSuggestion(spaces: readonly ProjectSpaceNode[], buildingId?: string | null): string {
  if (!hasAioSpace(buildingId)) return "";
  const path = getProjectSpacePath(spaces, buildingId);
  if (!path) return "";
  const ancestors = [...path].reverse();
  const building = ancestors.find(node => node.kind === "building")?.name.trim();
  const floor = ancestors.find(node => node.kind === "floor")?.name.trim();
  return [building, floor].filter(Boolean).join("_");
}

export function aioAddressAfterSpaceChange(spaces: readonly ProjectSpaceNode[], previousId: string | undefined,
  nextId: string | undefined, address = ""): string {
  if (hasAioSpace(nextId)) return aioAddressSuggestion(spaces, nextId);
  return address.trim() === aioAddressSuggestion(spaces, previousId) ? "" : address;
}

/** 只在提交时校验，不阻止用户清空输入框再重新填写。 */
export function aioAddressError(buildingId?: string | null, addrAlias?: string | null): string {
  const address = addrAlias?.trim() ?? "";
  if (hasAioSpace(buildingId) && !address) return "已选择空间，请填写具体位置";
  if (Array.from(address).length > AIO_ADDRESS_MAX_LENGTH) return `具体位置不能超过 ${AIO_ADDRESS_MAX_LENGTH} 个字符`;
  return "";
}
