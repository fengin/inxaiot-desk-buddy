import { describe, expect, it } from "vitest";
import { screenRemoteTextError, screenTapPoint } from "./screenRemoteInput";

describe("单屏辅助操作坐标及输入", () => {
  it("横屏在正方形窗口上下留白，留白不发送，内容按原图像素换算", () => {
    const rect = { left: 100, top: 50, width: 1000, height: 1000 };
    expect(screenTapPoint(600, 550, rect, 800, 480)).toEqual({ x: 400, y: 240 });
    expect(screenTapPoint(600, 150, rect, 800, 480)).toBeNull();
    expect(screenTapPoint(100, 250, rect, 800, 480)).toEqual({ x: 0, y: 0 });
    expect(screenTapPoint(1100, 550, rect, 800, 480)).toBeNull();
    expect(screenTapPoint(600, 850, rect, 800, 480)).toBeNull();
  });
  it("竖屏左右留白、缩放和偏移正确，右下像素不越界", () => {
    const rect = { left: 30, top: 20, width: 800, height: 400 };
    expect(screenTapPoint(430, 220, rect, 480, 800)).toEqual({ x: 240, y: 400 });
    expect(screenTapPoint(50, 220, rect, 480, 800)).toBeNull();
    expect(screenTapPoint(549.9, 419.9, rect, 480, 800)).toEqual({ x: 479, y: 799 });
  });
  it("无效或零大小图片不生成点击", () => {
    expect(screenTapPoint(1, 1, { left: 0, top: 0, width: 0, height: 500 }, 800, 480)).toBeNull();
    expect(screenTapPoint(Number.NaN, 1, { left: 0, top: 0, width: 500, height: 500 }, 800, 480)).toBeNull();
  });
  it("允许ASCII符号和有意义的空格，拒绝中文Emoji与控制字符而不回显原值", () => {
    expect(screenRemoteTextError(" user!@#'$%s\\\"&;|<> password ")).toBe("");
    expect(screenRemoteTextError(" ")).toBe("");
    for (const value of ["中文密码", "smile🙂", "line\nnext", "tab\t", "zero\0"]) {
      expect(screenRemoteTextError(value)).toContain("暂不支持");
      expect(screenRemoteTextError(value)).not.toContain(value);
    }
    expect(screenRemoteTextError("A".repeat(129))).toContain("128");
    expect(screenRemoteTextError("")).toBe("请输入内容");
  });
});
