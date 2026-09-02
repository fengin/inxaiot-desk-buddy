import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { dateZhCN, NConfigProvider, zhCN } from "naive-ui";
import { describe, expect, it, vi } from "vitest";

import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { configureDataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import { configureDiagnosticsAdapter } from "@/shared/api/diagnosticsAdapter";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";

const fullAppMountTimeout = 30_000;

describe("desktop demo shell", () => {
  it("uses Chinese inputs and keeps the refreshed login session identifier internal", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockImplementation(() => ({
        matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn()
      }))
    });
    const adapter = new FixtureWorkbenchAdapter();
    const challenge = {
      sessionUuid: "unit-challenge-first", captchaImageDataUrl: undefined,
      requiresCaptcha: true, expiresAtEpochSeconds: Math.floor(Date.now() / 1000) + 300
    };
    vi.spyOn(adapter, "createLoginChallenge")
      .mockResolvedValueOnce(challenge)
      .mockResolvedValue({ ...challenge, sessionUuid: "unit-challenge-refreshed" });
    const login = vi.spyOn(adapter, "loginProject");
    configureWorkbenchAdapter(adapter);
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    try {
      await flushPromises();
      const provider = wrapper.getComponent(NConfigProvider);
      expect(provider.props("locale")).toEqual(zhCN);
      expect(provider.props("dateLocale")).toEqual(dateZhCN);
      await wrapper.get('[data-testid="project-switcher"]').trigger("click");
      await flushPromises();
      const projectButton = Array.from(document.querySelectorAll<HTMLButtonElement>(".project-option"))
        .find((button) => button.textContent?.includes("成都金融中心"));
      expect(projectButton).toBeDefined();
      projectButton!.click();
      await flushPromises();
      expect(document.querySelector('[data-testid="login-session-uuid"]')).toBeNull();
      expect(document.body.textContent).not.toContain("会话标识");
      expect(document.body.textContent).not.toContain(challenge.sessionUuid);
      const fields = [
        ["login-username", "请输入平台账号", "unit-operator"],
        ["login-password", "请输入平台密码", "unit-password"],
        ["login-image-code", "请输入验证码", "unit-code"]
      ];
      for (const [id, placeholder, value] of fields) {
        const input = document.querySelector<HTMLInputElement>(`[data-testid="${id}"] input`)!;
        expect(input.placeholder).toBe(placeholder);
        input.value = value!;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
      await flushPromises();
      document.querySelector<HTMLButtonElement>('[title="刷新验证码"]')!.click();
      await flushPromises();
      const captcha = document.querySelector<HTMLInputElement>('[data-testid="login-image-code"] input')!;
      expect(captcha.value).toBe("");
      captcha.value = "unit-code-after-refresh";
      captcha.dispatchEvent(new Event("input", { bubbles: true }));
      await flushPromises();
      document.querySelector<HTMLButtonElement>('[data-testid="login-submit"]')!.click();
      await flushPromises();
      expect(login).toHaveBeenCalledWith("project-chengdu-center", {
        username: "unit-operator", password: "unit-password",
        imageCode: "unit-code-after-refresh", sessionUuid: "unit-challenge-refreshed"
      });
    } finally {
      wrapper.unmount();
    }
  }, fullAppMountTimeout);

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
