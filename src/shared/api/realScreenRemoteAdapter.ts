import { invoke } from "@tauri-apps/api/core";
import type { ScreenRemoteAdapter } from "./screenRemoteAdapter";
import type { ScreenRemoteActionInput, ScreenRemoteActionResult, ScreenRemoteControl, ScreenRemoteFrame, ScreenRemoteSession } from "@/shared/model/screenRemote";
import { commandErrorText } from "./errors";

async function call<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try { return await invoke<T>(command, args); }
  catch (error) {
    const message = command === "screen_remote_action"
      ? "操作结果未确认，请查看屏端画面；不会自动重发"
      : commandErrorText(error, "读取屏幕失败，请检查设备连接后重试");
    // eslint-disable-next-line preserve-caught-error -- 输入可能包含密码，刻意不保留原始IPC错误/参数，避免后续日志通过cause回溯敏感内容。
    throw new Error(message);
  }
}

export class RealScreenRemoteAdapter implements ScreenRemoteAdapter {
  open(localProjectId: string, screenId: string) {
    return call<ScreenRemoteSession>("screen_remote_open", { localProjectId, screenId });
  }
  capture(localProjectId: string, sessionId: string) {
    return call<ScreenRemoteFrame>("screen_remote_capture", { localProjectId, sessionId });
  }
  close(localProjectId: string, sessionId: string) {
    return call<void>("screen_remote_close", { localProjectId, sessionId });
  }
  beginControl(localProjectId: string, sessionId: string) {
    return call<ScreenRemoteControl>("screen_remote_begin_control", { localProjectId, sessionId });
  }
  endControl(localProjectId: string, sessionId: string) {
    return call<void>("screen_remote_end_control", { localProjectId, sessionId });
  }
  action(localProjectId: string, sessionId: string, input: ScreenRemoteActionInput) {
    return call<ScreenRemoteActionResult>("screen_remote_action", { localProjectId, sessionId, input });
  }
}
