import { beforeEach, describe, expect, it, vi } from "vitest";
import { RealScreenRemoteAdapter } from "./realScreenRemoteAdapter";

const { mockedInvoke } = vi.hoisted(() => ({ mockedInvoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mockedInvoke }));

describe("单屏查看真实适配器", () => {
  beforeEach(() => { mockedInvoke.mockReset(); });
  it("每个调用都携带当前项目与单屏会话，不经普通任务计划", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    const adapter = new RealScreenRemoteAdapter();
    await adapter.open("project", "screen");
    await adapter.capture("project", "session");
    await adapter.close("project", "session");
    expect(mockedInvoke.mock.calls).toEqual([
      ["screen_remote_open", { localProjectId: "project", screenId: "screen" }],
      ["screen_remote_capture", { localProjectId: "project", sessionId: "session" }],
      ["screen_remote_close", { localProjectId: "project", sessionId: "session" }],
    ]);
  });
  it("只展示受控命令错误，不把底层对象直接显示给用户", async () => {
    mockedInvoke.mockRejectedValue({ code: "IO_ERROR", params: { summary: "屏幕连接已断开" } });
    await expect(new RealScreenRemoteAdapter().capture("p", "s")).rejects.toThrow("屏幕连接已断开");
  });
  it("辅助操作使用独立短会话和帧编号，不走普通任务执行命令", async () => {
    mockedInvoke.mockResolvedValue(undefined);
    const adapter = new RealScreenRemoteAdapter();
    await adapter.beginControl("p", "s");
    await adapter.action("p", "s", { frameId: "f", action: { kind: "tap", x: 4, y: 8 } });
    await adapter.endControl("p", "s");
    expect(mockedInvoke.mock.calls).toEqual([
      ["screen_remote_begin_control", { localProjectId: "p", sessionId: "s" }],
      ["screen_remote_action", { localProjectId: "p", sessionId: "s", input: { frameId: "f", action: { kind: "tap", x: 4, y: 8 } } }],
      ["screen_remote_end_control", { localProjectId: "p", sessionId: "s" }],
    ]);
  });
  it("输入错误不传播可能携带密码的消息或cause", async () => {
    const secret = "Fixture-Password%$!";
    for (const cause of [new Error(`底层输入错误：${secret}`), { code: "INVALID_INPUT", params: { summary: secret } }]) {
      mockedInvoke.mockRejectedValueOnce(cause);
      const error = await new RealScreenRemoteAdapter().action("p", "s", { frameId: "f", action: { kind: "text", text: secret } })
        .catch((failure: unknown) => failure);
      expect(error).toBeInstanceOf(Error);
      expect((error as Error).message).toContain("操作结果未确认");
      expect((error as Error).message).not.toContain(secret);
      expect(error).not.toHaveProperty("cause");
    }
  });
});
