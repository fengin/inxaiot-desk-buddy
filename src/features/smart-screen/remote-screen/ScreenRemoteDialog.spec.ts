import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NButton, NCheckbox, NInput } from "naive-ui";
import { configureScreenRemoteAdapter, UnavailableScreenRemoteAdapter } from "@/shared/api/screenRemoteAdapter";
import type { ScreenRemoteAdapter } from "@/shared/api/screenRemoteAdapter";
import type { ScreenRemoteControlMode, ScreenRemoteFrame } from "@/shared/model/screenRemote";
import ScreenRemoteDialog from "./ScreenRemoteDialog.vue";

function fixture(textInputBehavior: "insert" | "replace" = "insert", controlMode: ScreenRemoteControlMode = "full") {
  let controlling = false;
  let frameId = 0;
  let urlId = 0;
  let inputGeneration = 0;
  const adapter = {
    open: vi.fn(async (projectId: string, screenId: string) => ({ projectId, screenId, sessionId: screenId, name: "测试屏", ip: "192.0.2.10" })),
    capture: vi.fn(async (_projectId: string, sessionId: string): Promise<ScreenRemoteFrame> => ({ sessionId, frameId: `f${++frameId}`, width: 800, height: 480, capturedAt: "2026-10-26T03:20:00Z", elapsedMs: 100, pngBase64: "iVBORw0KGgo=", controlActive: controlling, inputReady: controlling, inputGeneration })),
    close: vi.fn(async () => {}),
    beginControl: vi.fn(async () => { controlling = true; inputGeneration += 1; return { taskId: "t", idleTimeoutMs: 300000, frameMaxAgeMs: 30000, maxTextLength: 128, textInputBehavior, inputGeneration, controlMode }; }),
    endControl: vi.fn(async () => { controlling = false; inputGeneration += 1; }),
    action: vi.fn<ScreenRemoteAdapter["action"]>(async () => { inputGeneration += 2; return { status: "sent", message: "操作已发送", controlActive: true, inputGeneration, controlMode }; }),
  };
  configureScreenRemoteAdapter(adapter);
  const create = vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:screen-test-${++urlId}`);
  const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
  const wrapper = mount(ScreenRemoteDialog, { props: { show: true, projectId: "p1", screenId: "s1", name: "测试屏", ip: "192.0.2.10" }, attachTo: document.body, global: { stubs: { teleport: true } } });
  return { adapter, wrapper, create, revoke, setControlMode(value: ScreenRemoteControlMode) { controlMode = value; } };
}

async function loadImage(wrapper: ReturnType<typeof fixture>["wrapper"]) {
  const pending = wrapper.find<HTMLImageElement>(".screen-remote-image-pending");
  const image = pending.exists() ? pending : wrapper.get<HTMLImageElement>(".screen-remote-image-current");
  Object.defineProperty(image.element, "naturalWidth", { configurable: true, value: 800 });
  Object.defineProperty(image.element, "naturalHeight", { configurable: true, value: 480 });
  await image.trigger("load");
  await flushPromises();
  return wrapper.get<HTMLImageElement>(".screen-remote-image-current");
}
async function enable(wrapper: ReturnType<typeof fixture>["wrapper"]) {
  await wrapper.findAllComponents(NButton).find(item => item.text() === "开启辅助操作")!.trigger("click");
  await flushPromises();
}

describe("查看屏幕窗口", () => {
  afterEach(() => { vi.restoreAllMocks(); configureScreenRemoteAdapter(new UnavailableScreenRemoteAdapter()); });
  it("立即展示单屏画面及时间，默认只查看且不自动刷新", async () => {
    const { wrapper, adapter, revoke } = fixture();
    try {
      await flushPromises();
      await loadImage(wrapper);
      expect(adapter.open).toHaveBeenCalledWith("p1", "s1");
      expect(wrapper.get("img").attributes("src")).toBe("blob:screen-test-1");
      expect(wrapper.text()).toContain("192.0.2.10");
      expect(wrapper.text()).toContain("2026-10-26");
      expect(wrapper.getComponent(NCheckbox).props("checked")).toBe(false);
      expect(wrapper.getComponent(NCheckbox).text()).toBe("自动刷新（1秒）");
      expect(wrapper.text()).not.toContain("画面不会自动保存");
      expect(wrapper.text()).not.toContain("800 × 480");
      expect(wrapper.get(".screen-remote-footer").element.contains(wrapper.get(".screen-remote-feedback").element)).toBe(true);
      expect(wrapper.findAllComponents(NButton).map(item => item.text())).toEqual(["刷新画面", "开启辅助操作", "关闭"]);
      expect(wrapper.findComponent(NInput).exists()).toBe(false);
      expect(adapter.beginControl).not.toHaveBeenCalled();
      await wrapper.findAllComponents(NButton).find(item => item.text() === "关闭")!.trigger("click");
      await flushPromises();
      expect(wrapper.emitted("close")).toHaveLength(1);
      expect(revoke).toHaveBeenCalledWith("blob:screen-test-1");
      expect(adapter.close).toHaveBeenCalledWith("p1", "s1");
    } finally { wrapper.unmount(); }
  });
  it("刷新失败明确标识旧画面，关闭或目标切换会释放原会话", async () => {
    const { wrapper, adapter } = fixture();
    try {
      await flushPromises();
      await loadImage(wrapper);
      adapter.capture.mockRejectedValueOnce(new Error("连接失败"));
      await wrapper.findAllComponents(NButton).find(item => item.text() === "刷新画面")!.trigger("click");
      await flushPromises();
      expect(wrapper.get('[role="alert"]').text()).toContain("当前显示上一次画面");
      expect(wrapper.get(".screen-remote-viewport").classes()).toContain("is-stale");
      await wrapper.setProps({ projectId: "p2", screenId: "s2" });
      await flushPromises();
      expect(adapter.close).toHaveBeenCalledWith("p1", "s1");
      expect(adapter.open).toHaveBeenLastCalledWith("p2", "s2");
      expect(wrapper.find('[role="alert"]').exists()).toBe(false);
    } finally { wrapper.unmount(); }
    await flushPromises();
    expect(adapter.close).toHaveBeenCalledWith("p2", "s2");
  });
  it("只在显式辅助模式且新图加载后点按，忽略contain留白并绑定新帧", async () => {
    const { wrapper, adapter } = fixture();
    try {
      await flushPromises();
      let image = await loadImage(wrapper);
      await image.trigger("click", { clientX: 500, clientY: 500, button: 0 });
      expect(adapter.action).not.toHaveBeenCalled();
      await enable(wrapper);
      image = wrapper.get<HTMLImageElement>("img");
      vi.spyOn(image.element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 1000, 1000));
      await image.trigger("click", { clientX: 500, clientY: 500, button: 0 });
      expect(adapter.action).not.toHaveBeenCalled();
      image = await loadImage(wrapper);
      vi.spyOn(image.element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 1000, 1000));
      await image.trigger("click", { clientX: 500, clientY: 100, button: 0 });
      expect(adapter.action).not.toHaveBeenCalled();
      await image.trigger("click", { clientX: 500, clientY: 500, button: 0 });
      await flushPromises();
      await loadImage(wrapper);
      expect(adapter.action).toHaveBeenCalledExactlyOnceWith("p1", "s1", { frameId: "f2", action: { kind: "tap", x: 400, y: 240 } });
      expect(wrapper.text()).toContain("操作已发送，请查看屏端结果");
      expect(wrapper.text()).not.toContain("登录成功");
      const back = wrapper.findAllComponents(NButton).find(item => item.text() === "返回")!;
      expect(back.props("disabled")).toBe(false);
    } finally { wrapper.unmount(); }
  });
  it("输入默认掩码，发送立即清空，失败不回填或泄露密码", async () => {
    const { wrapper, adapter } = fixture();
    let reject!: (cause: unknown) => void;
    const pending = new Promise<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>((_resolve, failed) => { reject = failed; });
    try {
      await flushPromises();
      await enable(wrapper);
      await loadImage(wrapper);
      const input = wrapper.getComponent(NInput);
      expect(input.props("type")).toBe("password");
      input.vm.$emit("update:value", "Example-Private%$!");
      await flushPromises();
      adapter.action.mockReturnValueOnce(pending);
      await wrapper.findAllComponents(NButton).find(item => item.text() === "发送输入")!.trigger("click");
      expect(input.props("value")).toBe("");
      expect(adapter.action).toHaveBeenCalledExactlyOnceWith("p1", "s1", { frameId: "f2", action: { kind: "text", text: "Example-Private%$!" } });
      reject(new Error("Example-Private%$!不可展示"));
      await flushPromises();
      expect(wrapper.text()).toContain("操作结果未确认");
      expect(wrapper.text()).not.toContain("Example-Private");
      expect(wrapper.findComponent(NInput).exists()).toBe(false);
      expect(adapter.action).toHaveBeenCalledTimes(1);
    } finally { wrapper.unmount(); }
  });
  it("不支持的文字禁止发送，切项目清空输入且回到查看模式", async () => {
    const { wrapper, adapter } = fixture();
    try {
      await flushPromises();
      await enable(wrapper);
      await loadImage(wrapper);
      wrapper.getComponent(NInput).vm.$emit("update:value", "中文🙂");
      await flushPromises();
      expect(wrapper.text()).toContain("暂不支持中文");
      expect(wrapper.findAllComponents(NButton).find(item => item.text() === "发送输入")!.props("disabled")).toBe(true);
      expect(adapter.action).not.toHaveBeenCalled();
      await wrapper.setProps({ projectId: "p2", screenId: "s2" });
      await flushPromises();
      expect(wrapper.findComponent(NInput).exists()).toBe(false);
      await enable(wrapper);
      await loadImage(wrapper);
      expect(wrapper.getComponent(NInput).props("value")).toBe("");
      expect(adapter.open).toHaveBeenLastCalledWith("p2", "s2");
    } finally { wrapper.unmount(); }
  });
  it("仅名称更新或相同值重新传入不重新连接当前会话", async () => {
    const { wrapper, adapter } = fixture();
    try {
      await flushPromises();
      await wrapper.setProps({ name: "资料刷新后的名称", show: true, projectId: "p1", screenId: "s1", ip: "192.0.2.10" });
      await flushPromises();
      expect(adapter.open).toHaveBeenCalledTimes(1);
      expect(adapter.capture).toHaveBeenCalledTimes(1);
      expect(adapter.close).not.toHaveBeenCalled();
      expect(wrapper.text()).toContain("连接IP：192.0.2.10");
    } finally { wrapper.unmount(); }
  });
  it("按输入能力提示插入或替换，仅提供相应退格或清空按钮且不提供回车", async () => {
    for (const behavior of ["insert", "replace"] as const) {
      const { wrapper, adapter } = fixture(behavior);
      try {
        await flushPromises();
        await enable(wrapper);
        await loadImage(wrapper);
        expect(wrapper.get(".screen-remote-input-hint").text()).toContain(behavior === "insert" ? "内容插入当前光标处，屏端原内容请先清除" : "替换屏端当前输入框的全部内容");
        const labels = wrapper.findAllComponents(NButton).map(button => button.text());
        expect(labels).not.toContain("回车");
        expect(labels.includes("退格")).toBe(behavior === "insert");
        expect(labels.includes("清空输入框")).toBe(behavior === "replace");
        if (behavior === "replace") {
          await wrapper.findAllComponents(NButton).find(button => button.text() === "清空输入框")!.trigger("click");
          await flushPromises();
          expect(adapter.action).toHaveBeenCalledExactlyOnceWith("p1", "s1", { frameId: "f2", action: { kind: "key", key: "clear_text" } });
        }
      } finally { wrapper.unmount(); }
    }
  });
  it("休眠只开放唤醒，回执和操作后新图确认后才开放普通操作，布局槽不变", async () => {
    const { wrapper, adapter, setControlMode } = fixture("insert", "wake_only");
    try {
      await flushPromises();
      const viewport = wrapper.get(".screen-remote-viewport").element;
      const controls = wrapper.get(".screen-remote-assist-slot").element;
      await enable(wrapper);
      const image = await loadImage(wrapper);
      expect(wrapper.get(".screen-remote-input-hint").text()).toBe("屏幕已休眠，请点击唤醒");
      expect(wrapper.getComponent(NInput).props("disabled")).toBe(true);
      for (const label of ["发送输入", "退格", "返回"]) expect(wrapper.findAllComponents(NButton).find(button => button.text() === label)!.props("disabled")).toBe(true);
      const wake = wrapper.findAllComponents(NButton).find(button => button.text() === "唤醒")!;
      expect(wake.props("disabled")).toBe(false);
      vi.spyOn(image.element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 480));
      await image.trigger("click", { clientX: 400, clientY: 240, button: 0 });
      expect(adapter.action).not.toHaveBeenCalled();
      setControlMode("full");
      await wake.trigger("click"); await flushPromises();
      expect(adapter.action).toHaveBeenCalledExactlyOnceWith("p1", "s1", { frameId: "f2", action: { kind: "key", key: "wake_up" } });
      expect(wrapper.findAllComponents(NButton).find(button => button.text() === "返回")!.props("disabled")).toBe(true);
      await loadImage(wrapper);
      expect(wrapper.findAllComponents(NButton).find(button => button.text() === "返回")!.props("disabled")).toBe(false);
      expect(wrapper.getComponent(NInput).props("disabled")).toBe(false);
      expect(wrapper.get(".screen-remote-viewport").element).toBe(viewport);
      expect(wrapper.get(".screen-remote-assist-slot").element).toBe(controls);
    } finally { wrapper.unmount(); }
  });
  it("唤醒结果未确认小新前台时结束辅助并给出重新开启提示", async () => {
    const { wrapper, adapter } = fixture("replace", "wake_only");
    try {
      await flushPromises(); await enable(wrapper); await loadImage(wrapper);
      expect(wrapper.findAllComponents(NButton).find(button => button.text() === "清空输入框")!.props("disabled")).toBe(true);
      adapter.action.mockResolvedValueOnce({ status: "sent", message: "唤醒已发送，请待画面恢复后重新开启辅助操作", controlActive: false, inputGeneration: 3, controlMode: "wake_only" });
      await wrapper.findAllComponents(NButton).find(button => button.text() === "唤醒")!.trigger("click");
      await flushPromises(); await loadImage(wrapper);
      expect(wrapper.text()).toContain("唤醒已发送，请待画面恢复后重新开启辅助操作");
      expect(wrapper.findComponent(NInput).exists()).toBe(false);
      expect(wrapper.findAllComponents(NButton).map(button => button.text())).not.toContain("唤醒");
      expect(adapter.action).toHaveBeenCalledTimes(1);
    } finally { wrapper.unmount(); }
  });
  it("后台截图不禁用当前画面，提交可见帧且不接纳跨操作的迟到画面", async () => {
    const { wrapper, adapter } = fixture();
    let resolve!: (frame: Awaited<ReturnType<ScreenRemoteAdapter["capture"]>>) => void;
    const pending = new Promise<Awaited<ReturnType<ScreenRemoteAdapter["capture"]>>>((done) => { resolve = done; });
    try {
      await flushPromises();
      await enable(wrapper);
      let image = await loadImage(wrapper);
      vi.spyOn(image.element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 480));
      await image.trigger("click", { clientX: 400, clientY: 240, button: 0 });
      await flushPromises();
      image = await loadImage(wrapper);
      expect(wrapper.text()).toContain("操作已发送，请查看屏端结果");
      vi.spyOn(image.element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 480));
      const displayed = image.element;
      adapter.capture.mockReturnValueOnce(pending);
      await wrapper.findAllComponents(NButton).find(button => button.text() === "刷新画面")!.trigger("click");
      expect(wrapper.get(".screen-remote-image-current").element).toBe(displayed);
      expect(wrapper.findAllComponents(NButton).find(button => button.text() === "返回")!.props("disabled")).toBe(false);
      await image.trigger("click", { clientX: 400, clientY: 240, button: 0 });
      await flushPromises();
      expect(adapter.action).toHaveBeenCalledTimes(2);
      expect(adapter.action).toHaveBeenLastCalledWith("p1", "s1", { frameId: "f3", action: { kind: "tap", x: 400, y: 240 } });
      expect(wrapper.findAllComponents(NButton).find(button => button.text() === "返回")!.props("disabled")).toBe(true);
      resolve({ sessionId: "s1", frameId: "old-background", width: 800, height: 480, capturedAt: "2026-10-26T03:21:00Z", elapsedMs: 100, pngBase64: "iVBORw0KGgo=", controlActive: true, inputReady: true, inputGeneration: 3 });
      await flushPromises();
      expect(wrapper.get(".screen-remote-image-current").element).toBe(displayed);
      expect(wrapper.findAll("img")).toHaveLength(2);
      await loadImage(wrapper);
      expect(wrapper.get(".screen-remote-image-current").element).not.toBe(displayed);
      expect(wrapper.findAll("img")).toHaveLength(1);
      expect(adapter.action).toHaveBeenCalledTimes(2);
      expect(wrapper.findAllComponents(NButton).find(button => button.text() === "返回")!.props("disabled")).toBe(false);
    } finally { wrapper.unmount(); }
  });
  it("候选图加载前保留当前DOM与输入能力，加载后复用候选DOM且释放旧图", async () => {
    const { wrapper, adapter, revoke } = fixture();
    try {
      await flushPromises();
      await enable(wrapper);
      const current = await loadImage(wrapper);
      const previousUrl = current.attributes("src");
      await wrapper.findAllComponents(NButton).find(button => button.text() === "刷新画面")!.trigger("click");
      await flushPromises();
      const pending = wrapper.get(".screen-remote-image-pending");
      expect(pending.attributes("aria-hidden")).toBe("true");
      expect(wrapper.get(".screen-remote-image-current").element).toBe(current.element);
      expect(revoke).not.toHaveBeenCalledWith(previousUrl);
      expect(wrapper.findAllComponents(NButton).find(button => button.text() === "返回")!.props("disabled")).toBe(false);
      const next = await loadImage(wrapper);
      expect(next.element).toBe(pending.element);
      expect(next.attributes("aria-hidden")).toBe("false");
      expect(wrapper.find(".screen-remote-image-pending").exists()).toBe(false);
      expect(revoke).toHaveBeenCalledWith(previousUrl);
      expect(adapter.action).not.toHaveBeenCalled();
    } finally { wrapper.unmount(); }
  });
  it("查看、辅助、发送和刷新保持相同布局槽，关闭辅助后没有隐藏可聚焦输入", async () => {
    const { wrapper, adapter } = fixture();
    let finishAction!: (result: Awaited<ReturnType<ScreenRemoteAdapter["action"]>>) => void;
    let finishCapture!: (result: ScreenRemoteFrame) => void;
    const action = new Promise<Awaited<ReturnType<ScreenRemoteAdapter["action"]>>>((resolve) => { finishAction = resolve; });
    const capture = new Promise<ScreenRemoteFrame>((resolve) => { finishCapture = resolve; });
    try {
      await flushPromises();
      await loadImage(wrapper);
      const selectors = [".screen-remote-toolbar", ".screen-remote-viewport", ".screen-remote-assist-slot", ".screen-remote-feedback", ".screen-remote-footer"];
      const regions = selectors.map(selector => wrapper.get(selector).element);
      const sameRegions = () => selectors.forEach((selector, index) => expect(wrapper.get(selector).element).toBe(regions[index]));
      expect(wrapper.get(".screen-remote-assist-slot").findAll('input,textarea,button,[tabindex="0"]')).toHaveLength(0);
      expect(wrapper.get(".screen-remote-operation-message").attributes("tabindex")).toBe("-1");
      await enable(wrapper);
      sameRegions();
      const image = await loadImage(wrapper);
      vi.spyOn(image.element, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 480));
      adapter.action.mockReturnValueOnce(action);
      adapter.capture.mockReturnValueOnce(capture);
      await image.trigger("click", { clientX: 400, clientY: 240, button: 0 });
      sameRegions();
      expect(wrapper.get(".screen-remote-feedback").text()).toContain("正在发送操作");
      expect(wrapper.getComponent(NInput).props("disabled")).toBe(true);
      finishAction({ status: "sent", message: "操作已发送", controlActive: true, inputGeneration: 3, controlMode: "full" });
      await flushPromises();
      sameRegions();
      expect(wrapper.get(".screen-remote-feedback").text()).toContain("正在更新屏幕画面");
      finishCapture({ sessionId: "s1", frameId: "after-action", width: 800, height: 480,
        capturedAt: "2026-10-26T03:22:00Z", elapsedMs: 100, pngBase64: "iVBORw0KGgo=", controlActive: true, inputReady: true, inputGeneration: 3 });
      await flushPromises();
      await loadImage(wrapper);
      sameRegions();
      expect(wrapper.get(".screen-remote-feedback").text()).toContain("操作已发送，请查看屏端结果");
      await wrapper.findAllComponents(NButton).find(button => button.text() === "停止辅助操作")!.trigger("click");
      await flushPromises();
      sameRegions();
      expect(wrapper.findComponent(NInput).exists()).toBe(false);
      expect(wrapper.get(".screen-remote-assist-slot").findAll('input,textarea,button,[tabindex="0"]')).toHaveLength(0);
      expect(wrapper.get(".screen-remote-assist-idle").text()).toContain("当前仅查看屏幕");
    } finally { wrapper.unmount(); }
  });
  it("错误和重新读取复用反馈槽，保留完整长错误及可访问的重连操作", async () => {
    const { wrapper, adapter } = fixture();
    try {
      await flushPromises();
      await loadImage(wrapper);
      const viewport = wrapper.get(".screen-remote-viewport").element;
      const feedback = wrapper.get(".screen-remote-feedback").element;
      const assistSlot = wrapper.get(".screen-remote-assist-slot").element;
      const message = "连接异常，请核对设备网络。".repeat(30);
      adapter.capture.mockRejectedValueOnce(new Error(message));
      await wrapper.findAllComponents(NButton).find(button => button.text() === "刷新画面")!.trigger("click");
      await flushPromises();
      expect(wrapper.get(".screen-remote-viewport").element).toBe(viewport);
      expect(wrapper.get(".screen-remote-feedback").element).toBe(feedback);
      expect(wrapper.get(".screen-remote-assist-slot").element).toBe(assistSlot);
      const error = wrapper.get('.screen-remote-feedback [role="alert"]');
      expect(error.text()).toContain(message);
      expect(error.attributes("tabindex")).toBe("0");
      const reconnect = wrapper.findAllComponents(NButton).find(button => button.text() === "重新连接");
      expect(reconnect).toBeDefined();
      expect(feedback.contains(reconnect!.element)).toBe(true);
      expect(wrapper.get(".screen-remote-toolbar").text()).not.toContain("重新连接");
      await wrapper.findAllComponents(NButton).find(button => button.text() === "刷新画面")!.trigger("click");
      await flushPromises();
      expect(wrapper.find('[role="alert"]').exists()).toBe(false);
      expect(wrapper.get(".screen-remote-feedback").element).toBe(feedback);
      expect(wrapper.get(".screen-remote-viewport").element).toBe(viewport);
    } finally { wrapper.unmount(); }
  });
});
