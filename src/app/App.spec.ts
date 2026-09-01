import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { configureDataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import { configureDiagnosticsAdapter } from "@/shared/api/diagnosticsAdapter";

const fullAppMountTimeout = 30_000;

describe("desktop demo shell", () => {
  it("renders the project context, business navigation and node page", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockImplementation(() => ({
        matches: false,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn()
      }))
    });
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    await flushPromises();
    expect(wrapper.text()).toContain("INX 实施工作台");
    expect(wrapper.text()).toContain("一体机列表");
    expect(wrapper.text()).toContain("发布参数");
    expect(wrapper.text()).toContain("部署升级");
    expect(wrapper.text()).toContain("深圳湾智慧园区");
    wrapper.unmount();
  }, fullAppMountTimeout);

  it("shows unavailable data-directory and diagnostics states instead of false success", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockImplementation(() => ({
        matches: false,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn()
      }))
    });
    configureDataDirectoryAdapter({
      async getStatus() { throw new Error("directory unavailable"); },
      async scheduleSwitch() { throw new Error("directory unavailable"); },
      async scheduleRollback() { throw new Error("directory unavailable"); }
    });
    configureDiagnosticsAdapter({
      async getSystemDiagnostics() { throw new Error("diagnostics unavailable"); }
    });
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    await flushPromises();
    expect(wrapper.text()).toContain("数据目录状态不可用");

    await wrapper.get('[data-testid="open-about"]').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("diagnostics unavailable");
    expect(document.body.textContent).toContain("重新读取诊断");
    wrapper.unmount();
  }, fullAppMountTimeout);
});
