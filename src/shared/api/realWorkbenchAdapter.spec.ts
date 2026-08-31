import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { RealWorkbenchAdapter } from "@/shared/api/realWorkbenchAdapter";
import type { ProjectInput } from "@/shared/model/project";
import { emptyReleaseProfileDraft } from "@/shared/model/releaseProfile";

const project: ProjectInput = {
  name: "契约项目",
  platformUrl: "http://192.168.3.6:8055",
  dbHost: "192.168.3.6",
  dbPort: 3306,
  dbUser: "tester",
  dbPassword: "secret",
  businessDb: "inxvision_iot_dev",
  workbenchDb: "inxaiot_desk_buddy"
};

describe("RealWorkbenchAdapter Tauri Command 契约", () => {
  const adapter = new RealWorkbenchAdapter();

  beforeEach(() => invokeMock.mockReset().mockResolvedValue({}));

  it("maps project CRUD, connection test and switch to stable command names", async () => {
    await adapter.listProjects();
    expect(invokeMock).toHaveBeenLastCalledWith("list_local_projects");
    await adapter.createProject(project);
    expect(invokeMock).toHaveBeenLastCalledWith("create_local_project", { input: project });
    await adapter.updateProject("project-1", project);
    expect(invokeMock).toHaveBeenLastCalledWith("update_local_project", { projectId: "project-1", input: project });
    await adapter.testProjectConnection({ existingProjectId: "project-1", project });
    expect(invokeMock).toHaveBeenLastCalledWith("test_project_connection", { request: { existingProjectId: "project-1", project } });
    await adapter.switchProject("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("switch_project", { projectId: "project-1" });
    await adapter.deleteProject("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("delete_local_project", { projectId: "project-1" });
  });

  it("maps platform session lifecycle without exposing tokens", async () => {
    await adapter.createLoginChallenge("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("create_project_login_challenge", { projectId: "project-1" });
    const request = { username: "operator", password: "secret", sessionUuid: "uuid", imageCode: "111" };
    await adapter.loginProject("project-1", request);
    expect(invokeMock).toHaveBeenLastCalledWith("login_project", { projectId: "project-1", request });
    await adapter.getProjectSession("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("get_project_session", { projectId: "project-1" });
    await adapter.checkProjectSession("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("check_project_session", { projectId: "project-1" });
    await adapter.logoutProject("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("logout_project", { projectId: "project-1" });
  });

  it("maps release profile validation/versioned save and HostKey lifecycle", async () => {
    const draft = emptyReleaseProfileDraft();
    await adapter.getReleaseProfile("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("get_release_profile", { projectId: "project-1" });
    await adapter.validateReleaseProfile(draft);
    expect(invokeMock).toHaveBeenLastCalledWith("validate_release_profile", { draft });
    await adapter.saveReleaseProfile("project-1", draft);
    expect(invokeMock).toHaveBeenLastCalledWith("save_release_profile", { projectId: "project-1", draft });
    await adapter.listHostKeys("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("list_host_keys", { projectId: "project-1" });
    await adapter.captureHostKey("project-1", { host: "192.168.3.79", port: 22 });
    expect(invokeMock).toHaveBeenLastCalledWith("capture_host_key", { projectId: "project-1", request: { host: "192.168.3.79", port: 22 } });
    const confirm = { host: "192.168.3.79", port: 22, algorithm: "ssh-ed25519", fingerprint: "SHA256:test", replaceChanged: true };
    await adapter.confirmHostKey("project-1", confirm);
    expect(invokeMock).toHaveBeenLastCalledWith("confirm_host_key", { projectId: "project-1", request: confirm });
  });

  it("keeps schema commands behind the same Real Adapter", async () => {
    await adapter.getWorkbenchSchemaStatus("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("get_workbench_schema_status", { localProjectId: "project-1" });
    await adapter.initializeOrUpgradeWorkbenchSchema("project-1");
    expect(invokeMock).toHaveBeenLastCalledWith("initialize_or_upgrade_workbench_schema", { localProjectId: "project-1" });
  });
});
