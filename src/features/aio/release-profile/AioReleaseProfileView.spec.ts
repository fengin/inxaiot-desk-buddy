import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";

import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";

class MissingLocalKeyAdapter extends FixtureWorkbenchAdapter {
  imported = false;

  override async getReleaseProfile(projectId: string) {
    if (!this.imported) {
      throw {
        code: "CONFLICT",
        params: { summary: "本机缺少当前项目主密钥，请导入加密密钥包" }
      };
    }
    return super.getReleaseProfile(projectId);
  }

  override async importReleaseMasterKey(projectId: string) {
    this.imported = true;
    return super.importReleaseMasterKey(projectId);
  }
}

describe("发布参数项目主密钥恢复入口", () => {
  it("keeps import available when the profile cannot be decrypted on this machine", async () => {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockImplementation(() => ({
        matches: false,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn()
      }))
    });
    const adapter = new MissingLocalKeyAdapter();
    configureWorkbenchAdapter(adapter);
    configureSystemDialogAdapter({
      real: false,
      async selectDirectory() { return null; },
      async selectFile() { return "D:\\secure\\project-key.inxkey"; },
      async saveFile() { return null; }
    });
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/release");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    await flushPromises();

    expect(wrapper.find('[data-testid="host-key-open"]').exists()).toBe(false);
    expect(document.querySelector('[data-testid="host-key-capture"]')).toBeNull();
    const openButton = wrapper.get('[data-testid="release-key-management-open"]');
    expect(openButton.attributes("disabled")).toBeUndefined();
    await openButton.trigger("click");
    await flushPromises();

    const passphrase = document.querySelector<HTMLInputElement>(
      '[data-testid="release-key-passphrase"] input'
    );
    expect(passphrase).not.toBeNull();
    expect(document.querySelector('[data-testid="release-key-export"]')).toBeNull();
    expect(document.querySelector('[data-testid="release-key-rotate"]')).toBeNull();
    passphrase!.value = "strong-passphrase";
    passphrase!.dispatchEvent(new Event("input", { bubbles: true }));
    await flushPromises();
    document
      .querySelector<HTMLButtonElement>('[data-testid="release-key-import"]')!
      .click();
    await flushPromises();

    expect(adapter.imported).toBe(true);
    expect(wrapper.text()).toContain("版本");
    wrapper.unmount();
  }, 30000);
});
