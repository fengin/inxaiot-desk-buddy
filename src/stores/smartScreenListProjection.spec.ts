import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import { screenCriticalDraftFields } from "@/shared/model/screenMaintenance";
import { useSmartScreensStore } from "./smartScreens";

const adapter = vi.hoisted(() => ({ load: vi.fn(), subscribe: vi.fn(() => () => {}) }));
vi.mock("@/shared/api/screenAdapter", () => ({ useScreenAdapter: () => adapter }));

function setup() {
  const store = useSmartScreensStore();
  store.snapshot = createScreenSnapshot();
  const screen = store.snapshot.screens.find((item) => item.source === "platform")!;
  store.snapshot.screens = [screen];
  const current = store.snapshot.screens[0]!;
  current.name = "已确认名称"; current.location = "已确认位置"; current.size = "10";
  current.mac = "02:11:22:33:44:55";
  const base = screenPlatformFields(current);
  store.snapshot.platformDrafts = { [current.id]: { screenId: current.id, baseRevision: current.revision, base,
    values: { ...base, name: "待提交名称", ip: "192.0.2.222", mac: "02:AA:BB:CC:DD:EE", location: "待提交位置", size: "4", spaceId: "floor-b-1" }, updatedAt: "2026-10-20T00:00:00Z" } };
  return { store, screen: current, draft: store.snapshot.platformDrafts[current.id]! };
}

describe("列表投影与已确认屏隔离", () => {
  beforeEach(() => { setActivePinia(createPinia()); adapter.load.mockReset(); });

  it("搜索新旧名称/IP/MAC/位置，筛选尺寸/空间随显示值但返回已确认原对象", () => {
    const { store, screen, draft } = setup();
    for (const keyword of [screen.name, screen.ip, screen.mac, "021122334455", screen.location, draft.values.name, draft.values.ip, draft.values.mac, "02AABBCCDDEE", draft.values.location]) {
      store.filters.keyword = keyword;
      expect(store.filtered, keyword).toHaveLength(1);
      expect(store.filtered[0]).toBe(screen);
    }
    store.filters.keyword = "";
    store.filters.size = "4";
    store.filters.space = "floor-b-1";
    expect(store.filtered).toEqual([screen]);
    expect(store.paged[0]).toBe(screen);
    expect(store.listSizeStats).toEqual({ small: 1, large: 0 });
    expect(store.listSpaceCounts.get("floor-b-1")).toBe(1);
    expect(store.stats.large).toBe(1);
    store.selectedIds = [screen.id];
    expect(store.selectedScreens[0]).toBe(screen);
    expect(store.visibleScreens[0]).toBe(screen);
    expect(store.snapshot.screens[0]).toBe(screen);
    expect(store.selectedScreens[0]!.ip).toBe(screen.ip);
    expect(store.selectedScreens[0]!.size).toBe("10");
    expect(screenCriticalDraftFields(screen, draft)).toEqual(["IP", "MAC", "尺寸"]);
    store.filters.size = "10";
    expect(store.filtered).toEqual([]);
  });

  it("状态版本和重复候选仍采用已确认事实，不因草稿新地址产生候选", () => {
    const { store, screen, draft } = setup();
    const local = { ...screen, id: "unregistered", source: "local" as const, ip: draft.values.ip, mac: "", observedMac: "" };
    store.snapshot.screens.push(local);
    expect(store.candidates).toHaveLength(0);
    store.filters.registration = "platform";
    store.filters.appVersion = screen.appVersion || "unknown";
    store.filters.status = screen.platformStatus;
    expect(store.filtered).toEqual([screen]);
    expect(store.statusDifferences.every((item) => item === screen)).toBe(true);
  });

  it("选择/翻页/关键词不重建投影与空间计数，草稿修改和撤销能刷新并收敛", () => {
    const { store, screen, draft } = setup();
    const projections = store.listProjections;
    const counts = store.listSpaceCounts;
    const spaceIndex = store.spaceIndex;
    store.toggleSelection(screen.id, true); store.page = 2; store.filters.keyword = "名称";
    expect(store.listProjections).toBe(projections);
    expect(store.listSpaceCounts).toBe(counts);
    expect(store.spaceIndex).toBe(spaceIndex);
    draft.values.name = "另一个草稿名";
    expect(store.listProjections).not.toBe(projections);
    expect(store.listProjections.get(screen.id)!.values.name).toBe("另一个草稿名");
    Object.assign(screen, draft.values);
    expect(store.listProjections.get(screen.id)!.changedFields.size).toBe(0);
    expect(store.draftIds.has(screen.id)).toBe(false);
    expect(store.snapshot.platformDrafts![screen.id]).toBe(draft);
    delete store.snapshot.platformDrafts![screen.id];
    expect(store.draftIds.has(screen.id)).toBe(false);
    expect(store.listProjections.get(screen.id)!.values.name).toBe(screen.name);
  });

  it("同值raw草稿保留CAS版本但不显示徽标，其他字段仍有差异时继续提示", async () => {
    const { store, screen, draft } = setup();
    draft.revision = 7;
    Object.assign(draft.values, screenPlatformFields(screen), { mac: "02-11-22-33-44-55" });
    expect(store.draftIds.has(screen.id)).toBe(false);
    expect(screenCriticalDraftFields(screen, draft)).toEqual([]);
    expect(store.snapshot.platformDrafts![screen.id]!.revision).toBe(7);
    draft.values.location = "仍待提交的位置";
    expect(store.draftIds.has(screen.id)).toBe(true);
    expect(store.listProjections.get(screen.id)!.changedFields).toEqual(new Set(["location"]));
    draft.values.location = screen.location;
    store.projectId = "raw-draft-refresh";
    adapter.load.mockResolvedValue(JSON.parse(JSON.stringify(store.snapshot)));
    await store.refresh();
    expect(store.draftIds.has(screen.id)).toBe(false);
    expect(store.snapshot.platformDrafts![screen.id]!.revision).toBe(7);
    expect(store.listProjections.get(screen.id)!.changedFields.size).toBe(0);
  });

  it("原位更新空间目录会刷新显示路径，项目切换清空旧草稿投影与选择", async () => {
    const { store, screen } = setup();
    const node = store.snapshot.spaces.find((item) => item.id === "floor-b-1")!;
    const before = store.listProjections;
    node.name = "重命名楼层";
    expect(store.listProjections).not.toBe(before);
    expect(store.listProjections.get(screen.id)!.spaceLabel).toContain("重命名楼层");
    store.selectedIds = [screen.id];
    adapter.load.mockResolvedValue({ screens: [], spaces: [], tasks: [], ignoredPairs: [], platformAvailable: true });
    await store.bindProject("another-project");
    expect(store.listProjections.size).toBe(0);
    expect(store.listSpaceCounts.size).toBe(0);
    expect(store.selectedScreens).toEqual([]);
    store.stop();
  });
});
