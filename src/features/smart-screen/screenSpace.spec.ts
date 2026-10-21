import { describe, expect, it } from "vitest";
import { computed, reactive } from "vue";
import { screenSpaces } from "@/dev-fixtures/screenData";
import type { SmartScreen } from "@/shared/model/screen";
import {
  buildScreenSpaceOptions,
  createScreenSpaceIndex,
  isScreenLocationUncertain,
  SCREEN_UNLOCATED_SPACE_KEY,
  screenSpaceLabel,
  screenMatchesSpace
} from "@/shared/model/screenSpace";
import type { ScreenSpaceAssignment } from "@/shared/model/screenSpace";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";

const buildings = screenSpaces.filter((space) => space.kind === "building");
const firstBuilding = buildings[0]!;
const secondBuilding = buildings[1]!;
const firstFloors = screenSpaces.filter((space) => space.kind === "floor" && space.parentId === firstBuilding.id);
const firstFloor = firstFloors[0]!;
const otherFloor = firstFloors[1]!;
const secondBuildingFloor = screenSpaces.find((space) => space.kind === "floor" && space.parentId === secondBuilding.id)!;
const located: ScreenSpaceAssignment = { spaceId: firstFloor.id, spacePath: "A座 / 1F" };

describe("智能屏空间目录与选择", () => {
  it("从空间目录生成楼幢和所属楼层选项，不依赖屏清单", () => {
    const options = buildScreenSpaceOptions(screenSpaces);
    expect(options).toHaveLength(buildings.length);
    expect(options[0]).toEqual({
      value: firstBuilding.id,
      label: firstBuilding.name,
      children: firstFloors.map((floor) => ({ value: floor.id, label: floor.name }))
    });
    expect(options[0]?.children?.some((floor) => floor.value === secondBuildingFloor.id)).toBe(false);
    expect(options.some((option) => option.value === SCREEN_UNLOCATED_SPACE_KEY)).toBe(false);
  });

  it("目录暂不可用保留原关联待核验，目录确实为空才判定失效", () => {
    expect(buildScreenSpaceOptions([])).toEqual([]);
    expect(isScreenLocationUncertain(located, [])).toBe(true);
    expect(screenMatchesSpace(located, firstBuilding.id, [])).toBe(false);
    expect(screenMatchesSpace(located, SCREEN_UNLOCATED_SPACE_KEY, [])).toBe(true);
    expect(isScreenLocationUncertain(located, [], false)).toBe(false);
    expect(screenSpaceLabel(located, [], false)).toBe("A座 / 1F（待核验）");
  });

  it("孤立楼层不伪造上级楼幢，无楼层的真实楼幢仍可显示", () => {
    expect(buildScreenSpaceOptions([firstFloor])).toEqual([]);
    expect(buildScreenSpaceOptions([firstBuilding, secondBuildingFloor])).toEqual([
      { value: firstBuilding.id, label: firstBuilding.name }
    ]);
  });

  it("楼幢筛选包含该楼幢各层，楼层筛选只匹配对应 ID", () => {
    const otherLocated = { spaceId: otherFloor.id };
    expect(isScreenLocationUncertain(located, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(located, firstBuilding.id, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(otherLocated, firstBuilding.id, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(located, firstFloor.id, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(otherLocated, firstFloor.id, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(located, secondBuilding.id, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(located, secondBuildingFloor.id, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(located, SCREEN_UNLOCATED_SPACE_KEY, screenSpaces)).toBe(false);
  });

  it.each<ScreenSpaceAssignment>([
    {},
    { spaceId: null },
    { spaceId: "deleted-space" }
  ])("未选或失效关联进入待定分组：%j", (screen) => {
    expect(isScreenLocationUncertain(screen, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(screen, SCREEN_UNLOCATED_SPACE_KEY, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(screen, firstBuilding.id, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(screen, firstFloor.id, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(screen, "", screenSpaces)).toBe(true);
  });

  it("展示名称与目录相同也不自动补造空间关联", () => {
    const screen: Pick<SmartScreen, "building" | "floor" | "spaceId"> = {
      building: firstBuilding.name,
      floor: firstFloor.name
    };
    expect(isScreenLocationUncertain(screen, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(screen, firstBuilding.id, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(screen, SCREEN_UNLOCATED_SPACE_KEY, screenSpaces)).toBe(true);
  });

  it("楼幢直属属于有效关联，深层区域通过祖先楼层与楼幢筛选", () => {
    const direct = { spaceId: firstBuilding.id };
    expect(isScreenLocationUncertain(direct, screenSpaces)).toBe(false);
    expect(screenMatchesSpace(direct, firstBuilding.id, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(direct, firstFloor.id, screenSpaces)).toBe(false);
    const deep = { spaceId: "area-a-1-room" };
    expect(screenSpaceLabel(deep, screenSpaces)).toBe("A座/1F/东区/会议区/会议室");
    expect(screenMatchesSpace(deep, firstFloor.id, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(deep, firstBuilding.id, screenSpaces)).toBe(true);
    expect(screenMatchesSpace(deep, secondBuilding.id, screenSpaces)).toBe(false);
    expect(isScreenLocationUncertain(deep, screenSpaces)).toBe(false);
  });

  it("全部空间包含有效记录，未知筛选 ID 不命中任何记录", () => {
    expect(screenMatchesSpace(located, "", screenSpaces)).toBe(true);
    expect(screenMatchesSpace(located, "deleted-space", screenSpaces)).toBe(false);
  });

  it("一次统计完整父链，保留直属、深层及离线待核验的分组语义", () => {
    const index = createScreenSpaceIndex(screenSpaces);
    const screens = [
      { spaceId: firstBuilding.id }, { spaceId: firstFloor.id }, { spaceId: "area-a-1-room" },
      { spaceId: secondBuildingFloor.id }, {}, { spaceId: "deleted-space" }
    ];
    const counts = index.counts(screens);
    expect(counts.get(firstBuilding.id)).toBe(3);
    expect(counts.get(firstFloor.id)).toBe(2);
    expect(counts.get("area-a-1-room")).toBe(1);
    expect(counts.get(secondBuilding.id)).toBe(1);
    expect(counts.get(SCREEN_UNLOCATED_SPACE_KEY)).toBe(2);
    expect(index.counts(screens, false).get(SCREEN_UNLOCATED_SPACE_KEY)).toBe(1);
    expect(index.label({ spaceId: "deleted-space", spacePath: "原空间" }, false)).toBe("原空间（待核验）");
  });

  it("目录原位变更及切换项目时重建索引，不保留旧父链或失效判定", () => {
    const state = reactive<{ spaces: ProjectSpaceNode[] }>({ spaces: [
      { id: "a", name: "A座", kind: "building" }, { id: "b", name: "B座", kind: "building" },
      { id: "floor", name: "1层", kind: "floor", parentId: "a" }
    ] });
    const index = computed(() => createScreenSpaceIndex(state.spaces));
    const screen = { spaceId: "floor" };
    expect(index.value.label(screen)).toBe("A座/1层");
    state.spaces[2]!.parentId = "b";
    expect(index.value.label(screen)).toBe("B座/1层");
    expect(index.value.counts([screen]).has("a")).toBe(false);
    expect(index.value.options().find(option => option.value === "b")?.children).toHaveLength(1);
    state.spaces[1]!.name = "";
    expect(index.value.uncertain(screen)).toBe(true);
    state.spaces[1]!.name = "B新座";
    expect(index.value.label(screen)).toBe("B新座/1层");
    state.spaces.push({ id: "b", name: "重复编号", kind: "building" });
    expect(index.value.uncertain(screen)).toBe(true);
    state.spaces = [{ id: "floor", name: "另一项目空间", kind: "area" }];
    expect(index.value.label(screen)).toBe("另一项目空间");
    expect(index.value.options()).toEqual([]);
  });
});
