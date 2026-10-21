import { describe, expect, it } from "vitest";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { createScreenListProjection } from "@/shared/model/screenListProjection";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import type { ScreenPlatformDraft } from "@/shared/model/screenRegistration";
import { createScreenSpaceIndex } from "@/shared/model/screenSpace";

function fixture() {
  const snapshot = createScreenSnapshot();
  const screen = snapshot.screens.find((item) => item.source === "platform")!;
  const base = screenPlatformFields(screen);
  const draft: ScreenPlatformDraft = { screenId: screen.id, baseRevision: screen.revision, base, values: { ...base }, updatedAt: "2026-10-20T00:00:00Z" };
  const index = createScreenSpaceIndex(snapshot.spaces);
  return { screen, draft, index };
}

describe("智能屏列表草稿投影", () => {
  it("六项草稿显示相对当前值的变更，保留原记录与旧 IP 事实来源", () => {
    const { screen, draft, index } = fixture();
    const before = JSON.stringify(screen);
    Object.assign(draft.values, { name: "待提交名称", ip: "192.0.2.222", mac: "", size: "4", spaceId: null, location: "待提交位置" });
    screen.observedMac = "02:11:22:33:44:55";
    const projection = createScreenListProjection(screen, draft, index);
    expect(projection.values).toEqual(draft.values);
    expect([...projection.changedFields].sort()).toEqual(["ip", "location", "mac", "name", "size", "spaceId"].sort());
    expect(projection.mac).toBe("");
    expect(projection.macExplicitlyEmpty).toBe(true);
    expect(projection.confirmedIpNote).toContain(screen.ip);
    expect(projection.titles.ip).toContain(`最新平台值：${screen.ip}`);
    expect(projection.searchValues).toContain(screen.ip);
    expect(projection.searchValues).toContain(draft.values.ip);
    expect({ ...screen, observedMac: JSON.parse(before).observedMac }).toEqual(JSON.parse(before));
  });

  it("平台刷新未编辑字段不回退，已收敛字段不着色，三方冲突仍显示草稿与最新平台值", () => {
    const { screen, draft, index } = fixture();
    draft.values.name = "本机名称";
    draft.values.location = "本机位置";
    screen.name = "本机名称";
    screen.location = "平台另一个位置";
    screen.ip = "192.0.2.77";
    const projection = createScreenListProjection(screen, draft, index);
    expect(projection.values.ip).toBe(screen.ip);
    expect(projection.changedFields.has("ip")).toBe(false);
    expect(projection.changedFields.has("name")).toBe(false);
    expect(projection.values.location).toBe("本机位置");
    expect(projection.titles.location).toContain("最新平台值：平台另一个位置");
    expect(projection.changedFields).toEqual(new Set(["location"]));
  });

  it("MAC格式等价不着色，显式清空即使平台也已为空仍不回填实测 MAC", () => {
    const { screen, draft, index } = fixture();
    screen.mac = draft.base.mac = "02:11:22:33:44:55";
    draft.values.mac = "02-11-22-33-44-55";
    expect(createScreenListProjection(screen, draft, index).changedFields.has("mac")).toBe(false);
    draft.values.mac = "";
    screen.mac = "";
    screen.observedMac = "02:11:22:33:44:55";
    const projection = createScreenListProjection(screen, draft, index);
    expect(projection.mac).toBe("");
    expect(projection.macExplicitlyEmpty).toBe(true);
    expect(projection.changedFields.has("mac")).toBe(false);
  });

  it("未保存的采集MAC只显示来源，保存实际差异后才作为待提交MAC着色", () => {
    const { screen, draft, index } = fixture();
    screen.mac = draft.base.mac = draft.values.mac = "";
    screen.observedMac = "02:11:22:33:44:55";
    const candidate = createScreenListProjection(screen, undefined, index);
    expect(candidate.mac).toBe(screen.observedMac);
    expect(candidate.values.mac).toBe(""); expect(candidate.changedFields.size).toBe(0);
    expect(candidate.titles.mac).toContain("采集值尚未保存为待提交修改");
    draft.values.mac = screen.observedMac;
    const saved = createScreenListProjection(screen, draft, index);
    expect(saved.changedFields).toEqual(new Set(["mac"]));
    expect(saved.titles.mac).toContain("MAC 地址待提交");
    draft.values.mac = "";
    expect(createScreenListProjection(screen, draft, index).changedFields.size).toBe(0);
    expect(screen.mac).toBe("");
  });

  it("离线新空间不继承旧路径，空空间和撤销草稿都能准确显示", () => {
    const { screen, draft } = fixture();
    screen.spacePath = "已确认楼/旧楼层";
    draft.values.spaceId = "new-space";
    const index = createScreenSpaceIndex([]);
    const projection = createScreenListProjection(screen, draft, index, false, false);
    expect(projection.spaceAssignment.spacePath).toBe("");
    expect(projection.spaceLabel).not.toContain("旧楼层");
    expect(projection.spaceLabel).toContain("待核验");
    expect(projection.titles.spaceId).toContain("已确认值（缓存）：已确认楼/旧楼层");
    draft.values.spaceId = null;
    expect(createScreenListProjection(screen, draft, index, false, false).spaceLabel).toBe("待定空间");
    const restored = createScreenListProjection(screen, undefined, index, false, false);
    expect(restored.changedFields.size).toBe(0);
    expect(restored.spaceLabel).toContain("旧楼层");
  });
});
