import { isTauriRuntime } from "@/shared/api/activity";

/** 生产构建永不开放原型；桌面开发须由专用启动命令显式启用。 */
export function isPrototypePreview(): boolean {
  return import.meta.env.DEV && (!isTauriRuntime() || import.meta.env.VITE_SCREEN_PROTOTYPE === "1");
}
