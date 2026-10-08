import { defineComponent, h, ref } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NMessageProvider } from "naive-ui";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import type { InventoryApplyOutcome, InventoryPreview } from "@/shared/model/aio";
import AioNodeCreateDialog from "./AioNodeCreateDialog.vue";

beforeEach(() => {
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() })) });
});

function render(adapter = new FixtureAioAdapter()) {
  configureAioAdapter(adapter);
  const show = ref(true), projectId = ref("project-a");
  const created = vi.fn();
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, {}, { default: () => h(AioNodeCreateDialog, { show: show.value, projectId: projectId.value, "onUpdate:show": (value: boolean) => { show.value = value; }, onCreated: created }) }) }), { attachTo: document.body, global: { stubs: { teleport: true } } });
  const fill = async (name = "测试新增", ip = "192.0.2.79", mac = "aa-bb-cc-dd-ee-79") => {
    await wrapper.get('[data-testid="aio-create-name"] input').setValue(name);
    await wrapper.get('[data-testid="aio-create-ip"] input').setValue(ip);
    await wrapper.get('[data-testid="aio-create-mac"] input').setValue(mac);
  };
  const save = async () => { await wrapper.get('[data-testid="aio-create-save"]').trigger("click"); await flushPromises(); };
  return { adapter, wrapper, show, projectId, created, fill, save };
}

describe("单台新增一体机", () => {
  it("名称、IP、MAC 必填；完整填写后复用导入应用并关闭预览", async () => {
    const context = render();
    const preview = vi.spyOn(context.adapter, "previewCreate"), apply = vi.spyOn(context.adapter, "applyImport");
    try {
      await context.save();
      expect(context.wrapper.text()).toContain("请填写名称；请填写 IP 地址；请填写 MAC 地址");
      expect(preview).not.toHaveBeenCalled();
      await context.fill(); await context.save();
      expect(apply).toHaveBeenCalledTimes(1);
      expect(context.created).toHaveBeenCalledTimes(1);
      expect(context.show.value).toBe(false);
      expect(await context.adapter.getLatestImport("project-a")).toBeNull();
      const nodes = await context.adapter.listNodes("project-a", { page: 1, pageSize: 100 });
      expect(nodes.items.find((node) => node.macNormalized === "AABBCCDDEE79")).toMatchObject({ name: "测试新增", ip: "192.0.2.79", managementState: "pending" });
    } finally { context.wrapper.unmount(); }
  });

  it("已有 CSV 预览不覆盖，提示先处理", async () => {
    const adapter = new FixtureAioAdapter();
    const original = await adapter.previewImport("project-a", "C:/original.csv");
    const context = render(adapter), apply = vi.spyOn(adapter, "applyImport");
    try {
      await context.fill(); await context.save();
      expect(context.wrapper.text()).toContain("请先处理或放弃后再新增一体机");
      expect(await adapter.getLatestImport("project-a")).toEqual(original.session);
      expect(apply).not.toHaveBeenCalled();
    } finally { context.wrapper.unmount(); }
  });

  it("已有 MAC 拒绝重复新增，不更新原资料", async () => {
    const context = render();
    const nodes = await context.adapter.listNodes("project-a", { page: 1, pageSize: 100 });
    const existing = nodes.items.find((node) => node.managementState !== "platform_existing")!;
    try {
      await context.fill("修改已有名称", "192.0.2.250", existing.mac); await context.save();
      expect(context.wrapper.text()).toContain("无需重复新增");
      expect((await context.adapter.getNodeDetail("project-a", existing.mac)).node.name).toBe(existing.name);
      expect(context.created).not.toHaveBeenCalled();
    } finally { context.wrapper.unmount(); }
  });

  it("平台接管要二次确认；取消后释放自己创建的预览", async () => {
    const context = render();
    const nodes = await context.adapter.listNodes("project-a", { page: 1, pageSize: 100 });
    const platformNode = nodes.items.find((node) => node.managementState === "platform_existing")!;
    const apply = vi.spyOn(context.adapter, "applyImport");
    try {
      await context.fill(platformNode.name, platformNode.ip, platformNode.mac); await context.save();
      expect(context.wrapper.text()).toContain("确认接管");
      expect(apply).not.toHaveBeenCalled();
      await context.wrapper.findAll("button").find((button) => button.text() === "取消")!.trigger("click");
      await flushPromises();
      expect(context.show.value).toBe(false);
      expect(await context.adapter.getLatestImport("project-a")).toBeNull();
    } finally { context.wrapper.unmount(); }
  });

  it("平台接管确认后保存一次，同 MAC 再次新增被拒绝", async () => {
    const context = render();
    const nodes = await context.adapter.listNodes("project-a", { page: 1, pageSize: 100 });
    const platformNode = nodes.items.find((node) => node.managementState === "platform_existing")!;
    const apply = vi.spyOn(context.adapter, "applyImport");
    try {
      await context.fill(platformNode.name, platformNode.ip, platformNode.mac); await context.save();
      expect(apply).not.toHaveBeenCalled();
      await context.save();
      expect(apply).toHaveBeenCalledTimes(1);
      expect(context.created).toHaveBeenCalledTimes(1);
      expect(await context.adapter.getLatestImport("project-a")).toBeNull();
      await expect(context.adapter.previewCreate("project-a", { name: platformNode.name, ip: platformNode.ip, mac: platformNode.mac })).rejects.toThrow("无需重复新增");
    } finally { context.wrapper.unmount(); }
  });

  it("切换项目后，迟到的新增预览被清理而不应用", async () => {
    const context = render();
    const original = context.adapter.previewCreate.bind(context.adapter);
    let finish!: (result: InventoryPreview) => void;
    const pending = new Promise<InventoryPreview>((resolve) => { finish = resolve; });
    vi.spyOn(context.adapter, "previewCreate").mockImplementation(() => pending);
    const apply = vi.spyOn(context.adapter, "applyImport");
    try {
      await context.fill(); await context.wrapper.get('[data-testid="aio-create-save"]').trigger("click");
      const result = await original("project-a", { name: "测试新增", ip: "192.0.2.79", mac: "aa-bb-cc-dd-ee-79" });
      context.projectId.value = "project-b";
      await flushPromises();
      finish(result); await flushPromises();
      expect(apply).not.toHaveBeenCalled();
      expect(await context.adapter.getLatestImport("project-a")).toBeNull();
      expect(context.created).not.toHaveBeenCalled();
    } finally { context.wrapper.unmount(); }
  });

  it("旧项目保存未返回时，新项目可独立新增，旧结果不关闭新表单", async () => {
    const context = render();
    const original = context.adapter.applyImport.bind(context.adapter);
    const finish = new Map<string, () => Promise<void>>();
    const apply = vi.spyOn(context.adapter, "applyImport").mockImplementation((project, session) => new Promise<InventoryApplyOutcome>((resolve) => {
      finish.set(project, async () => { resolve(await original(project, session)); });
    }));
    const discard = vi.spyOn(context.adapter, "discardImport");
    try {
      await context.fill(); await context.save();
      expect(finish.has("project-a")).toBe(true);
      context.show.value = false; context.projectId.value = "project-b";
      await flushPromises(); context.show.value = true; await flushPromises();
      expect(context.wrapper.get('[data-testid="aio-create-name"] input').attributes("disabled")).toBeUndefined();
      await context.fill("第二项目新机器", "192.0.2.80", "aa-bb-cc-dd-ee-80"); await context.save();
      expect(apply.mock.calls.map((call) => call[0])).toEqual(["project-a", "project-b"]);
      await finish.get("project-a")!(); await flushPromises();
      expect(context.show.value).toBe(true);
      expect(context.wrapper.get('[data-testid="aio-create-name"] input').element).toHaveProperty("value", "第二项目新机器");
      expect(context.wrapper.get('[data-testid="aio-create-save"]').attributes("disabled")).toBeDefined();
      expect(context.created).not.toHaveBeenCalled();
      expect(discard).not.toHaveBeenCalled();
      await finish.get("project-b")!(); await flushPromises();
      expect(context.show.value).toBe(false);
      expect(context.created).toHaveBeenCalledTimes(1);
    } finally { context.wrapper.unmount(); }
  });
});
