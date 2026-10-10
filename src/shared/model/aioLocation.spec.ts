import { describe, expect, it } from "vitest";
import { aioAddressAfterSpaceChange, aioAddressError, aioAddressSuggestion } from "./aioLocation";
import type { ProjectSpaceNode } from "./projectSpace";

const spaces: ProjectSpaceNode[] = [
  { id: "1", name: "项目", kind: "other" },
  { id: "2", name: " T1塔楼 ", parentId: "1", kind: "building" },
  { id: "3", name: "20层", parentId: "2", kind: "floor" },
  { id: "4", name: "东侧机房", parentId: "3", kind: "area" },
  { id: "5", name: "独立区域", parentId: "1", kind: "area" },
  { id: "6", name: "地下一层", parentId: "1", kind: "floor" }
];

describe("一体机具体位置", () => {
  it("楼层和区域选择均以楼栋_楼层为建议，不含项目、区域或编号", () => {
    expect(aioAddressSuggestion(spaces, "3")).toBe("T1塔楼_20层");
    expect(aioAddressSuggestion(spaces, "4")).toBe("T1塔楼_20层");
  });

  it("只有楼栋或楼层时仅使用已知名称，不猜测缺失层级", () => {
    expect(aioAddressSuggestion(spaces, "2")).toBe("T1塔楼");
    expect(aioAddressSuggestion(spaces, "6")).toBe("地下一层");
    expect(aioAddressSuggestion(spaces, "1")).toBe("");
    expect(aioAddressSuggestion(spaces, "5")).toBe("");
  });

  it("目录不可用、父链断开或未选空间时不生成虚假的位置", () => {
    expect(aioAddressSuggestion([], "3")).toBe("");
    expect(aioAddressSuggestion(spaces.filter(node => node.id !== "2"), "3")).toBe("");
    expect(aioAddressSuggestion(spaces, "0")).toBe("");
    expect(aioAddressSuggestion(spaces)).toBe("");
  });

  it("更换空间更新建议，取消空间仅清自动建议并保留自定义位置", () => {
    expect(aioAddressAfterSpaceChange(spaces, "3", "6", "T1塔楼_20层")).toBe("地下一层");
    expect(aioAddressAfterSpaceChange(spaces, "3", undefined, "T1塔楼_20层")).toBe("");
    expect(aioAddressAfterSpaceChange(spaces, "3", undefined, "门口弱电柜")).toBe("门口弱电柜");
  });

  it("同类层级嵌套时采用距离所选空间最近的楼栋和楼层", () => {
    const nested: ProjectSpaceNode[] = [...spaces,
      { id: "7", name: "附楼", parentId: "3", kind: "building" },
      { id: "8", name: "夹层", parentId: "7", kind: "floor" }
    ];
    expect(aioAddressSuggestion(nested, "8")).toBe("附楼_夹层");
  });

  it("有空间拒绝空或纯空白，未选空间保持选填，允许自定义内容", () => {
    for (const value of [undefined, "", " \t　 "]) expect(aioAddressError("3", value)).toBe("已选择空间，请填写具体位置");
    expect(aioAddressError("3", "  南侧弱电柜  ")).toBe("");
    expect(aioAddressError(undefined, "")).toBe("");
    expect(aioAddressError("0", "")).toBe("");
  });

  it("位置最多128个字符，超长拒绝且不截断空间建议", () => {
    expect(aioAddressError("3", "柜".repeat(128))).toBe("");
    expect(aioAddressError(undefined, "柜".repeat(129))).toBe("具体位置不能超过 128 个字符");
    expect(aioAddressError("3", "𠮷".repeat(128))).toBe("");
    const address = aioAddressSuggestion([{ id: "1", name: "楼".repeat(129), kind: "building" }], "1");
    expect(address).toHaveLength(129);
    expect(aioAddressError("1", address)).toBe("具体位置不能超过 128 个字符");
  });
});
