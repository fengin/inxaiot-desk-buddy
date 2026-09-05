import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import {
  FixtureActivityAdapter,
  publishFixtureTaskEvent
} from "@/dev-fixtures/activityFixtureAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import type { ActivityTask, TaskEventPayload } from "@/shared/model/activity";
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

class DeferredProjectActivityAdapter extends FixtureActivityAdapter {
  private readonly pending = new Map<string, (tasks: ActivityTask[]) => void>();

  override listTasks(projectId: string): Promise<ActivityTask[]> {
    return new Promise((resolve) => this.pending.set(projectId, resolve));
  }

  resolve(projectId: string, tasks: ActivityTask[]) {
    this.pending.get(projectId)?.(tasks);
    this.pending.delete(projectId);
  }
}

function projectTask(projectId: string): ActivityTask {
  return {
    id: `task-${projectId}`,
    projectId,
    domainType: "aio",
    operationType: "deployment_preflight",
    name: `${projectId}部署检查`,
    state: "checking",
    stage: "正在检查",
    progress: 50,
    targetCount: 2,
    completedCount: 1,
    updatedAt: "2026-09-05 10:00:00",
    cancellable: false
  };
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
    const event: TaskEventPayload = {
      eventId: "event-live",
      localTaskId: "task-live",
      sequence: 7,
      localProjectId: "project-shenzhen-bay",
      domainType: "aio",
      stage: "preflight_target",
      status: "checking",
      progressCurrent: 1,
      progressTotal: 2,
      level: "info",
      messageCode: "PREFLIGHT_PROGRESS",
      messageParams: {},
      message: "正在检查一体机",
      timestamp: "2026-09-05T10:00:00Z"
    };
    adapter.emit(event);
    expect(activity.lastEvent).toEqual(event);
    expect(activity.tasks[0]).toMatchObject({
      domainType: "aio",
      operationType: "full_upgrade"
    });
    activity.tasks[0]!.state = "checking";
    expect(activity.activeTaskCount).toBe(1);
    activity.dispose();
  });

  it("keeps preflight work-item progress separate from the real node count", async () => {
    const adapter = new FixtureActivityAdapter();
    const unlisten = await adapter.listen(() => undefined);
    const baseEvent: TaskEventPayload = {
      eventId: "preflight-work-1",
      localTaskId: "preflight-work",
      sequence: 1,
      localProjectId: "project-work",
      domainType: "aio",
      resourceType: null,
      resourceKey: null,
      stage: "检查镜像",
      status: "checking",
      progressCurrent: 2,
      progressTotal: 7,
      level: "info",
      messageCode: "PREFLIGHT_IMAGES_FINISHED",
      messageParams: {
        taskName: "整包升级检查",
        deploymentMode: "full_upgrade",
        targetCount: "2"
      },
      message: "镜像文件检查通过",
      timestamp: "2026-09-05T10:00:00Z"
    };
    publishFixtureTaskEvent(baseEvent);
    let task = (await adapter.listTasks("project-work", 100))[0]!;
    expect(task).toMatchObject({ progress: 29, targetCount: 2, completedCount: 0 });

    publishFixtureTaskEvent({
      ...baseEvent,
      eventId: "preflight-work-2",
      sequence: 2,
      resourceType: "aio",
      resourceKey: "001122AABBCC",
      stage: "渲染模板",
      progressCurrent: 5,
      messageCode: "PREFLIGHT_TARGET_RENDER_FINISHED",
      message: "一体机 1/2 发布模板渲染通过"
    });
    task = (await adapter.listTasks("project-work", 100))[0]!;
    expect(task).toMatchObject({ progress: 71, targetCount: 2, completedCount: 1 });

    publishFixtureTaskEvent({
      ...baseEvent,
      eventId: "preflight-work-3",
      sequence: 3,
      stage: "检查完成",
      status: "succeeded",
      progressCurrent: 7,
      messageCode: "PREFLIGHT_SUCCEEDED",
      message: "部署执行条件检查完成"
    });
    task = (await adapter.listTasks("project-work", 100))[0]!;
    expect(task).toMatchObject({ progress: 100, targetCount: 2, completedCount: 2 });
    unlisten();
  });

  it("clears terminal task records and the selected terminal task log through the adapter", async () => {
    const adapter = new FixtureActivityAdapter();
    configureActivityAdapter(adapter);
    const activity = useActivityStore();
    await activity.start("project-shenzhen-bay");
    const taskId = activity.tasks[0]!.id;
    await activity.selectTask(taskId);
    expect(activity.canClearFinishedTasks).toBe(true);
    expect(activity.canClearSelectedTaskLogs).toBe(true);
    expect(activity.logs).toHaveLength(3);

    await activity.clearSelectedTaskLogs();
    expect(activity.logs).toEqual([]);
    await activity.refreshLogs();
    expect(activity.logs).toEqual([]);

    expect(await activity.clearFinishedTasks()).toBe(1);
    expect(activity.tasks).toEqual([]);
    expect(activity.canClearFinishedTasks).toBe(false);
    activity.dispose();
  });

  it("does not offer clearing when the only finished record is a successful deployment preflight", async () => {
    configureActivityAdapter(new FixtureActivityAdapter());
    const activity = useActivityStore();
    await activity.start("project-shenzhen-bay");
    activity.tasks = [{
      id: "preflight-awaiting-submit",
      projectId: "project-shenzhen-bay",
      domainType: "aio",
      operationType: "deployment_preflight",
      name: "整包升级检查",
      state: "succeeded",
      stage: "检查完成",
      progress: 100,
      targetCount: 2,
      completedCount: 2,
      updatedAt: "2026-09-05 17:10:00",
      cancellable: false
    }];

    expect(activity.canClearFinishedTasks).toBe(false);
    activity.dispose();
  });

  it("ignores an older project refresh that finishes after the active project changed", async () => {
    const adapter = new DeferredProjectActivityAdapter();
    configureActivityAdapter(adapter);
    const activity = useActivityStore();

    const oldRefresh = activity.refreshTasks("project-old");
    const currentRefresh = activity.refreshTasks("project-current");
    adapter.resolve("project-current", [projectTask("project-current")]);
    await currentRefresh;
    adapter.resolve("project-old", [projectTask("project-old")]);
    await oldRefresh;

    expect(activity.tasks.map((task) => task.projectId)).toEqual(["project-current"]);
    expect(activity.selectedTaskId).toBe("task-project-current");
  });
});
