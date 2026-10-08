import { afterEach, describe, expect, it, vi } from "vitest";
import { isPrototypePreview } from "./runtimeMode";

afterEach(() => { vi.unstubAllEnvs(); delete window.__TAURI_INTERNALS__; });
describe("桌面原型显式启用", () => {
  it("普通桌面开发不混入模拟数据", () => {
    vi.stubEnv("DEV", true); vi.stubEnv("VITE_SCREEN_PROTOTYPE", "");
    window.__TAURI_INTERNALS__ = {};
    expect(isPrototypePreview()).toBe(false);
  });
  it("专用启动命令允许桌面开发原型", () => {
    vi.stubEnv("DEV", true); vi.stubEnv("VITE_SCREEN_PROTOTYPE", "1");
    window.__TAURI_INTERNALS__ = {};
    expect(isPrototypePreview()).toBe(true);
  });
  it("生产构建即使带开关仍禁止原型", () => {
    vi.stubEnv("DEV", false); vi.stubEnv("VITE_SCREEN_PROTOTYPE", "1");
    window.__TAURI_INTERNALS__ = {};
    expect(isPrototypePreview()).toBe(false);
  });
  it("保留普通浏览器开发预览", () => {
    vi.stubEnv("DEV", true); vi.stubEnv("VITE_SCREEN_PROTOTYPE", "");
    delete window.__TAURI_INTERNALS__;
    expect(isPrototypePreview()).toBe(true);
  });
});
