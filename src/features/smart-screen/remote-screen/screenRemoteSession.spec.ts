import { nextTick } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ScreenRemoteAdapter } from "@/shared/api/screenRemoteAdapter";
import type { ScreenRemoteControlMode, ScreenRemoteFrame, ScreenRemoteSession } from "@/shared/model/screenRemote";
import { browserScreenFrameUrls, createScreenRemoteSession, SCREEN_REMOTE_REFRESH_INTERVAL_MS } from "./screenRemoteSession";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (value: unknown) => void;
  const promise = new Promise<T>((done, failed) => { resolve = done; reject = failed; });
  return { promise, resolve, reject };
}
async function flush() { for (let index = 0; index < 12; index += 1) await Promise.resolve(); await nextTick(); }
const session = (id = "one"): ScreenRemoteSession => ({ sessionId: id, projectId: "project", screenId: id, name: id, ip: "192.0.2.1" });
function fixture(textInputBehavior: "insert" | "replace" = "insert", controlMode: ScreenRemoteControlMode = "full") {
  let controlling = false, serverGeneration = 0, frameNumber = 0;
  const nextFrame = (id = "one", extra: Partial<ScreenRemoteFrame> = {}): ScreenRemoteFrame => ({
    sessionId: id, frameId: `frame${++frameNumber}`, width: 800, height: 480,
    capturedAt: "2026-10-26T03:20:00Z", elapsedMs: 100, pngBase64: "iVBORw0KGgo=",
    controlActive: controlling, inputReady: controlling, inputGeneration: serverGeneration, ...extra,
  });
  const adapter = {
    open: vi.fn(async (_project: string, id: string) => session(id)),
    capture: vi.fn<ScreenRemoteAdapter["capture"]>(async (_project, id) => nextFrame(id)),
    close: vi.fn(async () => {}),
    beginControl: vi.fn<ScreenRemoteAdapter["beginControl"]>(async () => {
      controlling = true; serverGeneration += 1;
      return { taskId: "control-task", idleTimeoutMs: 300000, frameMaxAgeMs: 30000, maxTextLength: 128, textInputBehavior, inputGeneration: serverGeneration, controlMode };
    }),
    endControl: vi.fn(async () => { controlling = false; serverGeneration += 1; }),
    action: vi.fn<ScreenRemoteAdapter["action"]>(async () => {
      serverGeneration += 2;
      return { status: "sent", message: "操作已发送", controlActive: true, inputGeneration: serverGeneration, controlMode };
    }),
  } satisfies ScreenRemoteAdapter;
  const urls = { create: vi.fn((value: ScreenRemoteFrame) => `blob:${value.frameId}`), revoke: vi.fn() };
  const remote = createScreenRemoteSession(adapter, urls);
  async function show() {
    await flush();
    const url = remote.state.pendingImageUrl;
    expect(url, "应存在待显示候选图").not.toBe("");
    remote.imageLoaded(url);
    await flush();
  }
  async function openReady() { await remote.open({ projectId: "project", screenId: "one" }); await show(); }
  async function controlReady() { await openReady(); await remote.beginControl(); await show(); }
  return { adapter, urls, remote, nextFrame, show, openReady, controlReady,
    setGeneration(value: number) { serverGeneration = value; },
    setControlMode(value: ScreenRemoteControlMode) { controlMode = value; },
  };
}

describe("单屏显示帧与截图/输入独立通道", () => {
  afterEach(() => { vi.useRealTimers(); });

  it("首图解码前不显示，默认不轮询；替换图解码前保留当前图，结束释放两帧", async () => {
    vi.useFakeTimers();
    const f = fixture();
    await f.remote.open({ projectId: "project", screenId: "one" });
    expect(f.remote.state.imageUrl).toBe("");
    expect(f.remote.state.pendingImageUrl).toBe("blob:frame1");
    await f.show();
    expect(f.remote.state.frame).not.toHaveProperty("pngBase64");
    await vi.advanceTimersByTimeAsync(6000);
    expect(f.adapter.capture).toHaveBeenCalledTimes(1);
    await f.remote.refresh();
    expect(f.remote.state.imageUrl).toBe("blob:frame1");
    expect(f.remote.state.pendingImageUrl).toBe("blob:frame2");
    expect(f.urls.revoke).not.toHaveBeenCalled();
    await f.show();
    expect(f.remote.state.imageUrl).toBe("blob:frame2");
    expect(f.urls.revoke).toHaveBeenCalledWith("blob:frame1");
    await f.remote.refresh();
    await f.remote.close();
    expect(f.urls.revoke).toHaveBeenCalledWith("blob:frame2");
    expect(f.urls.revoke).toHaveBeenCalledWith("blob:frame3");
    expect(f.remote.state.imageUrl).toBe("");
    expect(f.remote.state.pendingImageUrl).toBe("");
  });

  it("一秒自动刷新在采集/解码完成后计时，慢采集与解码均不积压", async () => {
    vi.useFakeTimers();
    const f = fixture(); await f.openReady();
    expect(SCREEN_REMOTE_REFRESH_INTERVAL_MS).toBe(1000);
    const pending = deferred<ScreenRemoteFrame>();
    f.adapter.capture.mockReturnValueOnce(pending.promise);
    f.remote.setAutoRefresh(true);
    await vi.advanceTimersByTimeAsync(999);
    expect(f.adapter.capture).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    const manual = f.remote.refresh();
    await vi.advanceTimersByTimeAsync(5000);
    expect(f.adapter.capture).toHaveBeenCalledTimes(2);
    pending.resolve(f.nextFrame()); await manual;
    await vi.advanceTimersByTimeAsync(5000);
    expect(f.adapter.capture).toHaveBeenCalledTimes(2);
    await f.show();
    await vi.advanceTimersByTimeAsync(999);
    expect(f.adapter.capture).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(f.adapter.capture).toHaveBeenCalledTimes(3);
    await f.remote.close();
    await vi.advanceTimersByTimeAsync(5000);
    expect(f.adapter.capture).toHaveBeenCalledTimes(3);
  });

  it("关闭后网络和解码迟到都不复活画面", async () => {
    const f = fixture(); await f.openReady();
    const pending = deferred<ScreenRemoteFrame>();
    f.adapter.capture.mockReturnValueOnce(pending.promise);
    const request = f.remote.refresh();
    await f.remote.close();
    pending.resolve(f.nextFrame("one", { frameId: "late" })); await request;
    f.remote.imageLoaded("blob:late");
    expect(f.remote.state.imageUrl).toBe("");
    expect(f.remote.state.pendingImageUrl).toBe("");
    expect(f.urls.create).not.toHaveBeenCalledWith(expect.objectContaining({ frameId: "late" }));
  });

  it("换屏等待旧open收尾，旧session只关闭而不串到新屏", async () => {
    const f = fixture(), pending = deferred<ScreenRemoteSession>();
    f.adapter.open.mockReturnValueOnce(pending.promise);
    const first = f.remote.open({ projectId: "project", screenId: "one" });
    await flush();
    const next = f.remote.open({ projectId: "project", screenId: "two" });
    pending.resolve(session("one")); await Promise.all([first, next]); await f.show();
    expect(f.adapter.close).toHaveBeenCalledWith("project", "one");
    expect(f.adapter.capture.mock.calls).toEqual([["project", "two"]]);
    expect(f.remote.state.session?.screenId).toBe("two");
    await f.remote.close();
  });

  it("截图失败保留旧图并停止刷新，成功新帧显示后才恢复", async () => {
    vi.useFakeTimers();
    const f = fixture(); await f.controlReady();
    const previous = f.remote.state.imageUrl;
    f.adapter.capture.mockRejectedValueOnce(new Error("设备连接中断"));
    f.remote.setAutoRefresh(true);
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.remote.state.imageUrl).toBe(previous);
    expect(f.remote.state.stale).toBe(true);
    expect(f.remote.canAct()).toBe(false);
    expect(f.remote.state.autoRefresh).toBe(false);
    const count = f.adapter.capture.mock.calls.length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(f.adapter.capture).toHaveBeenCalledTimes(count);
    await f.remote.refresh();
    expect(f.remote.canAct()).toBe(false);
    await f.show();
    expect(f.remote.state.error).toBe("");
    expect(f.remote.canAct()).toBe(true);
    await f.remote.close();
  });

  it("候选图解码失败或超时保留当前图，不能借旧图继续输入", async () => {
    vi.useFakeTimers();
    const f = fixture(); await f.controlReady();
    const current = f.remote.state.imageUrl;
    await f.remote.refresh();
    const bad = f.remote.state.pendingImageUrl;
    f.remote.imageFailed(bad);
    expect(f.remote.state.imageUrl).toBe(current);
    expect(f.remote.state.error).toContain("无法显示");
    expect(f.remote.canAct()).toBe(false);
    expect(f.urls.revoke).toHaveBeenCalledWith(bad);
    await f.remote.refresh();
    await vi.advanceTimersByTimeAsync(10000);
    expect(f.remote.state.pendingImageUrl).toBe("");
    expect(f.remote.state.imageUrl).toBe(current);
    f.remote.imageLoaded(bad);
    expect(f.remote.state.imageUrl).toBe(current);
    await f.remote.close();
  });

  it("默认只查看，开启辅助后必须显示该控制代次的可输入帧", async () => {
    const f = fixture(); await f.openReady();
    expect(await f.remote.perform({ kind: "tap", x: 4, y: 8 })).toBe(false);
    await f.remote.beginControl(); await flush();
    expect(f.remote.canAct()).toBe(false);
    f.remote.imageLoaded("blob:wrong");
    expect(f.remote.canAct()).toBe(false);
    await f.show();
    expect(f.remote.canAct()).toBe(true);
    await f.remote.close();
  });

  it("休眠模式只允许用有效已显示帧唤醒，确认全功能后仍等操作后新图", async () => {
    const f = fixture("insert", "wake_only"); await f.openReady();
    await f.remote.beginControl();
    expect(f.remote.state.operationMessage).toBe("屏幕已休眠，请点击唤醒");
    expect(f.remote.canWake()).toBe(false);
    await f.show();
    expect(f.remote.canWake()).toBe(true);
    expect(f.remote.canAct()).toBe(false);
    for (const action of [{ kind: "tap", x: 4, y: 8 }, { kind: "text", text: "fixture" }, { kind: "key", key: "back" }, { kind: "key", key: "backspace" }] as const) {
      expect(await f.remote.perform(action)).toBe(false);
    }
    const visibleFrame = f.remote.state.frame!.frameId;
    f.setControlMode("full");
    expect(await f.remote.perform({ kind: "key", key: "wake_up" })).toBe(true);
    expect(f.adapter.action).toHaveBeenCalledExactlyOnceWith("project", "one", { frameId: visibleFrame, action: { kind: "key", key: "wake_up" } });
    expect(f.remote.state.controlMode).toBe("full");
    expect(f.remote.canAct()).toBe(false);
    expect(f.remote.canWake()).toBe(false);
    await f.show();
    expect(f.remote.canAct()).toBe(true);
    expect(f.remote.canWake()).toBe(true);
    await f.remote.close();
  });

  it.each([false, true])("唤醒已发送但小新未确认就绪时退出辅助，不重复唤醒或开放普通操作（后端active=%s）", async (controlActive) => {
    const f = fixture("replace", "wake_only"); await f.controlReady();
    f.adapter.action.mockResolvedValueOnce({ status: "sent", message: "唤醒已发送，请待画面恢复后重新开启辅助操作", controlActive, inputGeneration: 3, controlMode: "wake_only" });
    expect(await f.remote.perform({ kind: "key", key: "wake_up" })).toBe(true);
    await f.show();
    expect(f.remote.state.operationMessage).toBe("唤醒已发送，请待画面恢复后重新开启辅助操作");
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.canWake()).toBe(false);
    expect(f.remote.canAct()).toBe(false);
    expect(await f.remote.perform({ kind: "key", key: "wake_up" })).toBe(false);
    expect(f.adapter.action).toHaveBeenCalledTimes(1);
    expect(f.adapter.endControl).toHaveBeenCalledTimes(1);
    await f.remote.close();
  });

  it("仅唤醒也遵守过期、截图失败、输入单飞与关闭后的迟到回执保护", async () => {
    vi.useFakeTimers();
    const f = fixture("insert", "wake_only"); await f.controlReady();
    await vi.advanceTimersByTimeAsync(29500);
    expect(await f.remote.perform({ kind: "key", key: "wake_up" })).toBe(false);
    f.remote.explainUnavailableAction();
    expect(f.remote.state.operationMessage).toContain("画面已过期");
    await f.remote.refresh(); await f.show();
    f.adapter.capture.mockRejectedValueOnce(new Error("设备断开"));
    await f.remote.refresh();
    expect(f.remote.canWake()).toBe(false);
    await f.remote.refresh(); await f.show();
    const receipt = deferred<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>();
    f.adapter.action.mockReturnValueOnce(receipt.promise);
    const sending = f.remote.perform({ kind: "key", key: "wake_up" });
    expect(await f.remote.perform({ kind: "key", key: "wake_up" })).toBe(false);
    await f.remote.close();
    receipt.resolve({ status: "sent", message: "已发送", controlActive: true, inputGeneration: 3, controlMode: "full" });
    expect(await sending).toBe(false);
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.imageUrl).toBe("");
    expect(f.adapter.action).toHaveBeenCalledTimes(1);
  });

  it("未知或缺失控制模式不能被当作全功能授权", async () => {
    const f = fixture(); await f.openReady();
    const invalid = { taskId: "bad", frameMaxAgeMs: 30000, idleTimeoutMs: 300000, maxTextLength: 128, textInputBehavior: "insert", inputGeneration: 1, controlMode: "unknown" };
    f.adapter.beginControl.mockResolvedValueOnce(invalid as Awaited<ReturnType<ScreenRemoteAdapter["beginControl"]>>);
    await f.remote.beginControl();
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.operationMessage).toContain("能力无效");
    await f.show();
    await f.remote.beginControl(); await f.show();
    f.adapter.action.mockResolvedValueOnce({ status: "sent", message: "已发送", controlActive: true, inputGeneration: 3 } as Awaited<ReturnType<ScreenRemoteAdapter["action"]>>);
    expect(await f.remote.perform({ kind: "key", key: "wake_up" })).toBe(false);
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.operationMessage).toContain("结果未确认");
    await f.remote.close();
  });

  it("后台采集和候选解码不禁用当前已显示有效帧，输入使用可见旧frameId", async () => {
    const f = fixture(); await f.controlReady();
    const displayed = f.remote.state.frame!.frameId, oldUrl = f.remote.state.imageUrl;
    const pending = deferred<ScreenRemoteFrame>();
    f.adapter.capture.mockReturnValueOnce(pending.promise);
    const capture = f.remote.refresh();
    expect(f.remote.state.capturing).toBe(true);
    expect(f.remote.canAct()).toBe(true);
    pending.resolve(f.nextFrame()); await capture;
    expect(f.remote.state.decoding).toBe(true);
    expect(f.remote.state.imageUrl).toBe(oldUrl);
    expect(f.remote.canAct()).toBe(true);
    await f.remote.perform({ kind: "tap", x: 4, y: 8 });
    expect(f.adapter.action).toHaveBeenCalledExactlyOnceWith("project", "one", { frameId: displayed, action: { kind: "tap", x: 4, y: 8 } });
    expect(f.remote.canAct()).toBe(false);
    await f.show();
    expect(f.remote.canAct()).toBe(true);
    await f.remote.close();
  });

  it("跨输入开始/结束的迟到截图被丢弃，单飞结束后仅补一次操作后新帧", async () => {
    const f = fixture(); await f.controlReady();
    const pending = deferred<ScreenRemoteFrame>();
    const stale = f.nextFrame("one", { frameId: "pre-action", inputGeneration: 1, inputReady: true });
    f.adapter.capture.mockReturnValueOnce(pending.promise);
    const capture = f.remote.refresh();
    await f.remote.perform({ kind: "key", key: "back" });
    expect(f.adapter.capture).toHaveBeenCalledTimes(3);
    expect(f.remote.canAct()).toBe(false);
    pending.resolve(stale); await capture; await flush();
    expect(f.urls.create).not.toHaveBeenCalledWith(expect.objectContaining({ frameId: "pre-action" }));
    expect(f.adapter.capture).toHaveBeenCalledTimes(4);
    await f.show();
    expect(f.remote.state.frame?.inputGeneration).toBe(3);
    expect(f.remote.canAct()).toBe(true);
    await f.remote.close();
  });

  it("输入期间截图仍可显示但不开放下一输入，完成后必须新采集并解码", async () => {
    const f = fixture(); await f.controlReady();
    const receipt = deferred<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>();
    f.adapter.action.mockImplementationOnce(async () => { f.setGeneration(2); return receipt.promise; });
    const sending = f.remote.perform({ kind: "tap", x: 4, y: 8 });
    f.adapter.capture.mockResolvedValueOnce(f.nextFrame("one", { frameId: "during-action", inputGeneration: 2, inputReady: false }));
    await f.remote.refresh(); await f.show();
    expect(f.remote.state.frame?.frameId).toBe("during-action");
    expect(f.remote.state.acting).toBe(true);
    expect(f.remote.canAct()).toBe(false);
    expect(await f.remote.perform({ kind: "key", key: "back" })).toBe(false);
    f.setGeneration(3);
    receipt.resolve({ status: "sent", message: "已发送", controlActive: true, inputGeneration: 3, controlMode: "full" });
    await sending; await flush();
    expect(f.remote.canAct()).toBe(false);
    await f.show();
    expect(f.remote.canAct()).toBe(true);
    expect(f.adapter.action).toHaveBeenCalledTimes(1);
    await f.remote.close();
  });

  it("输入开始使旧解码候选失效，迟到load不能在操作后重新解锁", async () => {
    const f = fixture(); await f.controlReady();
    await f.remote.refresh();
    const oldCandidate = f.remote.state.pendingImageUrl;
    await f.remote.perform({ kind: "key", key: "back" }); await flush();
    f.remote.imageLoaded(oldCandidate);
    expect(f.remote.canAct()).toBe(false);
    expect(f.urls.revoke).toHaveBeenCalledWith(oldCandidate);
    await f.show();
    expect(f.remote.canAct()).toBe(true);
    await f.remote.close();
  });

  it("后台代次早于动作回执的旧ready帧不显示也不解锁", async () => {
    const f = fixture(); await f.controlReady();
    f.adapter.capture.mockResolvedValueOnce(f.nextFrame("one", { frameId: "old-ready", inputGeneration: 1, inputReady: true }));
    await f.remote.perform({ kind: "key", key: "back" }); await flush();
    expect(f.urls.create).not.toHaveBeenCalledWith(expect.objectContaining({ frameId: "old-ready" }));
    expect(f.remote.canAct()).toBe(false);
    await f.remote.refresh(); await f.show();
    expect(f.remote.canAct()).toBe(true);
    await f.remote.close();
  });

  it("刷新中也能启停辅助，旧控制代次回包不能结束或解锁新控制", async () => {
    const f = fixture(); await f.controlReady();
    const pending = deferred<ScreenRemoteFrame>();
    f.adapter.capture.mockReturnValueOnce(pending.promise);
    const capturing = f.remote.refresh();
    await f.remote.endControl();
    await f.remote.beginControl();
    expect(f.remote.state.controlActive).toBe(true);
    pending.resolve(f.nextFrame("one", { frameId: "old-control", inputGeneration: 1, controlActive: false, inputReady: false }));
    await capturing; await flush();
    expect(f.urls.create).not.toHaveBeenCalledWith(expect.objectContaining({ frameId: "old-control" }));
    expect(f.remote.state.controlActive).toBe(true);
    await f.show();
    expect(f.remote.canAct()).toBe(true);
    await f.remote.close();
  });

  it("输入在途不排队重发，异常不回显密码，控制结束后仍刷新只读画面", async () => {
    const f = fixture(); await f.controlReady();
    const pending = deferred<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>();
    f.adapter.action.mockReturnValueOnce(pending.promise);
    const secret = "Fixture-Private!%s";
    const sending = f.remote.perform({ kind: "text", text: secret });
    expect(await f.remote.perform({ kind: "key", key: "back" })).toBe(false);
    expect(JSON.stringify(f.remote.state)).not.toContain(secret);
    pending.reject(new Error(secret)); expect(await sending).toBe(false);
    await f.show();
    expect(f.adapter.action).toHaveBeenCalledTimes(1);
    expect(f.adapter.endControl).toHaveBeenCalledTimes(1);
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.operationMessage).toContain("未确认");
    expect(JSON.stringify(f.remote.state)).not.toContain(secret);
    await f.remote.close();
  });

  it("过期、越界和不支持文字在发送前拒绝，保持原输入行为白名单", async () => {
    vi.useFakeTimers();
    for (const behavior of ["insert", "replace"] as const) {
      const f = fixture(behavior); await f.controlReady();
      expect(await f.remote.perform({ kind: "tap", x: 800, y: 2 })).toBe(false);
      expect(await f.remote.perform({ kind: "text", text: "中文🙂" })).toBe(false);
      expect(await f.remote.perform({ kind: "text", text: "" })).toBe(false);
      expect(await f.remote.perform({ kind: "key", key: behavior === "insert" ? "clear_text" : "backspace" })).toBe(false);
      await vi.advanceTimersByTimeAsync(29500);
      expect(await f.remote.perform({ kind: "key", key: "back" })).toBe(false);
      expect(f.adapter.action).not.toHaveBeenCalled();
      await f.remote.refresh(); await f.show();
      expect(await f.remote.perform({ kind: "key", key: behavior === "insert" ? "backspace" : "clear_text" })).toBe(true);
      await f.remote.close();
    }
  });

  it("未确认响应或结束后的迟到动作回执不能恢复控制", async () => {
    const f = fixture(); await f.controlReady();
    f.adapter.action.mockResolvedValueOnce({ status: "unconfirmed", message: "不可回显", controlActive: false, inputGeneration: 3, controlMode: "full" });
    expect(await f.remote.perform({ kind: "key", key: "back" })).toBe(false);
    expect(f.remote.state.controlActive).toBe(false);
    await f.show(); await f.remote.beginControl(); await f.show();
    const pending = deferred<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>();
    f.adapter.action.mockReturnValueOnce(pending.promise);
    const sending = f.remote.perform({ kind: "key", key: "back" });
    await f.remote.endControl();
    pending.resolve({ status: "sent", message: "已发送", controlActive: true, inputGeneration: 99, controlMode: "full" });
    await sending; await f.show();
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.operationMessage).toBe("已停止辅助操作");
    await f.remote.close();
  });

  it.each([
    { mode: "wake_only", status: "sent", key: "wake_up", message: "唤醒已发送，请待画面恢复后重新开启辅助操作", expected: "唤醒已发送" },
    { mode: "full", status: "sent", key: "back", message: "已发送", expected: "操作已发送" },
    { mode: "full", status: "unconfirmed", key: "back", message: "结果未确认", expected: "操作结果未确认" },
  ] as const)("后台截图先观察到会话结束仍保留在途操作回执：$mode/$status", async ({ mode, status, key, message, expected }) => {
    const f = fixture("insert", mode); await f.controlReady();
    const receipt = deferred<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>();
    f.adapter.action.mockReturnValueOnce(receipt.promise);
    const sending = f.remote.perform({ kind: "key", key });
    f.setGeneration(2);
    f.adapter.capture.mockResolvedValueOnce(f.nextFrame("one", { controlActive: false, inputReady: false }));
    await f.remote.refresh(); await f.show();
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.acting).toBe(true);
    expect(f.remote.canAct()).toBe(false);
    expect(f.remote.canWake()).toBe(false);
    expect(await f.remote.perform({ kind: "key", key })).toBe(false);
    receipt.resolve({ status, message, controlActive: false, inputGeneration: 3, controlMode: mode });
    expect(await sending).toBe(status === "sent");
    await f.show();
    expect(f.remote.state.operationMessage).toContain(expected);
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.acting).toBe(false);
    expect(f.remote.canAct()).toBe(false);
    expect(f.remote.canWake()).toBe(false);
    expect(f.adapter.action).toHaveBeenCalledTimes(1);
    await f.remote.close();
  });

  it("后台已观察结束后主动停止仍拒绝在途唤醒的迟到授权", async () => {
    const f = fixture("insert", "wake_only"); await f.controlReady();
    const receipt = deferred<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>();
    f.adapter.action.mockReturnValueOnce(receipt.promise);
    const sending = f.remote.perform({ kind: "key", key: "wake_up" });
    f.adapter.capture.mockResolvedValueOnce(f.nextFrame("one", { controlActive: false, inputReady: false }));
    await f.remote.refresh(); await f.show();
    await f.remote.endControl();
    receipt.resolve({ status: "sent", message: "已发送", controlActive: true, inputGeneration: 3, controlMode: "full" });
    expect(await sending).toBe(false);
    await f.show();
    expect(f.remote.state.operationMessage).toBe("已停止辅助操作");
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.canWake()).toBe(false);
    expect(f.adapter.action).toHaveBeenCalledTimes(1);
    await f.remote.close();
  });

  it("开启控制未返回时关闭窗口，迟到授权被解除且不恢复图", async () => {
    const f = fixture(); await f.openReady();
    const pending = deferred<Awaited<ReturnType<ScreenRemoteAdapter["beginControl"]>>>();
    f.adapter.beginControl.mockReturnValueOnce(pending.promise);
    const enabling = f.remote.beginControl();
    await f.remote.close();
    pending.resolve({ taskId: "old", frameMaxAgeMs: 30000, idleTimeoutMs: 300000, maxTextLength: 128, textInputBehavior: "insert", inputGeneration: 1, controlMode: "full" });
    await enabling;
    expect(f.adapter.endControl).toHaveBeenCalledTimes(1);
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.remote.state.opened).toBe(false);
    expect(f.remote.state.imageUrl).toBe("");
  });

  it("后台控制已结束与本机闲置到期仍回到查看模式", async () => {
    vi.useFakeTimers();
    const f = fixture(); await f.controlReady();
    f.adapter.capture.mockResolvedValueOnce(f.nextFrame("one", { controlActive: false, inputReady: false }));
    await f.remote.refresh(); await f.show();
    expect(f.remote.state.controlActive).toBe(false);
    await f.remote.beginControl(); await f.show();
    await vi.advanceTimersByTimeAsync(300000); await flush();
    expect(f.remote.state.controlActive).toBe(false);
    expect(f.adapter.endControl).toHaveBeenCalledTimes(1);
    expect(f.remote.state.operationMessage).toContain("长时间未使用");
    await f.remote.close();
  });

  it("错会话、无效帧字段及非PNG内容不能成为当前图片", async () => {
    const f = fixture(); await f.openReady();
    f.adapter.capture.mockResolvedValueOnce(f.nextFrame("another"));
    await f.remote.refresh();
    expect(f.remote.state.error).toContain("会话已变化");
    f.adapter.capture.mockResolvedValueOnce(f.nextFrame("one", { width: 0 }));
    await f.remote.refresh();
    expect(f.remote.state.error).toContain("响应无效");
    expect(() => browserScreenFrameUrls.create(f.nextFrame("one", { pngBase64: btoa("not PNG") }))).toThrow("格式无效");
    expect(() => browserScreenFrameUrls.create(f.nextFrame("one", { pngBase64: "!not-base64" }))).toThrow("格式无效");
    expect(() => browserScreenFrameUrls.create(f.nextFrame("one", { pngBase64: "A".repeat(12 * 1024 * 1024 + 1) }))).toThrow("超过显示上限");
    await f.remote.close();
  });
});
