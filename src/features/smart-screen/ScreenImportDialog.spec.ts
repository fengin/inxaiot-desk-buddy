import { createPinia } from "pinia";
import { defineComponent, h, ref } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NCheckbox, NMessageProvider, NPagination } from "naive-ui";
import { FixtureScreenAdapter, SCREEN_STORAGE_PREFIX } from "@/dev-fixtures/screenFixtureAdapter";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";
import ScreenImportDialog from "./ScreenImportDialog.vue";

const spaces: ProjectSpaceNode[] = [
  { id: "a", name: "A座", kind: "building" }, { id: "f", name: "2F", kind: "floor", parentId: "a" },
  { id: "area", name: "东区", kind: "area", parentId: "f" }, { id: "room", name: "会议室", kind: "other", parentId: "area" }
];
const header = "名称,IP,尺寸,MAC,空间路径,安装位置\n";
const valid = "会议屏,192.0.2.180,10,,A座 / 2F / 东区 / 会议室,门口";
const cleanups: (() => void)[] = [];
afterEach(() => { cleanups.splice(0).forEach((cleanup) => cleanup()); vi.restoreAllMocks(); });

async function render(available = true) {
  const project = "import-project", snapshot = createScreenSnapshot();
  snapshot.screens = []; snapshot.spaces = spaces.map((space) => ({ ...space })); snapshot.spacesAvailable = available;
  localStorage.setItem(SCREEN_STORAGE_PREFIX + project, JSON.stringify({ ...snapshot, schemaVersion: 3 }));
  const adapter = new FixtureScreenAdapter(localStorage, 20); configureScreenAdapter(adapter);
  const pinia = createPinia(), store = useSmartScreensStore(pinia); await store.bindProject(project);
  const show = ref(true);
  const wrapper = mount(defineComponent({ setup: () => () => h(NMessageProvider, null, { default: () => h(ScreenImportDialog, {
    show: show.value, "onUpdate:show": (value: boolean) => { show.value = value; }
  }) }) }), { attachTo: document.body, global: { plugins: [pinia], stubs: { teleport: true } } });
  cleanups.push(() => { wrapper.unmount(); store.stop(); adapter.dispose(); });
  await flushPromises();
  const dialog = () => wrapper.findComponent(ScreenImportDialog);
  const click = async (label: string) => { await dialog().findAll("button").find((button) => button.text() === label)!.trigger("click"); await flushPromises(); };
  const preview = async (content: string) => { await dialog().get("textarea").setValue(content); await click("检查清单"); };
  return { wrapper, adapter, project, store, show, dialog, click, preview };
}

describe("智能屏导入空间修正", () => {
  it("默认包含错误行，明确取消错误记录后才只提交合格记录", async () => {
    const { adapter, project, dialog, preview } = await render();
    const save = vi.spyOn(adapter, "importLocal");
    await preview(header + valid + "\n错误屏,999.0.2.1,4,,不存在空间,门口");
    expect(dialog().get('[data-testid="screen-import-summary"]').text()).toContain("已选 2 条（1 条可提交，1 条需修正）");
    expect(dialog().get('[data-testid="screen-import-submit"]').attributes("disabled")).toBeDefined();
    const incorrect = dialog().get('[data-import-line="3"]').getComponent(NCheckbox);
    expect(incorrect.props("checked")).toBe(true); expect(incorrect.props("disabled")).toBe(false);
    incorrect.vm.$emit("update:checked", false); await flushPromises();
    expect(dialog().get('[data-testid="screen-import-summary"]').text()).toContain("不提交 1 条");
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    expect(save).toHaveBeenCalledOnce();
    expect(save.mock.calls[0]![1]).toHaveLength(1);
    expect(save.mock.calls[0]![1][0]).toMatchObject({ spaceId: "room", spacePath: "A座/2F/东区/会议室" });
    expect((await adapter.load(project)).screens).toHaveLength(1);
  });

  it("多行相同错误路径可统一修正，保留原始路径并提交所选真实节点", async () => {
    const { adapter, dialog, preview, click } = await render();
    const save = vi.spyOn(adapter, "importLocal");
    await preview(header + "屏一,192.0.2.180,10,,旧楼层,门口\n屏二,192.0.2.181,4,,旧楼层,门口");
    const batch = dialog().findAllComponents(ProjectSpaceSelect).find((select) => select.attributes("aria-label") === "批量修正导入空间")!;
    batch.vm.$emit("update:modelValue", "area"); await flushPromises();
    await click("应用到选中 2 行");
    expect(dialog().findAll('[data-import-line]').every((row) => row.text().includes("旧楼层") && row.text().includes("人工已修正") && row.text().includes("A座/2F/东区"))).toBe(true);
    expect(dialog().get('[data-testid="screen-import-submit"]').attributes("disabled")).toBeUndefined();
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    expect(save.mock.calls[0]![1].map((row) => row.spaceId)).toEqual(["area", "area"]);
  });

  it("目录不可用时非空路径阻断，可明确设置为待定空间", async () => {
    const { adapter, dialog, preview, click } = await render(false);
    const save = vi.spyOn(adapter, "importLocal");
    await preview(header + valid);
    expect(dialog().text()).toContain("目录不可用");
    expect(dialog().get('[data-testid="screen-import-submit"]').attributes("disabled")).toBeDefined();
    await click("设为待定空间");
    expect(dialog().get('[data-testid="screen-import-submit"]').attributes("disabled")).toBeUndefined();
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    expect(save.mock.calls[0]![1][0]).toMatchObject({ spaceId: null, spacePath: "" });
  });

  it("预览分页不缩小提交范围，跨页错误仍阻断且批量修正覆盖全部已选行", async () => {
    const { adapter, dialog, preview, click } = await render();
    const save = vi.spyOn(adapter, "importLocal");
    const records = Array.from({ length: 21 }, (_, index) => `屏${index + 1},192.0.2.${160 + index},10,,${index === 20 ? "错误路径" : "A座"},门口`);
    await preview(header + records.join("\n"));
    expect(dialog().findAll('[data-import-line]')).toHaveLength(20);
    expect(dialog().get('[data-testid="screen-import-summary"]').text()).toContain("已选 21 条（20 条可提交，1 条需修正）");
    expect(dialog().get('[data-testid="screen-import-submit"]').attributes("disabled")).toBeDefined();
    const batch = dialog().findAllComponents(ProjectSpaceSelect).find((select) => select.attributes("aria-label") === "批量修正导入空间")!;
    batch.vm.$emit("update:modelValue", "area"); await flushPromises(); await click("应用到选中 21 行");
    dialog().getComponent(NPagination).vm.$emit("update:page", 2); await flushPromises();
    expect(dialog().findAll('[data-import-line]')).toHaveLength(1);
    expect(dialog().get('[data-import-line="22"]').text()).toContain("人工已修正");
    expect(dialog().get('[data-testid="screen-import-summary"]').text()).toContain("已选 21 条（21 条可提交，0 条需修正）");
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    expect(save.mock.calls[0]![1]).toHaveLength(21);
    expect(save.mock.calls[0]![1].every((row) => row.spaceId === "area")).toBe(true);
  });

  it("保存前目录改变会停止提交并重新核对，人工修正后才提交新路径", async () => {
    const { adapter, store, dialog, preview } = await render();
    await preview(header + valid);
    const changed = await adapter.load(store.projectId);
    changed.spaces.find((space) => space.id === "area")!.name = "新区";
    vi.spyOn(adapter, "load").mockResolvedValue(changed);
    const save = vi.spyOn(adapter, "importLocal").mockResolvedValue();
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    expect(save).not.toHaveBeenCalled(); expect(dialog().text()).toContain("项目空间目录已变化");
    expect(dialog().get('[data-import-line="2"]').text()).toContain("未找到空间");
    const selector = dialog().findAllComponents(ProjectSpaceSelect).find((select) => select.attributes("aria-label") === "修正第 2 行空间")!;
    selector.vm.$emit("update:modelValue", "room"); await flushPromises();
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    expect(save).toHaveBeenCalledOnce();
    expect(save.mock.calls[0]![1][0]).toMatchObject({ spaceId: "room", spacePath: "A座/2F/新区/会议室" });
  });

  it("读取文件期间切换项目并重新打开，旧文件迟到返回不污染新清单", async () => {
    const { store, show, dialog } = await render();
    let finish!: (text: string) => void;
    const file = new File(["data"], "screens.csv");
    vi.spyOn(file, "text").mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const input = dialog().get<HTMLInputElement>('[data-testid="screen-import-file"]');
    Object.defineProperty(input.element, "files", { configurable: true, value: [file] });
    await input.trigger("change");
    await store.bindProject("another-import-project"); await flushPromises();
    expect(show.value).toBe(false);
    show.value = true; await flushPromises();
    finish(header + valid); await flushPromises();
    expect(dialog().get<HTMLTextAreaElement>("textarea").element.value).toBe("");
  });

  it.each(["success", "failure"] as const)("提交防重，切换项目后迟到的 %s 不覆盖新抽屉", async (outcome) => {
    const { adapter, store, show, dialog, preview } = await render();
    let finish!: () => void;
    let reject!: (reason: Error) => void;
    const save = vi.spyOn(adapter, "importLocal").mockReturnValue(new Promise((resolve, failure) => { finish = resolve; reject = failure; }));
    await preview(header + valid);
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click");
    expect(save).toHaveBeenCalledOnce();
    await store.bindProject("another-import-project"); await flushPromises();
    show.value = true; await flushPromises();
    if (outcome === "success") finish(); else reject(new Error("旧项目导入失败")); await flushPromises();
    expect(dialog().text()).not.toContain("旧项目导入失败");
    expect(dialog().get<HTMLTextAreaElement>("textarea").element.value).toBe("");
    expect(document.body.textContent).not.toContain("已导入 1 台智能屏");
  });

  it("保存前重查目录尚未返回时关闭并重新打开，不再提交旧清单", async () => {
    const { adapter, store, show, dialog, preview } = await render();
    await preview(header + valid);
    const snapshot = await adapter.load(store.projectId);
    let finish!: (value: typeof snapshot) => void;
    vi.spyOn(adapter, "load").mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    const save = vi.spyOn(adapter, "importLocal");
    await dialog().get('[data-testid="screen-import-submit"]').trigger("click"); await flushPromises();
    show.value = false; await flushPromises(); show.value = true; await flushPromises();
    finish(snapshot); await flushPromises();
    expect(save).not.toHaveBeenCalled();
    expect(dialog().get<HTMLTextAreaElement>("textarea").element.value).toBe("");
  });

  it("只有点击导出才下载本项目完整路径清单", async () => {
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:space-paths");
    vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => undefined);
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => undefined);
    const { dialog } = await render();
    expect(create).not.toHaveBeenCalled();
    await dialog().get('[data-testid="screen-import-space-export"]').trigger("click");
    expect(click).toHaveBeenCalledOnce();
    const blob = create.mock.calls[0]![0] as Blob;
    expect(await blob.text()).toContain('"A座/2F/东区/会议室"');
  });
});
