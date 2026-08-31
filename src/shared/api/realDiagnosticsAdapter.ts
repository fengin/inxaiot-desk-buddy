import { invoke } from "@tauri-apps/api/core";

import type { DiagnosticsAdapter } from "@/shared/api/diagnosticsAdapter";
import type { SystemDiagnostics } from "@/shared/model/diagnostics";

export class RealDiagnosticsAdapter implements DiagnosticsAdapter {
  getSystemDiagnostics() {
    return invoke<SystemDiagnostics>("get_system_diagnostics");
  }
}
