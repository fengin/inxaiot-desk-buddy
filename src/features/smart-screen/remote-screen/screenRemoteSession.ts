import { nextTick, shallowReactive } from "vue";
import type { ScreenRemoteAdapter } from "@/shared/api/screenRemoteAdapter";
import type { ScreenRemoteAction, ScreenRemoteControlMode, ScreenRemoteFrame, ScreenRemoteFrameInfo, ScreenRemoteSession, ScreenRemoteTarget } from "@/shared/model/screenRemote";
import { SCREEN_REMOTE_TEXT_LIMIT, screenRemoteTextError } from "./screenRemoteInput";

export const SCREEN_REMOTE_REFRESH_INTERVAL_MS = 1000;

export interface ScreenFrameUrls {
  create(frame: ScreenRemoteFrame): string;
  revoke(url: string): void;
}

export const browserScreenFrameUrls: ScreenFrameUrls = {
  create(frame) {
    // 后端也做PNG及体积核验；前端不接受任意URL、SVG或无限长响应。
    if (frame.pngBase64.length > 12 * 1024 * 1024) throw new Error("屏幕图片超过显示上限");
    let raw: string;
    try { raw = atob(frame.pngBase64); }
    catch { throw new Error("屏幕图片格式无效，请重新读取"); }
    const signature = [137, 80, 78, 71, 13, 10, 26, 10];
    if (!signature.every((value, index) => raw.charCodeAt(index) === value)) {
      throw new Error("屏幕图片格式无效，请重新读取");
    }
    const bytes = new Uint8Array(raw.length);
    for (let index = 0; index < raw.length; index += 1) bytes[index] = raw.charCodeAt(index);
    return URL.createObjectURL(new Blob([bytes.buffer], { type: "image/png" }));
  },
  revoke(url) { URL.revokeObjectURL(url); },
};

/** 截图和输入各自单飞。当前已显示帧在后台采集和解码时继续有效。 */
export function createScreenRemoteSession(adapter: ScreenRemoteAdapter, urls = browserScreenFrameUrls) {
  const state = shallowReactive({
    opened: false, connecting: false, capturing: false, decoding: false, autoRefresh: false,
    session: null as ScreenRemoteSession | null,
    frame: null as ScreenRemoteFrameInfo | null, imageUrl: "",
    pendingFrame: null as ScreenRemoteFrameInfo | null, pendingImageUrl: "",
    error: "", stale: false, imageLoaded: false, frameExpired: false,
    controlActive: false, controlChanging: false, acting: false, waitingForInputFrame: false,
    maxTextLength: SCREEN_REMOTE_TEXT_LIMIT, textInputBehavior: "insert" as "insert" | "replace",
    controlMode: "full" as ScreenRemoteControlMode,
    operationMessage: "", operationUnconfirmed: false,
  });
  type Candidate = { frame: ScreenRemoteFrameInfo; url: string; generation: number; epoch: number; startedAt: number; session: ScreenRemoteSession };
  let generation = 0;
  let controlGeneration = 0;
  let frameEpoch = 0;
  let displayedEpoch = -1;
  let minimumInputGeneration = 0;
  let frameDeadline = 0;
  let frameMaxAgeMs = 30_000;
  let idleTimeoutMs = 300_000;
  let opening: Promise<void> | undefined;
  let capture: { sessionId: string; promise: Promise<void> } | undefined;
  let candidate: Candidate | undefined;
  let requiredCapture = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let frameTimer: ReturnType<typeof setTimeout> | undefined;
  let idleTimer: ReturnType<typeof setTimeout> | undefined;
  let decodeTimer: ReturnType<typeof setTimeout> | undefined;

  function stopTimer() { if (timer !== undefined) clearTimeout(timer); timer = undefined; }
  function stopIdleTimer() { if (idleTimer !== undefined) clearTimeout(idleTimer); idleTimer = undefined; }
  function clearCandidate() {
    if (decodeTimer !== undefined) clearTimeout(decodeTimer);
    decodeTimer = undefined;
    const previous = candidate;
    candidate = undefined;
    state.pendingFrame = null; state.pendingImageUrl = ""; state.decoding = false;
    if (previous && previous.url !== state.imageUrl) urls.revoke(previous.url);
  }
  function clearFrame() {
    clearCandidate();
    if (frameTimer !== undefined) clearTimeout(frameTimer);
    frameTimer = undefined; frameDeadline = 0; displayedEpoch = -1;
    if (state.imageUrl) urls.revoke(state.imageUrl);
    state.imageUrl = ""; state.frame = null; state.stale = false;
    state.imageLoaded = false; state.frameExpired = false;
  }
  function boundary() {
    frameEpoch += 1;
    clearCandidate();
    state.waitingForInputFrame = state.controlActive || state.acting || state.controlChanging;
  }
  function resetControl() {
    controlGeneration += 1; stopIdleTimer(); boundary();
    state.controlActive = false; state.controlChanging = false; state.acting = false;
    state.waitingForInputFrame = false; state.operationMessage = ""; state.operationUnconfirmed = false;
    state.textInputBehavior = "insert"; state.controlMode = "full"; minimumInputGeneration = 0;
  }
  function touchControl() {
    stopIdleTimer();
    if (state.controlActive) idleTimer = setTimeout(() => {
      idleTimer = undefined; void endControl("辅助操作已因长时间未使用而停止");
    }, idleTimeoutMs);
  }
  function setFrameDeadline(startedAt: number) {
    if (frameTimer !== undefined) clearTimeout(frameTimer);
    frameDeadline = startedAt + Math.max(0, frameMaxAgeMs - 500);
    state.frameExpired = performance.now() >= frameDeadline;
    frameTimer = setTimeout(() => { state.frameExpired = true; frameTimer = undefined; }, Math.max(0, frameDeadline - performance.now()));
  }
  async function release(session: ScreenRemoteSession | null) {
    if (!session) return;
    try { await adapter.close(session.projectId, session.sessionId); } catch { /* cancellation and expiry are backend safeguards */ }
  }
  async function releaseControl(session: ScreenRemoteSession) {
    try { await adapter.endControl(session.projectId, session.sessionId); } catch { /* close/expiry also ends the lease */ }
  }
  function schedule() {
    stopTimer();
    if (state.opened && state.autoRefresh && state.session && !state.error && !capture && !candidate && !state.connecting) {
      timer = setTimeout(() => { timer = undefined; void refresh(); }, SCREEN_REMOTE_REFRESH_INTERVAL_MS);
    }
  }
  function settleCapture() {
    if (requiredCapture && state.opened && state.session && !state.connecting && !state.controlChanging && !capture && !candidate) {
      requiredCapture = false;
      void refresh();
    } else if (!requiredCapture) schedule();
  }
  function requireFreshCapture() {
    requiredCapture = true;
    stopTimer();
    settleCapture();
  }
  function isCurrent(current: number, session: ScreenRemoteSession, epoch: number) {
    return current === generation && session === state.session && epoch === frameEpoch && state.opened;
  }
  function validInputGeneration(value: number) { return Number.isSafeInteger(value) && value >= 0; }
  function validControlMode(value: unknown): value is ScreenRemoteControlMode { return value === "full" || value === "wake_only"; }

  async function refresh(): Promise<void> {
    const session = state.session;
    const current = generation;
    const epoch = frameEpoch;
    if (!state.opened || !session || state.connecting) return;
    // 普通重复刷新不排队；输入完成的必要新帧用一个布尔需求合并。
    if (capture) return capture.promise;
    if (candidate) return;
    stopTimer(); state.capturing = true; state.error = "";
    const startedAt = performance.now();
    const request = (async () => {
      try {
        const frame = await adapter.capture(session.projectId, session.sessionId);
        if (!isCurrent(current, session, epoch)) return;
        if (frame.sessionId !== session.sessionId) throw new Error("屏幕会话已变化，请重新连接");
        if (!Number.isInteger(frame.width) || !Number.isInteger(frame.height) || frame.width <= 0 || frame.height <= 0
          || !validInputGeneration(frame.inputGeneration) || typeof frame.inputReady !== "boolean") {
          throw new Error("屏幕响应无效，请重新读取");
        }
        if (state.controlActive && frame.inputGeneration < minimumInputGeneration) return;
        if (state.controlActive) minimumInputGeneration = Math.max(minimumInputGeneration, frame.inputGeneration);
        let acceptedEpoch = epoch;
        if (state.controlActive && frame.controlActive === false) {
          // 只读观察立即撤掉输入权限，但保留当前在途操作的最终回执。
          // 主动停止、换屏和关窗仍推进controlGeneration并拒绝其迟到回执。
          if (!state.acting) controlGeneration += 1;
          state.controlActive = false; stopIdleTimer();
          boundary(); acceptedEpoch = frameEpoch; state.waitingForInputFrame = false;
          state.operationMessage = "辅助操作已结束，请重新开启后操作";
        }
        const url = urls.create(frame);
        const info: ScreenRemoteFrameInfo = {
          sessionId: frame.sessionId, frameId: frame.frameId, width: frame.width, height: frame.height,
          capturedAt: frame.capturedAt, elapsedMs: frame.elapsedMs, controlActive: frame.controlActive,
          inputReady: frame.inputReady, inputGeneration: frame.inputGeneration,
        };
        candidate = { frame: info, url, generation: current, epoch: acceptedEpoch, startedAt, session };
        state.pendingFrame = info; state.pendingImageUrl = url; state.decoding = true;
        decodeTimer = setTimeout(() => { if (candidate?.url === url) imageFailed(url); }, 10_000);
        // 已显示图及其inputReady不在采集返回时切换，等待候选图实际load。
      } catch (error) {
        if (isCurrent(current, session, epoch)) {
          state.error = error instanceof Error ? error.message : "读取屏幕失败，请重试";
          state.stale = Boolean(state.frame); state.autoRefresh = false;
        }
      } finally {
        if (current === generation && session === state.session) state.capturing = false;
      }
    })();
    capture = { sessionId: session.sessionId, promise: request };
    await request;
    if (capture?.promise === request) capture = undefined;
    settleCapture();
  }

  async function open(target: ScreenRemoteTarget): Promise<void> {
    const current = ++generation;
    const previousOpening = opening;
    const previousSession = state.session;
    requiredCapture = false; stopTimer(); clearFrame(); resetControl();
    state.session = null; state.opened = true; state.connecting = true;
    state.capturing = false; state.autoRefresh = false; state.error = "";
    await release(previousSession);
    if (previousOpening) await previousOpening;
    if (current !== generation || !state.opened) return;
    const request = (async () => {
      try {
        const session = await adapter.open(target.projectId, target.screenId);
        if (current !== generation || !state.opened) { await release(session); return; }
        if (session.projectId !== target.projectId || session.screenId !== target.screenId) {
          await release(session); throw new Error("屏幕会话不属于当前设备，请重新连接");
        }
        state.session = session; state.connecting = false;
        requiredCapture = true;
        if (capture) await capture.promise;
        if (current === generation && state.opened) {
          requiredCapture = false;
          await refresh();
        }
      } catch (error) {
        if (current === generation && state.opened) state.error = error instanceof Error ? error.message : "连接屏幕失败，请重试";
      } finally { if (current === generation) state.connecting = false; }
    })();
    opening = request;
    await request;
    if (opening === request) opening = undefined;
  }
  async function close() {
    generation += 1; requiredCapture = false; stopTimer(); resetControl();
    const session = state.session;
    state.opened = false; state.connecting = false; state.capturing = false;
    state.autoRefresh = false; state.session = null; state.error = ""; clearFrame();
    await release(session);
  }
  function setAutoRefresh(value: boolean) {
    state.autoRefresh = value && state.opened && Boolean(state.session) && !state.error;
    if (!state.autoRefresh) stopTimer(); else schedule();
  }
  function frameForImage(url: string) {
    return candidate?.url === url ? candidate.frame : state.imageUrl === url ? state.frame : null;
  }
  function imageFailed(url: string) {
    if (!url || !state.opened) return;
    if (candidate?.url === url) {
      clearCandidate(); state.stale = Boolean(state.frame);
    } else if (url === state.imageUrl) clearFrame();
    else return;
    stopTimer(); state.autoRefresh = false; state.error = "屏幕图片无法显示，请重新读取";
  }
  function imageLoaded(url: string) {
    const next = candidate;
    if (!next || next.url !== url) return;
    if (!isCurrent(next.generation, next.session, next.epoch)
      || (state.controlActive && next.frame.inputGeneration < minimumInputGeneration)) {
      clearCandidate(); settleCapture(); return;
    }
    const previousUrl = state.imageUrl;
    if (decodeTimer !== undefined) clearTimeout(decodeTimer);
    decodeTimer = undefined;
    candidate = undefined;
    state.frame = next.frame; state.imageUrl = next.url;
    state.pendingFrame = null; state.pendingImageUrl = ""; state.decoding = false;
    displayedEpoch = next.epoch; state.imageLoaded = true; state.stale = false; state.error = "";
    setFrameDeadline(next.startedAt);
    state.waitingForInputFrame = state.controlActive && !next.frame.inputReady;
    // Vue按frameId复用已load的候选img；旧图在DOM提交后才释放，不出现空图间隙。
    if (previousUrl && previousUrl !== next.url) void nextTick(() => urls.revoke(previousUrl));
    settleCapture();
  }
  function hasOperableFrame() {
    return state.opened && state.controlActive && Boolean(state.session && state.frame)
      && state.imageLoaded && state.frame?.inputReady === true
      && state.frame.inputGeneration >= minimumInputGeneration && displayedEpoch === frameEpoch
      && !state.frameExpired && performance.now() < frameDeadline
      && !state.stale && !state.error && !state.connecting && !state.acting && !state.controlChanging;
  }
  function canAct() { return state.controlMode === "full" && hasOperableFrame(); }
  function canWake() { return validControlMode(state.controlMode) && hasOperableFrame(); }
  function explainUnavailableAction() {
    if (!state.controlActive) return;
    state.operationMessage = state.acting
      ? "正在处理上一次操作，本次点击未发送，请稍后再点"
      : state.waitingForInputFrame
        ? "正在等待操作后的新画面，本次点击未发送，请稍后再点"
        : state.controlMode === "wake_only" && canWake()
          ? "屏幕已休眠，请点击唤醒"
        : !state.imageLoaded || state.controlChanging
          ? "画面尚未就绪，本次点击未发送，请稍后再点"
          : "画面已过期，本次点击未发送，请刷新后再点";
    state.operationUnconfirmed = true;
  }
  async function beginControl() {
    const session = state.session, current = generation;
    if (!state.opened || !session || state.controlActive || state.connecting || state.controlChanging || state.acting) return;
    const control = ++controlGeneration;
    state.controlChanging = true; boundary();
    state.operationMessage = ""; state.operationUnconfirmed = false;
    try {
      const capability = await adapter.beginControl(session.projectId, session.sessionId);
      if (current !== generation || control !== controlGeneration || session !== state.session) { await releaseControl(session); return; }
      if (!Number.isFinite(capability.frameMaxAgeMs) || capability.frameMaxAgeMs <= 500
        || !Number.isFinite(capability.idleTimeoutMs) || capability.idleTimeoutMs <= 0
        || !Number.isInteger(capability.maxTextLength) || capability.maxTextLength < 1
        || !["insert", "replace"].includes(capability.textInputBehavior)
        || !validControlMode(capability.controlMode)
        || !validInputGeneration(capability.inputGeneration)) {
        await releaseControl(session); throw new Error("辅助操作能力无效，请重新连接");
      }
      frameMaxAgeMs = Math.min(30_000, capability.frameMaxAgeMs);
      idleTimeoutMs = Math.min(300_000, capability.idleTimeoutMs);
      state.maxTextLength = Math.min(SCREEN_REMOTE_TEXT_LIMIT, capability.maxTextLength);
      state.textInputBehavior = capability.textInputBehavior; minimumInputGeneration = capability.inputGeneration;
      state.controlMode = capability.controlMode;
      state.controlActive = true;
      state.operationMessage = state.controlMode === "wake_only" ? "屏幕已休眠，请点击唤醒" : "已开启辅助操作，登录结果以屏端页面为准";
      touchControl();
    } catch (error) {
      if (current === generation && control === controlGeneration) {
        state.operationMessage = error instanceof Error ? error.message : "辅助操作未能开启";
        state.operationUnconfirmed = true;
      }
    } finally {
      if (current === generation && control === controlGeneration) {
        state.controlChanging = false; boundary(); requireFreshCapture();
      }
    }
  }
  async function endControl(message = "已停止辅助操作") {
    const session = state.session;
    if (!session || !state.opened || (!state.controlActive && !state.acting)) return;
    const current = generation, control = ++controlGeneration;
    stopIdleTimer(); state.controlActive = false; state.controlChanging = true; boundary();
    state.operationMessage = message; state.operationUnconfirmed = false;
    try { await adapter.endControl(session.projectId, session.sessionId); }
    catch {
      if (current === generation && control === controlGeneration) {
        state.operationMessage = "已停止发送操作，连接状态待确认"; state.operationUnconfirmed = true;
      }
    } finally {
      if (current === generation && control === controlGeneration) {
        state.controlChanging = false; boundary(); state.waitingForInputFrame = false; requireFreshCapture();
      }
    }
  }
  async function perform(action: ScreenRemoteAction): Promise<boolean> {
    const session = state.session, frame = state.frame;
    const waking = action.kind === "key" && action.key === "wake_up";
    if (!session || !frame || !(waking ? canWake() : canAct())) return false;
    if (action.kind === "text" && screenRemoteTextError(action.text, state.maxTextLength)) {
      state.operationMessage = screenRemoteTextError(action.text, state.maxTextLength); state.operationUnconfirmed = true; return false;
    }
    if (action.kind === "key" && (!["backspace", "clear_text", "back", "wake_up"].includes(action.key)
      || (action.key === "backspace" && state.textInputBehavior !== "insert")
      || (action.key === "clear_text" && state.textInputBehavior !== "replace"))) return false;
    if (action.kind === "tap" && (!Number.isInteger(action.x) || !Number.isInteger(action.y)
      || action.x < 0 || action.y < 0 || action.x >= frame.width || action.y >= frame.height)) return false;
    const current = generation, control = controlGeneration;
    state.acting = true; boundary();
    schedule();
    state.operationMessage = ""; state.operationUnconfirmed = false;
    let sent = false;
    try {
      const result = await adapter.action(session.projectId, session.sessionId, { frameId: frame.frameId, action });
      if (current !== generation || session !== state.session || control !== controlGeneration) return false;
      if (!validInputGeneration(result.inputGeneration) || !validControlMode(result.controlMode)) throw new Error("辅助操作响应无效");
      minimumInputGeneration = Math.max(minimumInputGeneration, result.inputGeneration);
      state.controlMode = result.controlMode;
      sent = result.status === "sent";
      state.operationMessage = sent
        ? waking && result.controlMode === "wake_only"
          ? typeof result.message === "string" && result.message.trim()
            ? result.message
            : "唤醒已发送，暂未确认小新可操作，请稍后重新开启辅助操作"
          : "操作已发送，请查看屏端结果"
        : "操作结果未确认，请查看屏端；本次内容不会自动重发";
      state.operationUnconfirmed = !sent || (waking && result.controlMode === "wake_only");
      if (!sent || !result.controlActive || (waking && result.controlMode === "wake_only")) {
        state.controlActive = false; stopIdleTimer(); await releaseControl(session);
      } else touchControl();
    } catch {
      if (current === generation && session === state.session && control === controlGeneration) {
        state.controlActive = false; stopIdleTimer();
        state.operationMessage = "操作结果未确认，请查看屏端；本次内容不会自动重发";
        state.operationUnconfirmed = true; await releaseControl(session);
      }
    } finally {
      if (current === generation && session === state.session) {
        state.acting = false; boundary(); requireFreshCapture();
      }
    }
    return sent;
  }
  return { state, open, close, refresh, setAutoRefresh, imageFailed, imageLoaded, frameForImage, canAct, canWake,
    explainUnavailableAction, beginControl, endControl, perform };
}
