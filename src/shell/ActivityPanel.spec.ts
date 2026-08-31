import { createPinia, setActivePinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import ActivityPanel from "@/shell/ActivityPanel.vue";
import { useActivityStore } from "@/stores/activity";

describe("activity panel", () => {
  it("renders the shared task DTO and switches to filtered local logs", async () => {
    const pinia = createPinia();
    setActivePinia(pinia);
    const activity = useActivityStore();
    activity.openPanel("tasks");
    const wrapper = mount(ActivityPanel, { global: { plugins: [pinia] } });
    await flushPromises();
    expect(wrapper.get("[data-testid='activity-panel']").text()).toContain("A栋一体机整包升级");
    await wrapper.get(".task-row").trigger("click");
    await flushPromises();
    expect(activity.panelTab).toBe("logs");
    expect(wrapper.findAll(".log-line")).toHaveLength(3);

    activity.logLevels = ["WARN"];
    await activity.refreshLogs();
    await flushPromises();
    expect(wrapper.findAll(".log-line")).toHaveLength(0);
    wrapper.unmount();
  });
});
