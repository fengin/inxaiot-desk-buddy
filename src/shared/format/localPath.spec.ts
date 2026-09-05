import { describe, expect, it } from "vitest";

import { formatDisplayLocalPath } from "@/shared/format/localPath";

describe("本地路径显示格式化", () => {
  it("隐藏Windows扩展盘符路径前缀", () => {
    expect(formatDisplayLocalPath("\\\\?\\D:\\release\\emqx.tar"))
      .toBe("D:\\release\\emqx.tar");
  });

  it("把Windows扩展UNC路径恢复为普通UNC路径", () => {
    expect(formatDisplayLocalPath("\\\\?\\UNC\\server\\share\\emqx.tar"))
      .toBe("\\\\server\\share\\emqx.tar");
  });

  it("保留普通Windows、POSIX和空路径", () => {
    expect(formatDisplayLocalPath("D:\\release\\emqx.tar")).toBe("D:\\release\\emqx.tar");
    expect(formatDisplayLocalPath("/opt/release/emqx.tar")).toBe("/opt/release/emqx.tar");
    expect(formatDisplayLocalPath(undefined)).toBe("");
  });
});
