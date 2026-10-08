import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import type { ComponentPublicInstance } from "vue";

import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { useReleaseProfileStore } from "@/stores/releaseProfile";

type StatusInput = ComponentPublicInstance<{ status?: string }>;

class CredentialsResetRequiredAdapter extends FixtureWorkbenchAdapter {
  override async getReleaseProfile(projectId: string) {
    const profile = await super.getReleaseProfile(projectId);
    return profile && { ...profile, credentialsResetRequired: true };
  }
}

describe("发布参数页面", () => {
  it("shows the effective agent and warns before replacement", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
    const selectFile = vi.fn(async () => "D:\\release\\verified-agent.sh");
    configureSystemDialogAdapter({
      real: false,
      async selectDirectory() { return null; },
      selectFile
    });
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/release");
    await router.isReady();
    const wrapper = mount(App, { attachTo: document.body, global: { plugins: [createPinia(), router, i18n] } });
    try {
      await flushPromises();
      expect(wrapper.text()).toContain("host-info.json");
      expect((wrapper.get('[data-testid="release-agent-script"] input').element as HTMLInputElement).value)
        .toBe("edge-node-agent.sh · v0.1.9 · 内置");
      await wrapper.get('[data-testid="release-edit"]').trigger("click");
      await wrapper.get('[data-testid="release-agent-replace"]').trigger("click");
      await flushPromises();
      expect(selectFile).not.toHaveBeenCalled();
      expect(document.body.textContent).toContain("非必要情况下请勿更换一体机脚本");
      expect(document.body.textContent).toContain("当前工作台支持的部署、升级和回滚操作");
      const confirm = Array.from(document.body.querySelectorAll("button"))
        .find((button) => button.textContent?.includes("确认更换"));
      expect(confirm).toBeDefined();
      confirm?.click();
      await flushPromises();
      expect(selectFile).toHaveBeenCalledTimes(1);
      expect((wrapper.get('[data-testid="release-agent-script"] input').element as HTMLInputElement).value)
        .toBe("edge-node-agent.sh · v0.1.9 · 项目配置");
      expect(wrapper.get('[data-testid="release-agent-view"]').text()).toBe("查看");
    } finally {
      wrapper.unmount();
    }
  }, 30000);

  it("prompts for re-entry when credentials need resetting and clears the prompt after a normal save", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    configureWorkbenchAdapter(new CredentialsResetRequiredAdapter());
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/release");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, { attachTo: document.body, global: { plugins: [pinia, router, i18n] } });
    try {
      await flushPromises();
      expect(wrapper.get('[data-testid="release-credentials-reset-required"]').text())
        .toBe("发布凭据加密格式已更新，请重新填写发布参数");

      await wrapper.get('[data-testid="release-edit"]').trigger("click");
      const release = useReleaseProfileStore(pinia);
      release.draft.credentials.platformAuthKey = "re-entered-platform-auth-key";
      await wrapper.get('[data-testid="release-save"]').trigger("click");
      await flushPromises();

      expect(wrapper.find('[data-testid="release-credentials-reset-required"]').exists()).toBe(false);
    } finally {
      wrapper.unmount();
    }
  }, 30000);

  it("marks the exact invalid inputs and template, then clears corrected input feedback", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }))
    });
    const adapter = new FixtureWorkbenchAdapter();
    vi.spyOn(adapter, "validateReleaseProfile").mockRejectedValueOnce({
      code: "CONFIG_VALIDATION_FAILED",
      params: { summary: "请修正标红的输入项" },
      fieldErrors: {
        "credentials.platformAuthKey": "请填写平台 API AuthKey",
        "values.platformApiPort": "端口必须是 1～65535 的整数",
        "values.envTemplate": ".env 模板存在未知变量"
      }
    });
    vi.spyOn(adapter, "saveReleaseProfile").mockRejectedValueOnce({
      code: "CONFIG_VALIDATION_FAILED",
      params: { summary: "请修正标红的输入项" },
      fieldErrors: { "credentials.sshPrivateKey": "SSH 私钥格式或算法不正确" }
    });
    configureWorkbenchAdapter(adapter);
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/release");
    await router.isReady();
    const pinia = createPinia();
    const wrapper = mount(App, { attachTo: document.body, global: { plugins: [pinia, router, i18n] } });
    try {
      await flushPromises();
      await wrapper.get('[data-testid="release-edit"]').trigger("click");
      await wrapper.get('[data-testid="release-validate"]').trigger("click");
      await flushPromises();
      for (const id of ["release-auth-key", "release-api-port", "release-env-template"]) {
        expect(wrapper.getComponent<StatusInput>(`[data-testid="${id}"]`).props("status")).toBe("error");
      }
      expect(wrapper.getComponent<StatusInput>('[data-testid="release-platform-host"]').props("status")).toBeUndefined();
      expect(wrapper.get('[data-testid="release-auth-key"] input').attributes("aria-invalid")).toBe("true");
      expect(wrapper.get('[id="release-error-credentials.platformAuthKey"]').text()).toBe("请填写平台 API AuthKey");
      expect(wrapper.get(".template-footer").text()).toContain(".env 模板存在未知变量");

      await wrapper.get('[data-testid="release-auth-key"] input').setValue("corrected-test-key");
      expect(wrapper.getComponent<StatusInput>('[data-testid="release-auth-key"]').props("status")).toBeUndefined();
      expect(wrapper.find('[id="release-error-credentials.platformAuthKey"]').exists()).toBe(false);
      expect(wrapper.getComponent<StatusInput>('[data-testid="release-api-port"]').props("status")).toBe("error");

      const release = useReleaseProfileStore(pinia);
      release.resetDraft();
      release.draft.credentials.sshPrivateKey = "invalid-test-private-key";
      await wrapper.get('[data-testid="release-save"]').trigger("click");
      await flushPromises();
      expect(wrapper.getComponent<StatusInput>('[data-testid="release-ssh-private-key"]').props("status")).toBe("error");
      expect(wrapper.get('[id="release-error-credentials.sshPrivateKey"]').text()).toContain("私钥格式或算法不正确");
    } finally {
      wrapper.unmount();
    }
  }, 30000);
});
