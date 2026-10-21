import { createPinia } from "pinia";
import { defineComponent, h, watch } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import { NCheckbox, NMessageProvider, NSelect } from "naive-ui";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import type { ScreenAction } from "@/shared/model/screen";
import type { ScreenRegistrationPreview as RegistrationPreview } from "@/shared/model/screenRegistration";
import ScreenOperations from "./ScreenOperations.vue";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenRegistrationPreview from "./ScreenRegistrationPreview.vue";

function context(operation: ScreenAction = "ping") {
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 20);
  configureScreenAdapter(adapter);
  const store = useSmartScreensStore(pinia), snapshot = createScreenSnapshot(), sample = snapshot.screens[0]!;
  snapshot.screens = Array.from({ length: 4 }, (_, index) => ({ ...sample, id: `selection-${index + 1}`, name: `选屏测试${index + 1}`, source: "platform" as const, ip: `192.0.2.${index + 1}`, mac: `02:00:00:00:00:0${index + 1}`, size: index < 2 ? "10" as const : "4" as const, aliases: [] }));
  snapshot.tasks = []; snapshot.platformDrafts = {};
  store.projectId = "selection-project"; store.snapshot = snapshot; store.operation = operation;
  const ids = snapshot.screens.map(screen => screen.id);
  return { pinia, adapter, store, ids, dispose() { store.stop(); adapter.dispose(); } };
}
async function render(operation: ScreenAction = "ping") {
  const ctx = context(operation);
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, { default: () => h(ScreenOperations) }) }), { global: { plugins: [ctx.pinia], stubs: { teleport: true } } });
  await flushPromises();
  const button = (text: string) => wrapper.findAll("button").find(item => item.text() === text)!;
  const click = async (text: string) => { await button(text).trigger("click"); await flushPromises(); };
  const size = async (value: string) => { wrapper.getComponent(ScreenTargetPicker).findAllComponents(NSelect).find(item => item.attributes("aria-label") === "运维屏尺寸")!.vm.$emit("update:value", value); await flushPromises(); };
  const row = (index: number) => wrapper.getComponent(ScreenTargetPicker).findAll(".screen-target-row")[index]!.getComponent(NCheckbox);
  const file = async () => { const input = wrapper.get<HTMLInputElement>('[data-testid="screen-apk-input"]'); Object.defineProperty(input.element, "files", { configurable: true, value: [new File(["apk"], "selection.apk")] }); await input.trigger("change"); await flushPromises(); };
  return { ...ctx, wrapper, click, size, row, file, dispose() { wrapper.unmount(); ctx.dispose(); } };
}

describe("智能屏批量选择", () => {
  it("登记所选资料提示按有效差异显示，平台已收敛的raw草稿不继续提示待更新", async () => {
    const view = await render("register");
    try {
      const screen = view.store.snapshot.screens[0]!, base = screenPlatformFields(screen);
      view.store.snapshot.platformDrafts = { [screen.id]: { screenId: screen.id, revision: 4, baseRevision: screen.revision, base,
        values: { ...base, name: "拟更新名称", mac: screen.mac.replaceAll(":", "-") }, updatedAt: "now" } };
      view.store.selectedIds = [screen.id]; await flushPromises();
      expect(view.wrapper.get(".screen-registration-selected-row").text()).toContain("资料待更新");
      screen.name = "拟更新名称"; screen.ip = "192.0.2.240"; await flushPromises();
      const row = view.wrapper.get(".screen-registration-selected-row");
      expect(row.text()).not.toContain("资料待更新"); expect(row.text()).toContain("已注册");
      expect(row.text()).toContain(screen.ip); expect(row.text()).toContain(screen.name);
      expect(view.store.snapshot.platformDrafts[screen.id]!.revision).toBe(4);
    } finally { view.dispose(); }
  });

  it("一次更新选择集合，保持原顺序、筛选外记录及去重", () => {
    const view = context();
    try {
      view.store.selectedIds = [view.ids[3]!];
      const changed = vi.fn(), stop = watch(() => view.store.selectedIds, changed, { flush: "sync" });
      view.store.toggleSelections([view.ids[0]!, view.ids[1]!, view.ids[0]!], true);
      expect(view.store.selectedIds).toEqual([view.ids[3], view.ids[0], view.ids[1]]); expect(changed).toHaveBeenCalledOnce();
      changed.mockClear(); view.store.toggleSelections([view.ids[1]!, view.ids[0]!], false);
      expect(view.store.selectedIds).toEqual([view.ids[3]]); expect(changed).toHaveBeenCalledOnce(); stop();
    } finally { view.dispose(); }
  });
  it("空范围、已选择项和未选择项的重复设置不触发无意义更新", () => {
    const view = context();
    try {
      view.store.toggleSelection(view.ids[0]!, true);
      const changed = vi.fn(), stop = watch(() => view.store.selectedIds, changed, { flush: "sync" });
      view.store.toggleSelections([], true); view.store.toggleSelections([view.ids[0]!], true); view.store.toggleSelections([view.ids[1]!], false);
      expect(changed).not.toHaveBeenCalled();
      view.store.toggleSelection(view.ids[1]!, true); view.store.toggleSelection(view.ids[0]!, false);
      expect(view.store.selectedIds).toEqual([view.ids[1]]); expect(changed).toHaveBeenCalledTimes(2); stop();
    } finally { view.dispose(); }
  });
  it("当前页批量选择也只更新一次，并保留其他页选择", () => {
    const view = context();
    try {
      view.store.pageSize = 2; view.store.page = 2; view.store.selectedIds = [view.ids[0]!];
      const changed = vi.fn(), stop = watch(() => view.store.selectedIds, changed, { flush: "sync" });
      view.store.selectPage(true); expect(view.store.selectedIds).toEqual([view.ids[0], view.ids[2], view.ids[3]]); expect(changed).toHaveBeenCalledOnce();
      changed.mockClear(); view.store.selectPage(false); expect(view.store.selectedIds).toEqual([view.ids[0]]); expect(changed).toHaveBeenCalledOnce(); stop();
    } finally { view.dispose(); }
  });
  it("操作页全选与取消仅影响当前匹配，单台勾选仍独立生效", async () => {
    const view = await render();
    try {
      view.store.selectedIds = [view.ids[3]!]; await view.size("10");
      const changed = vi.fn(), stop = watch(() => view.store.selectedIds, changed, { flush: "sync" });
      await view.click("选择全部"); expect(view.store.selectedIds).toEqual([view.ids[3], view.ids[0], view.ids[1]]); expect(changed).toHaveBeenCalledOnce();
      changed.mockClear(); await view.click("取消全选"); expect(view.store.selectedIds).toEqual([view.ids[3]]); expect(changed).toHaveBeenCalledOnce();
      view.row(0).vm.$emit("update:checked", true); await flushPromises(); expect(view.store.selectedIds).toEqual([view.ids[3], view.ids[0]]); stop();
    } finally { view.dispose(); }
  });
  it("批量加入另一尺寸清除已选安装包，移除后必须重新选包", async () => {
    const view = await render("install");
    try {
      view.row(0).vm.$emit("update:checked", true); await flushPromises(); await view.file();
      expect(view.wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(true);
      await view.size("4"); await view.click("选择全部");
      expect(view.store.selectedIds).toEqual([view.ids[0], view.ids[2], view.ids[3]]);
      expect(view.wrapper.get('[data-testid="screen-apk-size-warning"]').text()).toContain("一批只能操作同规格尺寸屏");
      expect(view.wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
      await view.click("取消全选"); expect(view.store.selectedIds).toEqual([view.ids[0]]);
      expect(view.wrapper.find('[data-testid="screen-apk-size-warning"]').exists()).toBe(false);
      expect(view.wrapper.get('[data-testid="screen-operation-first-action"]').attributes("disabled")).toBeDefined();
    } finally { view.dispose(); }
  });
  it("非登记操作不序列化登记资料，切到登记后仍对选择和目录同步失效", async () => {
    const view = await render();
    const stringify = vi.spyOn(JSON, "stringify");
    const fingerprints = () => stringify.mock.calls.filter(([value]) => value && typeof value === "object" && "identities" in value && "spaces" in value && "targets" in value);
    try {
      await view.click("选择全部"); await view.click("取消全选"); expect(fingerprints()).toHaveLength(0);
      view.store.operation = "register"; view.store.toggleSelection(view.ids[0]!, true); await flushPromises(); expect(fingerprints().length).toBeGreaterThan(0);
      vi.spyOn(view.adapter, "previewPlatformRegistration").mockImplementation(async (projectId, ids): Promise<RegistrationPreview> => ({ id: "selection-preview", projectId, createdAt: "2026-10-20T00:00:00Z", items: view.store.snapshot.screens.filter(screen => ids.includes(screen.id)).map(screen => ({ screenId: screen.id, mode: "update", state: "ready", reason: "通过", before: screenPlatformFields(screen), after: screenPlatformFields(screen), diffs: [], expectedRevision: screen.revision, needsSpaceConfirmation: false, macSource: "collected", macMessage: "已有确认", requiredMacConfirmation: null, duplicateIds: [] })) }));
      await view.wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises(); expect(view.wrapper.findComponent(ScreenRegistrationPreview).exists()).toBe(true);
      view.store.toggleSelections([view.ids[1]!], true); await flushPromises(); expect(view.wrapper.findComponent(ScreenRegistrationPreview).exists()).toBe(false); expect(view.wrapper.text()).toContain("请重新检查并预览");
      await view.wrapper.get('[data-testid="screen-operation-first-action"]').trigger("click"); await flushPromises();
      view.store.snapshot.spaces[0]!.name += "更新"; await flushPromises(); expect(view.wrapper.findComponent(ScreenRegistrationPreview).exists()).toBe(false); expect(view.wrapper.text()).toContain("请重新检查并预览");
    } finally { stringify.mockRestore(); view.dispose(); }
  });
  it("切换项目清理旧选择与安装包，不携带旧批次", async () => {
    const view = await render("install");
    try {
      view.store.toggleSelection(view.ids[0]!, true); await flushPromises(); await view.file();
      vi.spyOn(view.adapter, "load").mockResolvedValue({ ...createScreenSnapshot(), spacesAvailable: true });
      await view.store.bindProject("selection-project-b"); await flushPromises();
      expect(view.store.selectedIds).toEqual([]); expect(view.store.projectId).toBe("selection-project-b");
      expect(view.wrapper.find('[data-testid="screen-apk-selected"]').exists()).toBe(false);
    } finally { view.dispose(); }
  });
});
