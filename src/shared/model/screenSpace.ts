import { createProjectSpacePathResolver, getProjectSpacePath, projectSpacePath } from "./projectSpace";
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

/** 当前目录的派生索引；各项目由 store 的 computed 独立持有，不建立全局缓存。 */
export function createScreenSpaceIndex(spaces: readonly ScreenSpaceNode[]) {
  // computed 在这里读取完整节点，使名称、父级、类型的原位变化也会重建索引。
  const directory = spaces.map(({ id, name, parentId, kind }) => ({ id, name, parentId, kind }));
  const resolve = createProjectSpacePathResolver(directory);
  const uncertain = (screen: ScreenSpaceAssignment, available = true) => !screen.spaceId || (available && !resolve(screen.spaceId));
  return {
    uncertain,
    label(screen: ScreenSpaceAssignment, available = true): string {
      if (!screen.spaceId) return "待定空间";
      const path = resolve(screen.spaceId)?.map((node) => node.name.trim()).join("/");
      if (!available) return `${screen.spacePath || path || '已关联空间'}（待核验）`;
      return path || `空间已失效：${screen.spacePath || screen.spaceId}`;
    },
    matches(screen: ScreenSpaceAssignment, key: string, available = true): boolean {
      if (!key) return true;
      if (key === SCREEN_UNLOCATED_SPACE_KEY) return uncertain(screen, available);
      return Boolean(resolve(screen.spaceId)?.some((node) => node.id === key));
    },
    counts(screens: readonly ScreenSpaceAssignment[], available = true): Map<string, number> {
      const counts = new Map<string, number>();
      const add = (id: string) => counts.set(id, (counts.get(id) ?? 0) + 1);
      for (const screen of screens) {
        const path = resolve(screen.spaceId);
        if (!screen.spaceId || (available && !path)) add(SCREEN_UNLOCATED_SPACE_KEY);
        for (const node of path ?? []) add(node.id);
      }
      return counts;
    },
    /** 楼幢/楼层只作列表投影，计数和关联仍使用完整父链。 */
    options(): ScreenSpaceOption[] {
      const buildings: ScreenSpaceOption[] = [];
      const floors = new Map<string, ScreenSpaceOption[]>();
      for (const node of directory) {
        if (node.kind !== "building" && node.kind !== "floor") continue;
        const path = resolve(node.id);
        if (!path) continue;
        if (node.kind === "building") buildings.push({ value: node.id, label: node.name });
        else {
          const building = path.filter((parent) => parent.kind === "building").at(-1);
          if (building) {
            const children = floors.get(building.id) ?? [];
            children.push({ value: node.id, label: node.name });
            floors.set(building.id, children);
          }
        }
      }
      return buildings.map((building) => ({ ...building, ...(floors.has(building.value) ? { children: floors.get(building.value)! } : {}) }));
    }
  };
}

export function buildScreenSpaceOptions(spaces: readonly ScreenSpaceNode[]): ScreenSpaceOption[] {
  return createScreenSpaceIndex(spaces).options();
}

export function isScreenLocationUncertain(screen: ScreenSpaceAssignment, spaces: readonly ScreenSpaceNode[], available = true): boolean {
  return createScreenSpaceIndex(spaces).uncertain(screen, available);
}

export function screenSpaceLabel(screen: ScreenSpaceAssignment, spaces: readonly ScreenSpaceNode[], available = true): string {
  return createScreenSpaceIndex(spaces).label(screen, available);
}

export function screenMatchesSpace(screen: ScreenSpaceAssignment, key: string, spaces: readonly ScreenSpaceNode[], available = true): boolean {
  if (!key) return true;
  return createScreenSpaceIndex(spaces).matches(screen, key, available);
}

/** 楼幢/楼层仅为兼容既有列表的派生缓存，不接受独立赋值。 */
export function screenSpaceFields(spaces: readonly ScreenSpaceNode[], spaceId?: string | null) {
  const path = getProjectSpacePath(spaces, spaceId);
  const building = path?.filter((node) => node.kind === "building").at(-1);
  const floor = path?.filter((node) => node.kind === "floor").at(-1);
  return { spaceId: spaceId || null, spacePath: path ? projectSpacePath(spaces, spaceId) : "", building: building?.name ?? "", floor: floor?.name ?? "", buildingId: building?.id, floorId: floor?.id };
}
