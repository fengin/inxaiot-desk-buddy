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
import type { WorkbenchSchemaStatus } from "@/shared/model/project";

const fullAppMountTimeout = 30_000;

function schemaStatus(state: WorkbenchSchemaStatus["state"]): WorkbenchSchemaStatus {
  return {
    state,
    currentVersion: state === "upgrade_required" ? 1 : undefined,
    latestAvailableVersion: 2,
    appliedMigrationCount: state === "ready" ? 2 : 1,
    failedMigrationCount: 0,
    missingTables: state === "incompatible" ? ["aio_node"] : [],
    forbiddenTables: [],
    message: "仅供测试的内部结构信息"
  };
}

class SchemaStatusAdapter extends FixtureWorkbenchAdapter {
  constructor(private readonly currentSchemaStatus: WorkbenchSchemaStatus) {
    super();
  }

  override async getWorkbenchSchemaStatus(): Promise<WorkbenchSchemaStatus> {
    return structuredClone(this.currentSchemaStatus);
  }
}

async function openCurrentProjectEditor(wrapper: ReturnType<typeof mount>) {
  await wrapper.get('[data-testid="project-switcher"]').trigger("click");
  await flushPromises();
  const editButton = Array.from(document.querySelectorAll<HTMLButtonElement>("button"))
    .find((button) => button.textContent?.trim() === "编辑当前项目");
  expect(editButton).toBeDefined();
  editButton!.click();
  await flushPromises();
}

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
    await wrapper.get('[data-testid="open-about"]').trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("一体机清单管理、发布参数配置、部署升级和操作记录查询");
    expect(document.body.textContent).toContain("客户端标识");
    expect(document.body.textContent).toContain("DEMO-PC-001122AABBCC-192.0.2.142");
    expect(document.body.textContent).not.toContain("com.inxaiot.desk-buddy");
    expect(document.body.textContent).toContain("作者");
    expect(document.body.textContent).toContain("凌封");
    expect(document.body.textContent).not.toContain("Tauri Real Adapter");
    expect(document.body.textContent).not.toContain("Agent SHA-256");
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
    expect(document.body.textContent).toContain("暂时无法读取");
    expect(document.body.textContent).not.toContain("diagnostics unavailable");
    wrapper.unmount();
  }, fullAppMountTimeout);

  it("hides schema metadata when the current project is compatible", async () => {
    configureWorkbenchAdapter(new SchemaStatusAdapter(schemaStatus("ready")));
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    try {
      await flushPromises();
      await openCurrentProjectEditor(wrapper);
      expect(document.querySelector('[data-testid="schema-maintenance"]')).toBeNull();
      expect(document.body.textContent).not.toContain("当前版本");
      expect(document.body.textContent).not.toContain("可用版本");
      expect(document.body.textContent).not.toContain("检查结构");
    } finally {
      wrapper.unmount();
    }
  }, fullAppMountTimeout);

  it("shows only a user-facing compatibility action when an upgrade is required", async () => {
    configureWorkbenchAdapter(new SchemaStatusAdapter(schemaStatus("upgrade_required")));
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    try {
      await flushPromises();
      await openCurrentProjectEditor(wrapper);
      expect(document.body.textContent).toContain("工作台数据库结构未就绪");
      expect(document.body.textContent).toContain("初始化/升级");
      // 清单现保存在本机，平台会话有效时可查看；发布参数仍要求共享库就绪。
      expect(document.querySelector('[data-testid="aio-create-node"]')?.getAttribute("disabled")).toBeNull();
      await router.push("/aio/release"); await flushPromises();
      expect(document.body.textContent).toContain("工作台数据库尚未就绪，请在项目设置中处理。");
      expect(document.body.textContent).not.toContain("当前项目尚未登录平台，请在项目切换器中完成登录。");
      expect(document.body.textContent).not.toContain("当前版本");
      expect(document.body.textContent).not.toContain("可用版本");
      expect(document.body.textContent).not.toContain("仅供测试的内部结构信息");
    } finally {
      wrapper.unmount();
    }
  }, fullAppMountTimeout);

  it("does not offer schema migration for an incompatible project", async () => {
    configureWorkbenchAdapter(new SchemaStatusAdapter(schemaStatus("incompatible")));
    const { default: App } = await import("@/app/App.vue");
    await router.push("/aio/nodes");
    await router.isReady();
    const wrapper = mount(App, {
      attachTo: document.body,
      global: { plugins: [createPinia(), router, i18n] }
    });
    try {
      await flushPromises();
      await openCurrentProjectEditor(wrapper);
      expect(document.body.textContent).toContain("工作台数据库结构未就绪");
      expect(document.querySelector('[data-testid="schema-upgrade"]')).toBeNull();
      expect(document.body.textContent).toContain("重新检查");
    } finally {
      wrapper.unmount();
    }
  }, fullAppMountTimeout);
});
