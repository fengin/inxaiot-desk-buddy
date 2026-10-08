import type { ScreenOperationLock } from './operationHistory';

export interface ScreenTakeoverConflict {
  operationId: string;
  operationName: string;
  operationType: string;
  targetCount?: number;
  ownerUser: string;
  ownerInstanceId: string;
  targets: string[];
  locks: ScreenOperationLock[];
}
export interface ScreenTakeoverRequest { projectId: string; conflicts: ScreenTakeoverConflict[] }
export interface ScreenTakeoverConfirmation { assertCurrent(): void; finish(): void }
type Handler = (request: ScreenTakeoverRequest) => Promise<ScreenTakeoverConfirmation>;
let handler: Handler | undefined;
export function registerScreenTakeoverHandler(next: Handler) {
  handler = next;
  return () => { if (handler === next) handler = undefined; };
}
export function confirmScreenTakeover(request: ScreenTakeoverRequest) {
  if (!handler) return Promise.reject(new Error('请在当前智能屏页面确认是否接手操作'));
  return handler(request);
}
export function screenTakeoverConflicts(error: unknown): ScreenTakeoverConflict[] | undefined {
  const cause = error instanceof Error ? error.cause : error;
  if (!cause || typeof cause !== 'object' || !('code' in cause) || cause.code !== 'SCREEN_TAKEOVER_REQUIRED') return;
  const params = 'params' in cause ? cause.params as Record<string,string> : undefined;
  try {
    const value = JSON.parse(params?.details ?? '{}') as {conflicts?: ScreenTakeoverConflict[]};
    if (Array.isArray(value.conflicts) && value.conflicts.length && value.conflicts.every(item=>item.operationId && item.locks?.length)) return value.conflicts;
  } catch { /* 格式异常作为普通失败处理，不能猜测并释放其他锁。 */ }
}
