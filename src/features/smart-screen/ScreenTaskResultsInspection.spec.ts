import { createPinia } from "pinia";
import { h } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NMessageProvider } from "naive-ui";
import { describe, expect, it, vi } from "vitest";
import type { ScreenInspection, ScreenTask } from "@/shared/model/screen";
import * as screenApi from "@/shared/api/screenAdapter";
import ScreenTaskResults from "./ScreenTaskResults.vue";

function inspection(overrides: Partial<ScreenInspection> = {}): ScreenInspection {
  return {
    id: "current-observation", operationType: "inspect", observedIp: "192.0.2.74",
    observedAt: "2026-11-04T09:00:00Z", ping: "online", adbAvailable: true, abis: [], errors: [],
    ...overrides
  };
}
function task(action: "inspect" | "ping" = "inspect", observation: ScreenInspection | null = inspection()): ScreenTask {
  return {
    id: "device-check", projectId: "local-project", action, mode: "real", state: "succeeded",
    createdAt: "2026-11-04T09:00:00Z", updatedAt: "2026-11-04T09:00:00Z", logs: [],
    targets: [{ screenId: "screen-74", name: "本机测试屏", ip: "192.0.2.74", state: "succeeded", progress: 100,
      message: "本次检查已完成", result: { device: "succeeded", business: "not_required", shared: "not_required", observation } }]
  };
}
function render(value: ScreenTask) {
  return mount(NMessageProvider, {
    global: { plugins: [createPinia()], stubs: { teleport: true } },
    slots: { default: () => h(ScreenTaskResults, { task: value }) }
  });
}

describe("检查设备的逐项观测与任务结果", () => {
  it("离线与检查完成分别展示，不将成功任务当作网络在线", () => {
    const wrapper = render(task("inspect", inspection({ ping: "offline", adbAvailable: false, errors: ["ADB连接失败"] })));
    try {
      expect(wrapper.get('[data-testid="screen-check-network"]').text()).toBe("离线");
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("不可用");
      expect(wrapper.get('[data-testid="screen-target-execution-result"]').text()).toBe("检查完成");
      expect(wrapper.get(".screen-result-header .n-tag").text()).toBe("检查完成");
      expect(wrapper.get('[data-testid="screen-check-device-info"]').text()).toContain("安装未确认");
      expect(wrapper.get("thead").text()).toContain("网络连通性管理连接设备/小新信息检查结果");
    } finally { wrapper.unmount(); }
  });

  it("Ping在线但ADB失败时保留在线事实与任务失败的独立含义", () => {
    const value = task("inspect", inspection({ adbAvailable: false, errors: ["管理连接未授权"] }));
    value.state = "failed"; value.targets[0]!.state = "failed";
    value.targets[0]!.result!.device = "failed"; value.targets[0]!.message = "管理连接未授权";
    const wrapper = render(value);
    try {
      expect(wrapper.get('[data-testid="screen-check-network"]').text()).toBe("在线");
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("不可用");
      expect(wrapper.get('[data-testid="screen-target-execution-result"]').text()).toBe("检查未完成");
      expect(wrapper.get(".screen-result-header .n-tag").text()).toBe("失败");
      expect(wrapper.get(".screen-result-message").text()).toBe("管理连接未授权");
    } finally { wrapper.unmount(); }
  });

  it("快速模式的默认ADB false不表示管理连接失败，也不借用设备信息", () => {
    const wrapper = render(task("ping", inspection({ operationType: "ping", adbAvailable: false,
      android: "99-cached", appInstalled: true, appRunning: true, observedAppVersion: "9.9-cached" })));
    try {
      expect(wrapper.get('[data-testid="screen-check-network"]').text()).toBe("在线");
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("本次未检查");
      const info = wrapper.get('[data-testid="screen-check-device-info"]').text();
      expect(info).toContain("本次未检查"); expect(info).toContain("仅检查网络连通性");
      expect(info).not.toContain("cached");
      expect(wrapper.findAll("button").some(button => button.text() === "核对版本并同步平台")).toBe(false);
    } finally { wrapper.unmount(); }
  });

  it("完整检查关联快速观测时也不误称已检查ADB", () => {
    const wrapper = render(task("inspect", inspection({ operationType: "ping", adbAvailable: false })));
    try {
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("本次未检查");
      expect(wrapper.get('[data-testid="screen-check-device-info"]').text()).toContain("系统：未确认");
    } finally { wrapper.unmount(); }
  });

  it("本次完整读取展示系统和小新安装/运行/版本，旧无类型inspect仍兼容实测字段", () => {
    const wrapper = render(task("inspect", inspection({ operationType: undefined, android: "8.1", appInstalled: true,
      appRunning: false, observedAppVersion: "2.1.3" })));
    try {
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("可用");
      const info = wrapper.get('[data-testid="screen-check-device-info"]');
      expect(info.text()).toContain("系统：Android 8.1");
      expect(info.text()).toContain("小新：已安装 · 未运行 · 版本 2.1.3");
      expect(info.get(".screen-secondary").attributes("title")).toBe("小新：已安装 · 未运行 · 版本 2.1.3");
    } finally { wrapper.unmount(); }
  });

  it("观测缺少明确管理字段和设备字段时显示未检查/未确认", () => {
    const observed = inspection();
    delete (observed as Partial<ScreenInspection>).adbAvailable;
    const wrapper = render(task("inspect", observed));
    try {
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("本次未检查");
      expect(wrapper.get('[data-testid="screen-check-device-info"]').text()).toContain("系统：未确认");
      expect(wrapper.get('[data-testid="screen-check-device-info"]').text()).toContain("运行未确认");
      expect(wrapper.get('[data-testid="screen-check-device-info"]').text()).toContain("版本未确认");
    } finally { wrapper.unmount(); }
  });

  it.each(["inspect", undefined])("%s记录只有默认ADB false而无失败依据，不误判不可用", operationType => {
    const wrapper = render(task("inspect", inspection({ operationType, adbAvailable: false, errors: [] })));
    try {
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("本次未检查");
      expect(wrapper.get('[data-testid="screen-check-network"]').text()).toBe("在线");
    } finally { wrapper.unmount(); }
  });

  it("来源IP不一致时三类观测均不借用，当前资产缓存也不能填补本次结果", () => {
    const load = vi.fn().mockResolvedValue({ screens: [{ id: "screen-74", ping: "online", adbAvailable: true, android: "cached" }] });
    const adapter = vi.spyOn(screenApi, "useScreenAdapter").mockReturnValue({ mode: "real", load } as unknown as screenApi.ScreenAdapter);
    const wrapper = render(task("inspect", inspection({ observedIp: "192.0.2.99", android: "other-device", appInstalled: true,
      appRunning: true, observedAppVersion: "wrong-version" })));
    try {
      expect(wrapper.get('[data-testid="screen-check-network"]').text()).toBe("未知");
      expect(wrapper.get('[data-testid="screen-check-management"]').text()).toBe("本次未检查");
      expect(wrapper.get('[data-testid="screen-check-device-info"]').text()).not.toMatch(/cached|other-device|wrong-version/);
      expect(wrapper.get(".screen-result-message").text()).toContain("检查地址与本次目标不一致");
      expect(load).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); adapter.mockRestore(); }
  });

  it.each(["inspect", "ping"] as const)("%s保留平台差异确认入口，仅emit不自动写平台", async action => {
    const coverStatus = vi.fn(), submitVersionSync = vi.fn(), execute = vi.fn();
    const adapter = vi.spyOn(screenApi, "useScreenAdapter").mockReturnValue({ mode: "real", coverStatus, submitVersionSync, execute } as unknown as screenApi.ScreenAdapter);
    const wrapper = render(task(action));
    try {
      await wrapper.findAll("button").find(button => button.text() === "查看平台状态差异")!.trigger("click");
      expect(wrapper.getComponent(ScreenTaskResults).emitted("status")).toEqual([[]]);
      if (action === "inspect") {
        await wrapper.findAll("button").find(button => button.text() === "核对版本并同步平台")!.trigger("click");
        expect(wrapper.getComponent(ScreenTaskResults).emitted("versions")).toEqual([[["screen-74"]]]);
      }
      await wrapper.findAll("button").find(button => button.text() === "查看屏详情")!.trigger("click");
      expect(wrapper.getComponent(ScreenTaskResults).emitted("detail")).toEqual([["screen-74"]]);
      expect(coverStatus).not.toHaveBeenCalled(); expect(submitVersionSync).not.toHaveBeenCalled(); expect(execute).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); adapter.mockRestore(); }
  });

  it("表格横向滚动同步表头，避免窄窗列错位", async () => {
    const wrapper = render(task());
    try {
      const body = wrapper.get(".screen-result-table-body");
      body.element.scrollLeft = 180;
      await body.trigger("scroll"); await flushPromises();
      expect(wrapper.get(".screen-result-table-header").element.scrollLeft).toBe(180);
      expect(wrapper.find(".screen-task-results-check").exists()).toBe(true);
    } finally { wrapper.unmount(); }
  });
});
