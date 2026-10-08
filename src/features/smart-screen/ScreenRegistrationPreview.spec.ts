import { defineComponent, h, ref } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { NCheckbox } from "naive-ui";
import ScreenRegistrationPreview from "./ScreenRegistrationPreview.vue";
import type { ScreenRegistrationMacConfirmation, ScreenRegistrationPreview as Preview } from "@/shared/model/screenRegistration";

function preview(): Preview {
  const fields = { name: "会议室屏", ip: "192.0.2.1", mac: "A0:B1:C2:D3:E4:F5", size: "10" as const, spaceId: "floor-b", location: "门口" };
  return { id: "preview-1", projectId: "project-1", createdAt: "2026-09-28T01:00:00Z", items: [
    { screenId: "screen-1", mode: "update", state: "ready", reason: "资料检查通过", before: { ...fields, spaceId: "floor-a", location: "走廊" }, after: fields,
      diffs: [{ field: "location", label: "详细位置", before: "走廊", after: "门口" }, { field: "spaceId", label: "所在空间", before: "A座/1F", after: "A座/2F" }], expectedRevision: 1,
      needsSpaceConfirmation: true, macSource: "history", macMessage: "采集失败，需要确认历史 MAC", requiredMacConfirmation: "existing", duplicateIds: [] },
    { screenId: "screen-2", mode: "create", state: "blocked", reason: "IP 存在重复候选", before: null, after: { ...fields, name: "待核对屏", ip: "192.0.2.2" }, diffs: [], expectedRevision: 1,
      needsSpaceConfirmation: false, macSource: "conflict", macMessage: "MAC 不一致", requiredMacConfirmation: null, duplicateIds: ["screen-3"] },
    { screenId: "screen-4", mode: "update", state: "skip", reason: "平台资料无变更", before: fields, after: fields, diffs: [], expectedRevision: 1,
      needsSpaceConfirmation: false, macSource: "unchanged", macMessage: "身份未变化", requiredMacConfirmation: null, duplicateIds: [] }
  ] };
}

function render(data = preview()) {
  const included = ref(["screen-1"]), macConfirmations = ref<Record<string, ScreenRegistrationMacConfirmation>>({}), spaceConfirmations = ref<string[]>([]);
  const wrapper = mount(defineComponent({ setup: () => () => h(ScreenRegistrationPreview, {
    preview: data, spaces: [], included: included.value, macConfirmations: macConfirmations.value, spaceConfirmations: spaceConfirmations.value,
    "onUpdate:included": (value: string[]) => { included.value = value; },
    "onUpdate:macConfirmations": (value: Record<string, ScreenRegistrationMacConfirmation>) => { macConfirmations.value = value; },
    "onUpdate:spaceConfirmations": (value: string[]) => { spaceConfirmations.value = value; }
  }) }));
  return { wrapper, included, macConfirmations, spaceConfirmations };
}

describe("平台注册逐屏预览", () => {
  it("区分新增、更新、无需更新与需核对，展示差异及逐屏确认", async () => {
    const { wrapper, macConfirmations, spaceConfirmations } = render();
    try {
      expect(wrapper.text()).toContain("新增注册 0");
      expect(wrapper.text()).toContain("更新资料 1");
      expect(wrapper.text()).toContain("无需更新 1");
      expect(wrapper.text()).toContain("需要核对 1");
      expect(wrapper.get(".screen-registration-comparison").text()).toContain("平台当前");
      expect(wrapper.get(".screen-registration-comparison").text()).toContain("A座/1F");
      expect(wrapper.get(".screen-registration-comparison").text()).toContain("A座/2F");
      expect(wrapper.get(".screen-registration-mac").text()).toContain("历史记录");
      expect(macConfirmations.value).toEqual({}); expect(spaceConfirmations.value).toEqual([]);
      const confirms = wrapper.findAllComponents(NCheckbox).filter((checkbox) => checkbox.classes().includes("screen-registration-confirmation"));
      confirms[0]!.vm.$emit("update:checked", true); confirms[1]!.vm.$emit("update:checked", true); await flushPromises();
      expect(macConfirmations.value).toEqual({ "screen-1": "existing" }); expect(spaceConfirmations.value).toEqual(["screen-1"]);
      expect(wrapper.get('[data-testid="screen-registration-prototype"]').text()).toContain("不连接设备或写入真实平台");
    } finally { wrapper.unmount(); }
  });

  it("可取消单屏，阻断和无需更新目标不能勾选；重复项引导现有合并", async () => {
    const { wrapper, included } = render();
    try {
      wrapper.get('.screen-registration-target[data-screen-id="screen-1"]').getComponent(NCheckbox).vm.$emit("update:checked", false);
      await flushPromises(); expect(included.value).toEqual([]);
      expect(wrapper.text()).toContain("已取消本屏，本次不会提交");
      const blocked = wrapper.get('.screen-registration-target[data-screen-id="screen-2"]');
      expect(blocked.getComponent(NCheckbox).props("disabled")).toBe(true);
      expect(wrapper.get('.screen-registration-target[data-screen-id="screen-4"]').getComponent(NCheckbox).props("disabled")).toBe(true);
      await blocked.get("button").trigger("click");
      expect(wrapper.get(".screen-registration-detail").text()).toContain("身份冲突");
      await wrapper.findAll("button").find((button) => button.text() === "核对疑似重复")!.trigger("click");
      expect(wrapper.getComponent(ScreenRegistrationPreview).emitted("merge")).toHaveLength(1);
    } finally { wrapper.unmount(); }
  });

  it("无有效历史 MAC 的暂空确认独立保存，修改资料发出所选屏 ID", async () => {
    const data = preview();
    Object.assign(data.items[0]!, { macSource: "empty", macMessage: "采集失败且没有历史值", requiredMacConfirmation: "empty" });
    data.items[0]!.after.mac = "";
    const { wrapper, macConfirmations } = render(data);
    try {
      const confirmation = wrapper.findAllComponents(NCheckbox).find((checkbox) => checkbox.attributes("aria-label")?.includes("MAC 暂空"))!;
      confirmation.vm.$emit("update:checked", true); await flushPromises();
      expect(macConfirmations.value).toEqual({ "screen-1": "empty" });
      await wrapper.findAll("button").find((button) => button.text() === "编辑资料")!.trigger("click");
      expect(wrapper.getComponent(ScreenRegistrationPreview).emitted("edit")).toEqual([["screen-1"]]);
    } finally { wrapper.unmount(); }
  });
});
