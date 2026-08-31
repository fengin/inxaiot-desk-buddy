import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import type { TaskEventPayload } from "@/shared/model/activity";
import { useActivityStore } from "@/stores/activity";

class EventFixtureActivityAdapter extends FixtureActivityAdapter {
  private handler?: (event: TaskEventPayload) => void;

  override async listen(handler: (event: TaskEventPayload) => void) {
    this.handler = handler;
    return () => { this.handler = undefined; };
  }

  emit(event: TaskEventPayload) {
    this.handler?.(event);
  }
}

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

  it("exposes the latest task event for event-driven feature refresh", async () => {
    const adapter = new EventFixtureActivityAdapter();
    configureActivityAdapter(adapter);
    const activity = useActivityStore();
    await activity.start("project-shenzhen-bay");
    adapter.emit({ localTaskId: "task-live", sequence: 7 });
    expect(activity.lastEvent).toEqual({ localTaskId: "task-live", sequence: 7 });
    expect(activity.tasks[0]).toMatchObject({
      domainType: "aio",
      operationType: "full_upgrade"
    });
    activity.dispose();
  });
});
