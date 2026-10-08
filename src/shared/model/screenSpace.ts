import { getProjectSpacePath, projectSpacePath } from "./projectSpace";
import type { ProjectSpaceNode } from "./projectSpace";

export type ScreenSpaceNode = ProjectSpaceNode;

export type ScreenSpaceOption = {
  value: string;
  label: string;
  children?: ScreenSpaceOption[];
};

export interface ScreenSpaceAssignment {
  spaceId?: string | null;
  spacePath?: string;
}

export const SCREEN_UNLOCATED_SPACE_KEY = "__unlocated__";

/** 仅为列表生成楼幢/楼层投影，资产关联仍使用完整空间节点。 */
export function buildScreenSpaceOptions(spaces: readonly ScreenSpaceNode[]): ScreenSpaceOption[] {
  const buildings = spaces.filter((node) => node.kind === "building" && getProjectSpacePath(spaces, node.id));
  return buildings.map((building) => {
    const floors = spaces
      .filter((space) => space.kind === "floor" && getProjectSpacePath(spaces, space.id)?.filter((node) => node.kind === "building").at(-1)?.id === building.id)
      .map((floor) => ({ value: floor.id, label: floor.name }));
    return { value: building.id, label: building.name, ...(floors.length ? { children: floors } : {}) };
  });
}

export function isScreenLocationUncertain(screen: ScreenSpaceAssignment, spaces: readonly ScreenSpaceNode[], available = true): boolean {
  if (!screen.spaceId) return true;
  return available && !getProjectSpacePath(spaces, screen.spaceId);
}

export function screenSpaceLabel(screen: ScreenSpaceAssignment, spaces: readonly ScreenSpaceNode[], available = true): string {
  if (!screen.spaceId) return "待定空间";
  const path = projectSpacePath(spaces, screen.spaceId);
  if (!available) return `${screen.spacePath || path || '已关联空间'}（待核验）`;
  return path || `空间已失效：${screen.spacePath || screen.spaceId}`;
}

export function screenMatchesSpace(screen: ScreenSpaceAssignment, key: string, spaces: readonly ScreenSpaceNode[], available = true): boolean {
  if (!key) return true;
  if (key === SCREEN_UNLOCATED_SPACE_KEY) return isScreenLocationUncertain(screen, spaces, available);
  return Boolean(getProjectSpacePath(spaces, screen.spaceId)?.some((node) => node.id === key));
}

/** 楼幢/楼层仅为兼容既有列表的派生缓存，不接受独立赋值。 */
export function screenSpaceFields(spaces: readonly ScreenSpaceNode[], spaceId?: string | null) {
  const path = getProjectSpacePath(spaces, spaceId);
  const building = path?.filter((node) => node.kind === "building").at(-1);
  const floor = path?.filter((node) => node.kind === "floor").at(-1);
  return { spaceId: spaceId || null, spacePath: path ? projectSpacePath(spaces, spaceId) : "", building: building?.name ?? "", floor: floor?.name ?? "", buildingId: building?.id, floorId: floor?.id };
}
