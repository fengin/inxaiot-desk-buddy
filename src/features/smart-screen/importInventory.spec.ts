import { describe, expect, it } from "vitest";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { inventorySpacePaths, inventorySpacePathsCsv, parseScreenInventory, revalidateScreenInventory } from "./importInventory";

const spaces: ProjectSpaceNode[] = [
  { id: "a", name: "A座", kind: "building" },
  { id: "f", name: "2F", kind: "floor", parentId: "a" },
  { id: "area", name: "东区", kind: "area", parentId: "f" },
  { id: "room", name: "会议室", kind: "other", parentId: "area" }
];
const header = "名称,IP,尺寸,MAC,空间路径,安装位置\n";
const line = (path: string, ip = "192.0.2.180") => `新增屏,${ip},10,,${path},门口`;

describe("智能屏清单完整空间路径", () => {
  it("完整路径唯一匹配到深层节点，也允许停在楼幢或区域", () => {
    const rows = parseScreenInventory(header + [line(" A座 / 2F / 东区 / 会议室 "), line("A座", "192.0.2.181"), line("A座 / 2F / 东区", "192.0.2.182")].join("\n"), [], spaces);
    expect(rows.map((row) => row.input.spaceId)).toEqual(["room", "a", "area"]);
    expect(rows.every((row) => !row.error)).toBe(true);
    expect(rows[0]!.originalSpacePath).toBe("A座 / 2F / 东区 / 会议室");
    expect(rows[0]!.matchedSpacePath).toBe("A座/2F/东区/会议室");
    expect(rows[0]!.input.spacePath).toBe("A座/2F/东区/会议室");
  });

  it("非空路径缺失、歧义、失效或目录不可用均阻断；空路径允许待定", () => {
    const ambiguous = [...spaces, { ...spaces[3]!, id: "room-duplicate" }];
    expect(parseScreenInventory(header + line("A座 / 2F / 东区 / 会议室"), [], ambiguous)[0]!.spaceStatus).toBe("ambiguous");
    const missing = parseScreenInventory(header + line("会议室"), [], spaces)[0]!;
    expect(missing.spaceStatus).toBe("missing"); expect(missing.matchedSpacePath).toBe(""); expect(missing.input.spaceId).toBeNull();
    expect(parseScreenInventory(header + line("A座 / 2F / 东区 / 会议室"), [], spaces.filter((node) => node.id !== "area"))[0]!.error).toBeTruthy();
    expect(parseScreenInventory(header + line("A座"), [], spaces, false)[0]!.spaceStatus).toBe("unavailable");
    const empty = parseScreenInventory(header + line(""), [], [], false)[0]!;
    expect(empty.error).toBe(""); expect(empty.spaceStatus).toBe("empty"); expect(empty.input.spaceId).toBeNull();
  });

  it("人工修正保留原始路径和实际 ID；目录移除后重新阻断", () => {
    const original = parseScreenInventory(header + line("现场旧位置"), [], spaces);
    const corrected = revalidateScreenInventory(original.map((row) => ({ ...row, spaceOverrideId: "room" })), [], spaces);
    expect(corrected[0]).toMatchObject({ originalSpacePath: "现场旧位置", spaceStatus: "matched", matchedSpacePath: "A座/2F/东区/会议室", error: "", input: { spaceId: "room" } });
    const removed = revalidateScreenInventory(corrected, [], spaces.filter((node) => node.id !== "room"));
    expect(removed[0]!.error).toContain("已失效");
    const undetermined = revalidateScreenInventory(removed.map((row) => ({ ...row, spaceOverrideId: null })), [], [], false);
    expect(undetermined[0]!.error).toBe(""); expect(undetermined[0]!.originalSpacePath).toBe("现场旧位置");
  });

  it("保留尺寸/IP/MAC校验、重复提示、引号逗号及真实文件行号", () => {
    const snapshot = createScreenSnapshot();
    const content = '\uFEFF' + header + '"会议室,北",192.0.2.180,10,,A座,门口\n\n重复,192.0.2.180,4,,A座,\n错误,999.0.2.1,7,xxx,,\n平台同IP,192.0.2.31,10,,,\n同MAC,192.0.2.182,10,' + snapshot.screens[0]!.mac + ',,门口';
    const rows = parseScreenInventory(content, snapshot.screens, spaces);
    expect(rows[0]!.input.name).toBe("会议室,北"); expect(rows[1]!.line).toBe(4);
    expect(rows[1]!.error).toContain("文件内 IP 重复");
    expect(rows[2]!.error).toContain("IPv4"); expect(rows[2]!.error).toContain("MAC"); expect(rows[2]!.error).toContain("尺寸");
    expect(rows[3]!.warning).toContain("平台 IP"); expect(rows[4]!.warning).toContain("相同 MAC");
    expect(revalidateScreenInventory(rows, snapshot.screens, spaces)[2]!.error).toContain("尺寸");
  });

  it("表头可调整顺序，但拒绝旧楼幢楼层和缺失空间列", () => {
    expect(parseScreenInventory("空间路径,名称,尺寸,IP\nA座,门口屏,4,192.0.2.180", [], spaces)[0]!.input.spaceId).toBe("a");
    expect(() => parseScreenInventory("名称,IP,尺寸,楼幢,楼层\n屏,192.0.2.180,4,A座,2F", [], spaces)).toThrow("新清单模板");
    expect(() => parseScreenInventory("名称,IP,尺寸\n屏,192.0.2.180,4", [], spaces)).toThrow("空间路径");
    expect(() => parseScreenInventory(header + '"未闭合', [], spaces)).toThrow("引号未闭合");
  });

  it("导出只包含有效且可唯一匹配的完整路径，CSV带UTF-8标记", () => {
    const available = [...spaces, { id: "orphan", name: "孤立区域", kind: "area" as const, parentId: "missing" }, { ...spaces[3]!, id: "duplicate" }];
    expect(inventorySpacePaths(available)).toEqual(["A座", "A座/2F", "A座/2F/东区"]);
    expect(inventorySpacePaths(available, false)).toEqual([]);
    expect(inventorySpacePathsCsv(['A座 / 2F / "会议室"'])).toBe('\uFEFF空间路径\r\n"A座/2F/""会议室"""\r\n');
  });
});
