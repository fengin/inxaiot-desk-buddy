import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { RealDiagnosticsAdapter } from "@/shared/api/realDiagnosticsAdapter";

describe("RealDiagnosticsAdapter Tauri Command 契约", () => {
  beforeEach(() => invokeMock.mockReset().mockResolvedValue({}));

  it("uses the real diagnostics command", async () => {
    await new RealDiagnosticsAdapter().getSystemDiagnostics();
    expect(invokeMock).toHaveBeenLastCalledWith("get_system_diagnostics");
  });
});
