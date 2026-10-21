import { createPinia } from "pinia";
import { defineComponent, h, type VNode } from "vue";
import { DOMWrapper, flushPromises, mount } from "@vue/test-utils";
import { NDialogProvider, NMessageProvider } from "naive-ui";
import { describe, expect, it, vi } from "vitest";
import { createScreenSnapshot } from "@/dev-fixtures/screenData";
import { FixtureScreenAdapter } from "@/dev-fixtures/screenFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { configureScreenAdapter } from "@/shared/api/screenAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import type { ScreenInspection, SmartScreen } from "@/shared/model/screen";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ScreenOperations from "./ScreenOperations.vue";
import ScreenTimeCheck from "./ScreenTimeCheck.vue";
import ScreenDetail from "./ScreenDetail.vue";

// 独立于被测 formatter，取测试进程当地时区。可由父进程用不同 TZ 运行同一页面套件。
function expectedLocalTime(value: string) {
  const parts = new Intl.DateTimeFormat("en-GB", {
    calendar: "gregory", numberingSystem: "latn", hourCycle: "h23",
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit"
  }).formatToParts(new Date(value));
  const part = (name: Intl.DateTimeFormatPartTypes) => parts.find(item => item.type === name)!.value;
  return `${part("year")}-${part("month")}-${part("day")} ${part("hour")}:${part("minute")}:${part("second")}`;
}

function context() {
  const pinia = createPinia(), adapter = new FixtureScreenAdapter(localStorage, 20);
  configureScreenAdapter(adapter);
  configureActivityAdapter(new FixtureActivityAdapter(adapter));
  const store = useSmartScreensStore(pinia), snapshot = createScreenSnapshot();
  snapshot.screens = [snapshot.screens[0]!]; snapshot.tasks = []; snapshot.platformDrafts = {};
  store.projectId = "date-time-isolated-project"; store.snapshot = snapshot;
  const preflight = vi.spyOn(adapter, "preflight"), execute = vi.spyOn(adapter, "execute");
  function render(content: () => VNode) {
    return mount(defineComponent({ setup: () => () => h(NMessageProvider, null, {
      default: () => h(NDialogProvider, null, { default: content })
    }) }), { attachTo: document.body, global: { plugins: [pinia] } });
  }
  return { store, adapter, preflight, execute, render, dispose() { store.stop(); adapter.dispose(); } };
}

function fact(root: ParentNode, label: string) {
  const term = [...root.querySelectorAll("dt")].find(element => element.textContent?.trim() === label);
  expect(term, `应显示${label}`).toBeDefined();
  return term!.nextElementSibling as HTMLElement;
}

describe("智能屏页面当地时间显示", () => {
  it("校准时间设置显示冻结的电脑当地时间，查看页面不发起设备操作", async () => {
    const instant = "2026-10-21T06:20:10Z";
    vi.useFakeTimers({ toFake: ["Date"] }); vi.setSystemTime(new Date(instant));
    const view = context(); view.store.operation = "time";
    const wrapper = view.render(() => h(ScreenOperations));
    try {
      await flushPromises();
      const information = wrapper.get(".screen-setting-info");
      expect(information.get("strong").text()).toBe("当前电脑时间");
      expect(information.get("p").text()).toBe(expectedLocalTime(instant));
      expect(information.text()).not.toContain(instant);
      expect(view.store.selectedIds).toEqual([]);
      expect(view.preflight).not.toHaveBeenCalled(); expect(view.execute).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); view.dispose(); vi.useRealTimers(); }
  });

  it("点击真实校时Popover后，悬停提示及正文均转当地时间，设备时区和偏差保持原值", async () => {
    const observation: ScreenInspection = {
      id: "date-time-observation", observedIp: "192.0.2.31", observedAt: "2026-10-21T06:21:08Z",
      adbAvailable: true, abis: [], errors: [], deviceTime: "2026-10-21T06:21:06Z",
      computerTime: "2026-10-21T06:21:07Z", timezone: "Asia/Shanghai", clockOffsetSeconds: -1,
      automaticTime: true, automaticTimezone: false
    };
    const before = JSON.stringify(observation), view = context();
    const wrapper = view.render(() => h(ScreenTimeCheck, { observation }));
    try {
      const trigger = wrapper.get('[data-action-owner="time-check-popover"]');
      expect(trigger.text()).toBe("-1 秒 · 详情");
      expect(trigger.attributes("title")).toBe(`设备时间：${expectedLocalTime(observation.deviceTime!)}\n读取时电脑时间：${expectedLocalTime(observation.computerTime!)}\n设备时区：Asia/Shanghai\n自动校时 / 时区：开启 / 关闭`);
      expect(document.body.querySelector(".screen-time-check")).toBeNull();
      await trigger.trigger("click"); await flushPromises();
      await vi.waitFor(() => expect(document.body.querySelector(".screen-time-check")).not.toBeNull());
      const popup = document.body.querySelector(".screen-time-check")!;
      expect(fact(popup, "设备时间").textContent).toBe(expectedLocalTime(observation.deviceTime!));
      expect(fact(popup, "读取时电脑时间").textContent).toBe(expectedLocalTime(observation.computerTime!));
      expect(fact(popup, "设备时区").textContent).toBe("Asia/Shanghai");
      expect(fact(popup, "时间偏差").textContent).toBe("-1 秒");
      expect(fact(popup, "自动校时 / 时区").textContent).toBe("开启 / 关闭");
      expect(JSON.stringify(observation)).toBe(before);
      expect(view.preflight).not.toHaveBeenCalled(); expect(view.execute).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); view.dispose(); }
  });

  it("屏详情的MAC、检查、ADB、版本、缓存、设备电脑及历史时间统一显示，输入记录不被改写", async () => {
    const view = context();
    const screen: SmartScreen = {
      ...view.store.snapshot.screens[0]!, macCheckedAt: "2026-10-21T06:22:24Z", checkedAt: "2026-10-21T06:21:08Z",
      adbCheckedAt: "2026-10-21T06:21:09Z", versionCheckedAt: "2026-10-21T06:21:10Z",
      deviceTime: "2026-10-21T06:21:06Z", computerTime: "2026-10-21T06:21:07Z", timezone: "Asia/Shanghai", clockOffsetSeconds: -1
    };
    const cacheTime = "2026-10-21T06:22:30Z", taskTime = "2026-10-21T06:19:15Z";
    view.store.snapshot.screens = [screen]; view.store.snapshot.platformAvailable = false; view.store.snapshot.platformReadAt = cacheTime;
    view.store.snapshot.tasks = [{ id: "date-time-history", projectId: view.store.projectId, mode: "real", action: "inspect", state: "succeeded",
      createdAt: taskTime, updatedAt: taskTime, targets: [{ screenId: screen.id, name: screen.name, ip: screen.ip, state: "succeeded", progress: 100, message: "隔离历史记录" }], logs: [] }];
    const before = JSON.stringify({ screen, snapshot: view.store.snapshot });
    const wrapper = view.render(() => h(ScreenDetail, { screen }));
    try {
      await flushPromises();
      await vi.waitFor(() => expect(document.body.querySelector(".screen-detail-drawer")).not.toBeNull());
      const drawer = document.body.querySelector(".screen-detail-drawer")!;
      expect(fact(drawer, "MAC 采集时间").textContent).toBe(expectedLocalTime(screen.macCheckedAt!));
      const checked = fact(drawer, "最近检查");
      expect(checked.textContent).toBe(expectedLocalTime(screen.checkedAt!));
      expect(checked.title).toBe(expectedLocalTime(screen.checkedAt!));
      const more = drawer.querySelector<HTMLDetailsElement>(".screen-inspection-more")!;
      await new DOMWrapper(more.querySelector("summary")!).trigger("click");
      expect(more.open).toBe(true);
      expect(fact(more, "ADB 检查时间").textContent).toBe(expectedLocalTime(screen.adbCheckedAt!));
      expect(fact(more, "版本读取时间").textContent).toBe(expectedLocalTime(screen.versionCheckedAt!));
      expect(fact(more, "设备时间").textContent).toBe(expectedLocalTime(screen.deviceTime!));
      expect(fact(more, "读取时电脑时间").textContent).toBe(expectedLocalTime(screen.computerTime!));
      expect(fact(more, "设备时区").textContent).toBe("Asia/Shanghai");
      expect(fact(more, "平台读取时间").textContent).toBe(expectedLocalTime(cacheTime));
      expect(drawer.textContent).toContain(`缓存读取时间：${expectedLocalTime(cacheTime)}`);
      expect(drawer.querySelector(".screen-history-item small")?.textContent).toBe(expectedLocalTime(taskTime));
      expect(drawer.textContent).not.toContain("2026-10-21T06:");
      expect(JSON.stringify({ screen, snapshot: view.store.snapshot })).toBe(before);
      expect(view.preflight).not.toHaveBeenCalled(); expect(view.execute).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); view.dispose(); }
  });
});
