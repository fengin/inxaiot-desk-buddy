import { createPinia } from "pinia";
import { h, ref, unref, type Ref } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { NMessageProvider, NPagination, NTag } from "naive-ui";
import { describe, expect, it, vi } from "vitest";
import type { ScreenTask, ScreenTargetState } from "@/shared/model/screen";
import { screenStateLabel } from "@/shared/model/screen";
import * as screenApi from "@/shared/api/screenAdapter";
import { useActivityStore } from "@/stores/activity";
import ScreenTaskResults from "./ScreenTaskResults.vue";

function task(state: ScreenTask["state"] = "succeeded", targets: ScreenTargetState[] = ["succeeded"]): ScreenTask {
  return {
    id: "apk-preview-task", projectId: "screen-preview-project", action: "install", state,
    createdAt: "2026-09-27T10:00:00Z", updatedAt: "2026-09-27T10:00:00Z", logs: [],
    input: {
      action: "install", applicationId: "xiaoxin", targetIds: targets.map((_, index) => `screen-${index}`),
      appVersion: "", abi: "universal", reinstall: false, concurrency: 1,
      apk: { name: "xiaoxin.apk", size: 1024, lastModified: 0 }
    },
    targets: targets.map((targetState, index) => ({
      screenId: `screen-${index}`, name: `演示屏 ${index}`, ip: `192.0.2.${index + 1}`,
      state: targetState, progress: 100, message: "本次任务的逐台演示结果"
    }))
  };
}

function render(value: ScreenTask | Ref<ScreenTask>) {
  return mount(NMessageProvider, {
    global: { plugins: [createPinia()], stubs: { teleport: true } },
    slots: { default: () => h(ScreenTaskResults, { task: unref(value) }) }
  });
}

describe("未解析 APK 的安装结果展示", () => {
  it("正在执行的安装不把内部待确认结果显示成故障",()=>{
    const value=task('running',['running']);value.mode='real';value.targets[0]!.progress=40;value.targets[0]!.message='正在安装小新2.0.9-5019';
    value.targets[0]!.result={device:'unknown',business:'not_required',shared:'not_required',evidence:{}};
    const wrapper=render(value);try{expect(wrapper.text()).toContain('设备：执行中');expect(wrapper.text()).not.toContain('设备：待核实');expect(wrapper.text()).toContain('40%');}finally{wrapper.unmount();}
  });
  it("成功终态仅表示流程演示完成，不宣称已安装或更新版本", () => {
    const value = task();
    const wrapper = render(value);
    try {
      expect(wrapper.get(".screen-result-header .n-tag").text()).toBe("演示完成");
      expect(wrapper.get("tbody .n-tag").text()).toBe("演示完成");
      const notice = wrapper.get('[data-testid="screen-install-preview-notice"]').text();
      expect(notice).toContain("安装流程演示完成");
      expect(notice).toContain("APK 元数据未解析");
      expect(notice).toContain("未执行真实安装");
      expect(notice).toContain("未更新版本记录");
      expect(wrapper.get("thead").text()).toContain("演示结果 / 处理建议");
      expect(value.state).toBe("succeeded");
    } finally { wrapper.unmount(); }
  });

  it.each([
    ["cancelled", ["cancelled"]],
    ["failed", ["failed"]],
    ["partially_succeeded", ["succeeded", "failed"]],
    ["needs_review", ["needs_review"]]
  ] as [ScreenTask["state"], ScreenTargetState[]][])("%s 保留准确终态及可重试范围", async (state, targets) => {
    const wrapper = render(task(state, targets));
    try {
      expect(wrapper.get(".screen-result-header .n-tag").text()).toBe(screenStateLabel[state]);
      expect(wrapper.get('[data-testid="screen-install-preview-notice"]').text()).not.toContain("安装流程演示完成");
      const retry = wrapper.findAll("button").find((button) => button.text().startsWith("重试"));
      if (state === "needs_review") {
        expect(retry).toBeUndefined();
        expect(wrapper.findAll("button").some((button) => button.text() === "继续核实结果")).toBe(true);
      } else {
        await retry!.trigger("click");
        const expected = targets.flatMap((target, index) => ["failed", "cancelled"].includes(target) ? [`screen-${index}`] : []);
        expect(wrapper.findComponent(ScreenTaskResults).emitted("retry")).toEqual([[expected]]);
      }
    } finally { wrapper.unmount(); }
  });

  it.each(["已知版本安装", "其他操作"])("%s 保留既有结果文案", (scenario) => {
    const value = task();
    if (scenario === "已知版本安装") value.input!.appVersion = "1.6.0";
    else { value.action = "mac"; value.input!.action = "mac"; }
    const wrapper = render(value);
    try {
      expect(wrapper.get(".screen-result-header .n-tag").text()).toBe("已成功");
      expect(wrapper.find('[data-testid="screen-install-preview-notice"]').exists()).toBe(false);
      expect(wrapper.get("thead").text()).toContain("实际结果 / 处理建议");
    } finally { wrapper.unmount(); }
  });
});

function configTask(state: ScreenTask["state"], targets: ScreenTargetState[], results: Record<string, unknown>[]): ScreenTask {
  const value = task(state, targets);
  value.action = "app_config";
  value.mode = "real";
  value.input!.action = "app_config";
  value.targets.forEach((target, index) => {
    target.result = {
      device: "unknown", business: "not_required",
      shared: "not_required", evidence: { config: results[index] }
    };
  });
  return value;
}

describe("紧凑操作结果与恢复动作", () => {
  it.each([
    ["online", "succeeded", "在线", "success"],
    ["offline", "succeeded", "离线", "error"],
    ["online", "failed", "在线", "success"],
    [null, "failed", "未知", "warning"],
    [null, "succeeded", "未知", "warning"],
    [null, "running", "检查中", "info"],
    [null, "queued", "未检查", "default"],
    [null, "cancelled", "未检查", "default"]
  ] as const)("在线检查 %s / 操作 %s 显示 %s，不用执行状态推断在线", (ping, state, label, type) => {
    const value = task("succeeded", [state]); value.action = "ping"; value.mode = "real";
    const target = value.targets[0]!;
    target.message = state === "failed" ? "本机检查程序异常，无法执行" : "检查完成，结果已保存在本机";
    target.result = { device: state === "failed" ? "failed" : "succeeded", business: "not_required", shared: "not_required",
      observation: ping ? { id: "this-check", operationType: "ping", observedIp: target.ip, observedAt: value.createdAt,
        ping, adbAvailable: false, abis: [], errors: [] } : null };
    const wrapper = render(value);
    try {
      const tag = wrapper.get("tbody .n-tag");
      expect(tag.text()).toBe(label);
      expect(wrapper.get("tbody").getComponent(NTag).props("type")).toBe(type);
      expect(wrapper.get("thead").text()).toContain("在线状态");
      expect(wrapper.get(".screen-result-header .n-tag").text()).toBe("检查完成");
      expect(wrapper.get("tbody").text()).not.toContain("设备：已完成");
      if (state === "failed") expect(wrapper.get(".screen-result-message").text()).toBe(target.message);
      if (state === "succeeded" && !ping) expect(wrapper.get(".screen-result-message").text()).toContain("未记录本次在线检查结果");
    } finally { wrapper.unmount(); }
  });

  it("在线检查地址不匹配时不展示其他地址的在线结果，程序错误保留具体说明", async () => {
    const value = ref(task()); value.value.action = "ping";
    const target = value.value.targets[0]!;
    target.result = { device: "succeeded", business: "not_required", shared: "not_required",
      observation: { id: "wrong-address", observedIp: "192.0.2.99", observedAt: value.value.createdAt,
        ping: "online", adbAvailable: false, abis: [], errors: [] } };
    const wrapper = render(value);
    try {
      expect(wrapper.get("tbody .n-tag").text()).toBe("未知");
      expect(wrapper.get(".screen-result-message").text()).toContain("检查地址与本次目标不一致");
      target.result.observation!.observedIp = target.ip;
      target.result.observation!.ping = null;
      target.result.observation!.errors = ["系统 Ping 程序未找到"];
      target.message = "检查异常"; target.state = "failed";
      await flushPromises();
      expect(wrapper.get("tbody .n-tag").text()).toBe("未知");
      expect(wrapper.get(".screen-result-message").text()).toContain("系统 Ping 程序未找到");
    } finally { wrapper.unmount(); }
  });

  it.each(["ping", "inspect", "mac"] as const)("%s 结果末列可查看对应屏详情，分页后仍传递该行标识", async (action) => {
    const value = task("succeeded", Array.from({ length: 22 }, () => "succeeded"));
    value.action = action;
    const wrapper = render(value);
    try {
      expect(wrapper.get("thead th:last-child").text()).toBe("查看屏详情");
      wrapper.getComponent(NPagination).vm.$emit("update:page-size", 20);
      await flushPromises();
      wrapper.getComponent(NPagination).vm.$emit("update:page", 2);
      await flushPromises();
      await wrapper.get('[data-screen-id="screen-21"] td:last-child button').trigger("click");
      expect(wrapper.getComponent(ScreenTaskResults).emitted("detail")).toEqual([["screen-21"]]);
      expect(wrapper.getComponent(NPagination).props("page")).toBe(2);
    } finally { wrapper.unmount(); }
  });

  it("每屏仅一行，分别展示保存、重启、回读阶段和未保存字段", () => {
    const wrapper = render(configTask("needs_review", ["failed", "failed", "needs_review"], [
      { save: "partial", restart: "not_started", readback: "different", failedFields: ["environments.pre.h5Url"] },
      { save: "failed", restart: "not_required", readback: "not_required" },
      { save: "unknown", restart: "not_confirmed", readback: "unknown" }
    ]));
    try {
      const rows = wrapper.findAll("tbody tr");
      expect(rows).toHaveLength(3);
      expect(wrapper.find(".screen-config-stage-results").exists()).toBe(false);
      expect(wrapper.get("thead").findAll("th").map(cell => cell.text())).toEqual([
        "目标屏", "IP 地址", "状态", "保存", "重启", "回读", "实际结果 / 处理建议"
      ]);
      const stages = rows.map(row => row.findAll("td").slice(3, 6).map(cell => cell.text()));
      expect(stages).toEqual([
        ["部分保存", "未重启", "与本次设置不同"],
        ["未完成", "无需重启", "无需执行"],
        ["待核实", "未确认重启", "待核实"]
      ]);
      expect(rows[0]!.get("td:nth-child(2)").text()).toBe("192.0.2.1");
      expect(rows[0]!.get(".screen-warning.screen-result-message").text()).toBe("未保存字段：预发环境 · H5地址(可选)");
      expect(rows[0]!.get(".screen-warning.screen-result-message").attributes("title")).toContain("H5地址(可选)");
    } finally { wrapper.unmount(); }
  });

  it("保存成功但重启失败只提供重启，不重复提交配置", async () => {
    const wrapper = render(configTask("failed", ["failed", "failed", "succeeded"], [
      { save: "saved", restartRequired: true, restart: "failed", readback: "pending" },
      { save: "verified", restartRequired: true, restart: "not_started", readback: "verified" },
      { save: "unchanged", restartRequired: false, restart: "not_required", readback: "verified" }
    ]));
    try {
      expect(wrapper.findAll("button").some(button => button.text().startsWith("重试"))).toBe(false);
      await wrapper.findAll("button").find(button => button.text() === "只重启小新 2 台")!.trigger("click");
      expect(wrapper.getComponent(ScreenTaskResults).emitted("restart")).toEqual([[ ["screen-0", "screen-1"] ]]);
      expect(wrapper.getComponent(ScreenTaskResults).emitted("retry")).toBeUndefined();
    } finally { wrapper.unmount(); }
  });

  it("待核实目标仅核实已有结果，不再次保存或重启", async () => {
    const verify = vi.fn().mockResolvedValue(undefined), execute = vi.fn();
    const adapter = vi.spyOn(screenApi, "useScreenAdapter").mockReturnValue({ mode: "real", verify, execute } as unknown as screenApi.ScreenAdapter);
    const value = configTask("needs_review", ["needs_review"], [{ save: "saved", restartRequired: true, restart: "unknown", readback: "unknown" }]);
    const wrapper = render(value);
    try {
      expect(wrapper.findAll("button").some(button => /^(重试|只重启)/.test(button.text()))).toBe(false);
      await wrapper.findAll("button").find(button => button.text() === "继续核实结果")!.trigger("click");
      await flushPromises();
      expect(verify).toHaveBeenCalledExactlyOnceWith(value.projectId, value.id);
      expect(execute).not.toHaveBeenCalled();
      expect(wrapper.getComponent(ScreenTaskResults).emitted("retry")).toBeUndefined();
      expect(wrapper.getComponent(ScreenTaskResults).emitted("restart")).toBeUndefined();
    } finally { wrapper.unmount(); adapter.mockRestore(); }
  });

  it("分页只改变显示，重试和版本核对覆盖全任务的相应目标", async () => {
    const states: ScreenTargetState[] = Array.from({ length: 22 }, (_, index) => [0, 1, 21].includes(index) ? "failed" : "succeeded");
    const value = task("partially_succeeded", states);
    value.mode = "real";
    value.targets[1]!.result = { device: "succeeded", business: "failed", shared: "failed", evidence: { phase: "installed" } };
    const wrapper = render(value);
    try {
      wrapper.getComponent(NPagination).vm.$emit("update:page-size", 20);
      await flushPromises();
      expect(wrapper.findAll("tbody tr")).toHaveLength(20);
      wrapper.getComponent(NPagination).vm.$emit("update:page", 2);
      await flushPromises();
      expect(wrapper.findAll("tbody tr").map(row => row.attributes("data-screen-id"))).toEqual(["screen-20", "screen-21"]);
      await wrapper.findAll("button").find(button => button.text().startsWith("重试 2 台"))!.trigger("click");
      expect(wrapper.getComponent(ScreenTaskResults).emitted("retry")).toEqual([[ ["screen-0", "screen-21"] ]]);
      await wrapper.findAll("button").find(button => button.text() === "核对版本并同步平台")!.trigger("click");
      expect(wrapper.getComponent(ScreenTaskResults).emitted("versions")).toEqual([[value.targets.map(target => target.screenId)]]);
    } finally { wrapper.unmount(); }
  });

  it("仅保留公共日志入口，切换任务后回到第一页", async () => {
    const value = ref(task("succeeded", Array.from({ length: 22 }, () => "succeeded")));
    value.value.logs = [{ time: "2026-10-07T00:00:00Z", level: "INFO", message: "本次配置回读完成" }];
    const wrapper = render(value);
    try {
      wrapper.getComponent(NPagination).vm.$emit("update:page-size", 20);
      await flushPromises();
      wrapper.getComponent(NPagination).vm.$emit("update:page", 2);
      await flushPromises();
      expect(wrapper.findAll("button").filter(button => button.text() === "任务与日志面板")).toHaveLength(1);
      expect(wrapper.findAll("button").some(button => button.text() === "本次日志")).toBe(false);
      expect(wrapper.find(".screen-result-logs").exists()).toBe(false);
      value.value = { ...task(), id: "next-task", logs: [] };
      await flushPromises();
      expect(wrapper.getComponent(NPagination).props("page")).toBe(1);
      expect(wrapper.findAll("tbody tr")).toHaveLength(1);
    } finally { wrapper.unmount(); }
  });

  it("公共日志仍定位当前任务并清除旧筛选", async () => {
    const value = task();
    const wrapper = render(value), activity = useActivityStore();
    activity.logKeyword = "上一个任务";
    activity.logLevels = ["ERROR"];
    const refresh = vi.spyOn(activity, "refreshTasks").mockResolvedValue(undefined);
    const select = vi.spyOn(activity, "selectTask").mockResolvedValue(undefined);
    const open = vi.spyOn(activity, "openPanel");
    try {
      await wrapper.findAll("button").find(button => button.text() === "任务与日志面板")!.trigger("click");
      await flushPromises();
      expect(refresh).toHaveBeenCalledExactlyOnceWith(value.projectId, value.id);
      expect(select).toHaveBeenCalledExactlyOnceWith(value.id);
      expect(open).toHaveBeenCalledExactlyOnceWith("logs");
      expect(activity.logKeyword).toBe("");
      expect(activity.logLevels).toEqual([]);
      expect(wrapper.getComponent(ScreenTaskResults).emitted("logsOpened")).toHaveLength(1);
    } finally { wrapper.unmount(); refresh.mockRestore(); select.mockRestore(); open.mockRestore(); }
  });

  it.each(["切换查看任务", "在面板改选任务"])("%s后，旧日志请求不能重新打开面板或选中旧任务", async (action) => {
    const value = ref(task()), wrapper = render(value), activity = useActivityStore();
    let finish!: () => void;
    const refresh = vi.spyOn(activity, "refreshTasks").mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
    const select = vi.spyOn(activity, "selectTask").mockResolvedValue(undefined), open = vi.spyOn(activity, "openPanel");
    try {
      await wrapper.findAll("button").find(button => button.text() === "任务与日志面板")!.trigger("click");
      if (action === "切换查看任务") value.value = { ...task(), id: "new-task" };
      else activity.selectedTaskId = "manually-selected-task";
      await flushPromises();
      finish(); await flushPromises();
      expect(select).not.toHaveBeenCalled(); expect(open).not.toHaveBeenCalled();
      expect(wrapper.getComponent(ScreenTaskResults).emitted("logsOpened")).toBeUndefined();
    } finally { wrapper.unmount(); refresh.mockRestore(); select.mockRestore(); open.mockRestore(); }
  });
});
