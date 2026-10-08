import { defineComponent, h, ref } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { NCheckbox } from "naive-ui";
import type { ScreenPreflightItem } from "@/shared/model/screen";
import ScreenPreflightTable from "./ScreenPreflightTable.vue";

const rows: ScreenPreflightItem[] = [
  { screenId: "ready", name: "可校时屏", ip: "192.0.2.1", state: "ready", reason: "校时检查通过", observation: {
    id: "observation", observedIp: "192.0.2.1", observedAt: "2026-10-08T00:00:00Z", adbAvailable: true, abis: [], errors: [],
    deviceTime: "2026-10-08T00:00:30Z", computerTime: "2026-10-08T00:00:00Z", timezone: "Asia/Shanghai",
    automaticTime: false, automaticTimezone: true, clockOffsetSeconds: 30
  } },
  { screenId: "blocked", name: "需处理屏", ip: "192.0.2.2", state: "blocked", reason: "没有写入权限，请处理后重新检查" },
  { screenId: "skip", name: "无需执行屏", ip: "192.0.2.3", state: "skip", reason: "无需校准" }
];

describe("智能屏执行检查列表", () => {
  it("改为列表后只允许检查通过目标参与提交，提交期间不能修改选择", async () => {
    const included = ref(["ready"]), disabled = ref(false);
    const wrapper = mount(defineComponent({ setup: () => () => h(ScreenPreflightTable, {
      rows, included: included.value, disabled: disabled.value,
      "onUpdate:included": (value: string[]) => { included.value = value; }
    }) }));
    try {
      const checkbox = (id: string) => wrapper.get(`[data-screen-id="${id}"]`).getComponent(NCheckbox);
      expect(checkbox("blocked").props("disabled")).toBe(true);
      expect(checkbox("skip").props("disabled")).toBe(true);
      checkbox("blocked").vm.$emit("update:checked", true);
      checkbox("skip").vm.$emit("update:checked", true);
      await flushPromises(); expect(included.value).toEqual(["ready"]);
      checkbox("ready").vm.$emit("update:checked", false);
      await flushPromises(); expect(included.value).toEqual([]);
      checkbox("ready").vm.$emit("update:checked", true);
      await flushPromises(); expect(included.value).toEqual(["ready"]);
      disabled.value = true; await flushPromises();
      checkbox("ready").vm.$emit("update:checked", false);
      await flushPromises(); expect(included.value).toEqual(["ready"]);
    } finally { wrapper.unmount(); }
  });

  it("校时行显示偏差，点击仍能查看原设备时间、电脑时间、时区和开关", async () => {
    const wrapper = mount(ScreenPreflightTable, { props: { rows, included: ["ready"], time: true }, global: { stubs: { teleport: true } } });
    try {
      const detail = wrapper.get('[aria-label="查看校时检查详情"]');
      expect(detail.text()).toContain("30 秒");
      expect(detail.attributes("title")).toContain("Asia/Shanghai");
      expect(detail.attributes("title")).toContain("关闭 / 开启");
      await detail.trigger("click"); await flushPromises();
      expect(wrapper.get(".screen-time-check").text()).toContain("设备时间");
      expect(wrapper.get(".screen-time-check").text()).toContain("读取时电脑时间");
      expect(wrapper.get(".screen-time-check").text()).toContain("Asia/Shanghai");
      expect(wrapper.get(".screen-time-check").text()).toContain("关闭 / 开启");
      expect(wrapper.get('[data-screen-id="blocked"] td:last-child').attributes("title")).toBe(rows[1]!.reason);
    } finally { wrapper.unmount(); }
  });
});
