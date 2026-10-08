import type { ScreenSnapshot, SmartScreen } from "@/shared/model/screen";
import type { ScreenSpaceNode } from "@/shared/model/screenSpace";
import { screenSpaceFields } from "@/shared/model/screenSpace";

export const screenSpaces: ScreenSpaceNode[] = ["a", "b"].flatMap((building) => [
  { id: `building-${building}`, name: `${building.toUpperCase()}座`, kind: "building" as const },
  ...[1, 2, 3].flatMap((floor) => [
    { id: `floor-${building}-${floor}`, name: `${floor}F`, kind: "floor" as const, parentId: `building-${building}` },
    { id: `area-${building}-${floor}-east`, name: "东区", kind: "area" as const, parentId: `floor-${building}-${floor}` },
    { id: `area-${building}-${floor}-meeting`, name: "会议区", kind: "area" as const, parentId: `area-${building}-${floor}-east` },
    { id: `area-${building}-${floor}-room`, name: "会议室", kind: "area" as const, parentId: `area-${building}-${floor}-meeting` }
  ])
]);

/** 全部为文档保留网段中的虚构屏，不使用现场资产或凭据。 */
export function createScreenSnapshot(): ScreenSnapshot {
  const screens: SmartScreen[] = Array.from({ length: 21 }, (_, i) => {
    const size = i >= 18 ? "4" : "10";
    return {
      id: `platform-screen-${i + 1}`, source: "platform", name: `${i < 12 ? "A" : "B"}座-${i % 3 + 1}F-${["电梯厅", "会议室", "公共走廊", "接待区"][i % 4]}屏`,
      ip: `192.0.2.${31 + i}`, mac: i % 3 ? "" : `02:00:00:10:00:${(i + 1).toString(16).padStart(2, "0").toUpperCase()}`,
      observedMac: i % 2 ? "" : `02:00:00:10:00:${(i + 1).toString(16).padStart(2, "0").toUpperCase()}`,
      size, building: i < 12 ? "A座" : "B座", floor: `${i % 3 + 1}F`, location: `${["东区", "西区", "会议区"][i % 3]} · ${["电梯旁", "入口墙面", "门外右侧", "服务台"][i % 4]}`,
      buildingId: `building-${i < 12 ? "a" : "b"}`, floorId: `floor-${i < 12 ? "a" : "b"}-${i % 3 + 1}`,
      platformStatus: i === 5 || i === 16 ? "offline" : "online", revision: 1,
      ping: i === 5 ? "online" : i === 2 ? "offline" : i % 3 === 0 ? "online" : null,
      checkedAt: i % 3 === 0 || i === 5 || i === 2 ? new Date().toISOString() : null,
      appVersion: i % 5 === 0 ? null : i % 3 === 0 ? "1.6.0" : "1.5.2",
      android: size === "4" ? "Android 8.1" : "Android 10", abi: size === "4" ? "arm64-v8a" : "armeabi-v7a",
      adbAvailable: i !== 16 && i !== 10, persistentAdb: i % 3 === 0, freeSpaceMb: i === 8 ? 90 : 1600 + i * 32,
      clockOffsetSeconds: i % 4 === 0 ? 145 : 3, aliases: []
    };
  });
  for (let i = 0; i < 6; i++) screens.push({
    id: `local-screen-${i + 1}`, source: "local", name: ["A座待登记电梯厅屏", "B座新装门口屏", "备用地址待核对屏", "样板间 4 寸屏", "施工区新装屏", "接待区备用屏"][i]!,
    ip: i === 0 ? screens[0]!.ip : i === 2 ? screens[3]!.ip : `192.0.2.${101 + i}`,
    mac: "", observedMac: i === 0 ? screens[0]!.mac : i === 1 ? screens[12]!.mac : `02:00:00:20:00:0${i + 1}`,
    size: i === 3 ? "4" : "10", building: i === 4 ? "" : i === 1 ? "B座" : "A座", floor: i === 4 ? "" : `${i % 3 + 1}F`, location: "本机登记 · 等待项目录入",
    buildingId: i === 4 ? undefined : `building-${i === 1 ? "b" : "a"}`, floorId: i === 4 ? undefined : `floor-${i === 1 ? "b" : "a"}-${i % 3 + 1}`,
    platformStatus: "offline", revision: 1, ping: i === 4 ? null : "online", checkedAt: null,
    appVersion: i === 0 ? "1.5.2" : null, android: i === 3 ? "Android 8.1" : "Android 10", abi: i === 3 ? "arm64-v8a" : "armeabi-v7a",
    adbAvailable: true, persistentAdb: false, freeSpaceMb: 2400, clockOffsetSeconds: 210, aliases: []
  });
  for (const screen of screens) {
    const scope = screen.source === "platform" && screen.id === "platform-screen-2" ? "area-a-2-room"
      : screen.source === "platform" && screen.id === "platform-screen-3" ? "area-a-3-east"
        : screen.floorId ?? screen.buildingId ?? null;
    Object.assign(screen, screenSpaceFields(screenSpaces, scope));
  }
  return { screens, spaces: screenSpaces.map((space) => ({ ...space })), spacesAvailable: true, tasks: [], ignoredPairs: [], platformAvailable: true };
}
