import type { ScreenRemoteActionInput, ScreenRemoteActionResult, ScreenRemoteControl, ScreenRemoteFrame, ScreenRemoteSession } from "@/shared/model/screenRemote";

/** 独立于普通任务计划，图片和后续交互内容不得进入任务持久化。 */
export interface ScreenRemoteAdapter {
  open(projectId: string, screenId: string): Promise<ScreenRemoteSession>;
  capture(projectId: string, sessionId: string): Promise<ScreenRemoteFrame>;
  close(projectId: string, sessionId: string): Promise<void>;
  beginControl(projectId: string, sessionId: string): Promise<ScreenRemoteControl>;
  endControl(projectId: string, sessionId: string): Promise<void>;
  action(projectId: string, sessionId: string, input: ScreenRemoteActionInput): Promise<ScreenRemoteActionResult>;
}

export class UnavailableScreenRemoteAdapter implements ScreenRemoteAdapter {
  async open(): Promise<ScreenRemoteSession> {
    throw new Error("开发预览不连接真实屏幕，请在工作台桌面程序中查看");
  }
  async capture(): Promise<ScreenRemoteFrame> {
    throw new Error("尚未连接屏幕");
  }
  async close(): Promise<void> {}
  async beginControl(): Promise<ScreenRemoteControl> { throw new Error("开发预览不操作真实屏幕"); }
  async endControl(): Promise<void> {}
  async action(): Promise<ScreenRemoteActionResult> { throw new Error("开发预览不操作真实屏幕"); }
}

let adapter: ScreenRemoteAdapter = new UnavailableScreenRemoteAdapter();
export function configureScreenRemoteAdapter(next: ScreenRemoteAdapter) { adapter = next; }
export function useScreenRemoteAdapter(): ScreenRemoteAdapter { return adapter; }
