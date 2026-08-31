import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { useActivityStore } from "@/stores/activity";

describe("activity store", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("keeps browser fixtures behind the activity adapter contract", async () => {
    const activity = useActivityStore();
    await activity.start("project-shenzhen-bay");
    expect(activity.tasks).toHaveLength(1);
    expect(activity.activeTaskCount).toBe(0);

    const taskId = activity.tasks[0]!.id;
    await activity.selectTask(taskId);
    expect(activity.logs.some((entry) => entry.taskId === taskId)).toBe(true);

    activity.logLevels = ["WARN"];
    await activity.refreshLogs();
    expect(activity.logs).toHaveLength(0);
    activity.dispose();
  });
});
