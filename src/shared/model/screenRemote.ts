/** 单屏远程查看的短会话，仅驻留内存，不进入操作任务或资料草稿。 */
export interface ScreenRemoteSession {
  sessionId: string;
  projectId: string;
  screenId: string;
  name: string;
  ip: string;
}

export interface ScreenRemoteFrame {
  sessionId: string;
  frameId: string;
  width: number;
  height: number;
  capturedAt: string;
  pngBase64: string;
  elapsedMs: number;
  controlActive?: boolean;
  inputReady: boolean;
  inputGeneration: number;
}

export type ScreenRemoteFrameInfo = Omit<ScreenRemoteFrame, "pngBase64">;

export interface ScreenRemoteTarget {
  projectId: string;
  screenId: string;
}

export type ScreenRemoteControlMode = "full" | "wake_only";

export interface ScreenRemoteControl {
  taskId: string;
  idleTimeoutMs: number;
  frameMaxAgeMs: number;
  maxTextLength: number;
  textInputBehavior: "insert" | "replace";
  inputGeneration: number;
  controlMode: ScreenRemoteControlMode;
}

export type ScreenRemoteKey = "backspace" | "clear_text" | "back" | "wake_up";
export type ScreenRemoteAction = { kind: "tap"; x: number; y: number }
  | { kind: "key"; key: ScreenRemoteKey }
  | { kind: "text"; text: string };
export interface ScreenRemoteActionInput { frameId: string; action: ScreenRemoteAction }
export interface ScreenRemoteActionResult {
  status: "sent" | "unconfirmed";
  message: string;
  controlActive: boolean;
  inputGeneration: number;
  controlMode: ScreenRemoteControlMode;
}
