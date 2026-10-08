import { describe, expect, it } from "vitest";
import { buildProjectSpaceTree, getProjectSpacePath, projectSpacePath, resolveProjectSpacePath } from "./projectSpace";
import type { ProjectSpaceNode } from "./projectSpace";
import { screenSpaces } from "@/dev-fixtures/screenData";

describe("公共项目空间目录", () => {
  it("完整树与路径支持五层及中间节点选择", () => {
    expect(getProjectSpacePath(screenSpaces, "area-a-1-room")?.length).toBe(5);
    expect(projectSpacePath(screenSpaces, "area-a-1-room")).toBe("A座/1F/东区/会议区/会议室");
    const tree = buildProjectSpaceTree(screenSpaces);
    expect(tree[0]?.value).toBe("building-a");
    expect(tree[0]?.children?.[0]?.children?.[0]?.children?.[0]?.children?.[0]?.value).toBe("area-a-1-room");
    expect(resolveProjectSpacePath("A座", screenSpaces)).toMatchObject({ status: "matched", spaceId: "building-a" });
    expect(resolveProjectSpacePath("A座/1F / 东区", screenSpaces)).toMatchObject({ status: "matched", spaceId: "area-a-1-east", path: "A座/1F/东区" });
  });

  it("只接受完整唯一路径，不根据叶节点名、后缀或连字符推断", () => {
    expect(resolveProjectSpacePath("会议室", screenSpaces).status).toBe("missing");
    expect(resolveProjectSpacePath("A座-1F-东区", screenSpaces).status).toBe("missing");
    const ambiguous = [...screenSpaces, { id: "second-a", name: "A座", kind: "building" as const }];
    expect(resolveProjectSpacePath("A座", ambiguous).status).toBe("ambiguous");
    const hyphen: ProjectSpaceNode[] = [{ id: "x", name: "A-1座", kind: "building" }];
    expect(resolveProjectSpacePath("A-1座", hyphen).spaceId).toBe("x");
  });

  it("空路径可明确待定，非空路径在目录不可用时不误匹配", () => {
    expect(resolveProjectSpacePath("", [], false)).toEqual({ status: "empty", path: "", error: "" });
    expect(resolveProjectSpacePath("A座 / 1F", screenSpaces, false).status).toBe("unavailable");
  });

  it("断开的父链、循环或重复ID不生成可选空间，也不无限递归", () => {
    const broken: ProjectSpaceNode[] = [
      { id: "orphan", parentId: "missing", name: "断链", kind: "area" },
      { id: "x", parentId: "y", name: "X", kind: "area" },
      { id: "y", parentId: "x", name: "Y", kind: "area" },
      { id: "dup", name: "1", kind: "building" }, { id: "dup", name: "2", kind: "building" }
    ];
    expect(buildProjectSpaceTree(broken)).toEqual([]);
    for (const node of broken) expect(getProjectSpacePath(broken, node.id)).toBeUndefined();
  });
});
