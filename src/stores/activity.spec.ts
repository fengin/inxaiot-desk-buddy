import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

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

class HistoryActivityAdapter extends EventFixtureActivityAdapter {
  readonly calls: { projectId: string; limit: number }[] = [];

  constructor(readonly rows: ActivityTask[]) { super(); }

  override async listTasks(projectId: string, limit: number) {
    this.calls.push({ projectId, limit });
    return this.rows.filter(task => task.projectId === projectId).slice(0, limit);
  }

  override async listLogs(taskId: string) {
    return {
      items: [{ id: taskId, taskId, sequence: 1, timestamp: "2026-10-08T10:00:00Z",
        level: "INFO" as const, source: "测试屏", message: taskId }],
      nextOffset: 1,
      hasMore: false
    };
  }
}

class DeferredHistoryActivityAdapter extends HistoryActivityAdapter {
  private finishHistory?: (rows: ActivityTask[]) => void;

  override listTasks(projectId: string, limit: number): Promise<ActivityTask[]> {
    if (limit !== 500) return super.listTasks(projectId, limit);
    this.calls.push({ projectId, limit });
    return new Promise(resolve => { this.finishHistory = resolve; });
  }

  resolveHistory(rows = this.rows) { this.finishHistory?.(rows); }
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
  beforeEach(() => { setActivePinia(createPinia()); configureActivityAdapter(new FixtureActivityAdapter()); });

  it("新操作指定任务后，日志不再停留在旧诊断；普通刷新仍保留手动选择", async()=>{
    class CurrentTaskAdapter extends FixtureActivityAdapter{
      override async listTasks(){return [{...projectTask('project-current'),id:'install-current',name:'安装/升级小新',domainType:'smart_screen',operationType:'install',state:'running' as const}, {...projectTask('project-current'),id:'diagnostics-old',name:'采集诊断',state:'succeeded' as const}];}
      override async listLogs(taskId:string){return {items:[{id:taskId,taskId,sequence:1,timestamp:'2026-09-29T09:45:00Z',level:'INFO' as const,source:'办公室10寸屏',message:taskId==='install-current'?'正在传输安装包':'开始设备检查'}],nextOffset:1,hasMore:false};}
    }
    configureActivityAdapter(new CurrentTaskAdapter());const activity=useActivityStore();
    await activity.refreshTasks('project-current');await activity.selectTask('diagnostics-old');
    await activity.refreshTasks('project-current','install-current');expect(activity.selectedTask?.id).toBe('install-current');expect(activity.logs[0]?.message).toBe('正在传输安装包');
    await activity.selectTask('diagnostics-old');await activity.refreshTasks('project-current');expect(activity.selectedTask?.id).toBe('diagnostics-old');
    activity.dispose();
  });

  it("进度事件的并发刷新不会丢失刚提交任务的日志选择",async()=>{
    class PendingAdapter extends FixtureActivityAdapter{
      waits:Array<(items:ActivityTask[])=>void>=[];
      override listTasks():Promise<ActivityTask[]>{return new Promise(resolve=>this.waits.push(resolve));}
    }
    const adapter=new PendingAdapter();configureActivityAdapter(adapter);const activity=useActivityStore();
    const rows=[{...projectTask('p'),id:'new-install'},{...projectTask('p'),id:'old-diagnostic'}];
    const initial=activity.refreshTasks('p');adapter.waits[0]!(rows);await initial;await activity.selectTask('old-diagnostic');
    const preferred=activity.refreshTasks('p','new-install');const eventRefresh=activity.refreshTasks('p');
    adapter.waits[2]!(rows);await eventRefresh;adapter.waits[1]!(rows);await preferred;
    expect(activity.selectedTaskId).toBe('new-install');activity.dispose();
  });

  it("从历史打开第 101 条以后的任务，事件刷新仍显示该任务的日志；查看最近任务只读取 100 条", async () => {
    vi.useFakeTimers();
    const rows = Array.from({ length: 150 }, (_, index) => ({
      ...projectTask("p"), id: `history-${index}`
    }));
    const adapter = new HistoryActivityAdapter(rows);
    configureActivityAdapter(adapter);
    const activity = useActivityStore();
    try {
      await activity.start("p");
      expect(adapter.calls.map(call => call.limit)).toEqual([100]);
      await activity.refreshTasks("p", "history-149");
      expect(activity.selectedTask?.id).toBe("history-149");
      expect(activity.logs[0]?.taskId).toBe("history-149");

      adapter.emit({
        eventId: "new-progress", localTaskId: "history-0", sequence: 2,
        localProjectId: "p", domainType: "smart_screen", stage: "检查设备",
        status: "running", progressCurrent: 1, progressTotal: 2,
        level: "info", messageCode: "SCREEN_PROGRESS", messageParams: {},
        message: "检查中", timestamp: "2026-10-08T10:00:00Z"
      });
      await vi.advanceTimersByTimeAsync(120);
      expect(activity.selectedTaskId).toBe("history-149");
      expect(activity.selectedTask?.id).toBe("history-149");
      expect(activity.logs[0]?.taskId).toBe("history-149");
      expect(adapter.calls.map(call => call.limit)).toEqual([100, 100, 500, 100, 500]);

      await activity.selectTask("history-0");
      await activity.refreshTasks("p");
      expect(adapter.calls.map(call => call.limit)).toEqual([100, 100, 500, 100, 500, 100]);
    } finally {
      activity.dispose();
      vi.useRealTimers();
    }
  });

  it("旧任务的扩展查询晚于项目切换返回时，不覆盖新项目及日志", async () => {
    const oldRows = Array.from({ length: 150 }, (_, index) => ({
      ...projectTask("old"), id: `old-${index}`
    }));
    const current = projectTask("current");
    const adapter = new DeferredHistoryActivityAdapter([...oldRows, current]);
    configureActivityAdapter(adapter);
    const activity = useActivityStore();
    const oldRefresh = activity.refreshTasks("old", "old-149");
    await vi.waitFor(() => expect(adapter.calls).toContainEqual({ projectId: "old", limit: 500 }));
    await activity.refreshTasks("current");
    adapter.resolveHistory(oldRows);
    await oldRefresh;
    expect(activity.tasks.map(task => task.id)).toEqual([current.id]);
    expect(activity.selectedTask?.id).toBe(current.id);
    expect(activity.logs[0]?.taskId).toBe(current.id);
    activity.dispose();
  });

  it("等待旧任务扩展查询时用户另选任务，慢查询不会重新选中旧任务", async () => {
    const rows = Array.from({ length: 150 }, (_, index) => ({
      ...projectTask("p"), id: `history-${index}`
    }));
    const adapter = new DeferredHistoryActivityAdapter(rows);
    configureActivityAdapter(adapter);
    const activity = useActivityStore();
    await activity.refreshTasks("p");
    const oldRefresh = activity.refreshTasks("p", "history-149");
    await vi.waitFor(() => expect(adapter.calls).toContainEqual({ projectId: "p", limit: 500 }));
    await activity.selectTask("history-1");
    adapter.resolveHistory();
    await oldRefresh;
    expect(activity.selectedTaskId).toBe("history-1");
    expect(activity.selectedTask?.id).toBe("history-1");
    expect(activity.logs[0]?.taskId).toBe("history-1");
    activity.dispose();
  });

  it("同项目新一次定位已完成后，旧任务扩展查询不再替换任务列表", async () => {
    const rows = Array.from({ length: 150 }, (_, index) => ({
      ...projectTask("p"), id: `history-${index}`
    }));
    const adapter = new DeferredHistoryActivityAdapter(rows);
    configureActivityAdapter(adapter);
    const activity = useActivityStore();
    const oldRefresh = activity.refreshTasks("p", "history-149");
    await vi.waitFor(() => expect(adapter.calls).toContainEqual({ projectId: "p", limit: 500 }));
    await activity.refreshTasks("p", "history-0");
    adapter.resolveHistory();
    await oldRefresh;
    expect(activity.tasks).toHaveLength(100);
    expect(activity.selectedTask?.id).toBe("history-0");
    expect(activity.logs[0]?.taskId).toBe("history-0");
    activity.dispose();
  });

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
      clearable: false,
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
