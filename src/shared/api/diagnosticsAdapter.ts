import type { SystemDiagnostics } from "@/shared/model/diagnostics";

export interface DiagnosticsAdapter {
  getSystemDiagnostics(): Promise<SystemDiagnostics>;
}

let adapter: DiagnosticsAdapter | undefined;

export function configureDiagnosticsAdapter(next: DiagnosticsAdapter) {
  adapter = next;
}

export function useDiagnosticsAdapter(): DiagnosticsAdapter {
  if (!adapter) throw new Error("诊断 Adapter 尚未初始化");
  return adapter;
}
