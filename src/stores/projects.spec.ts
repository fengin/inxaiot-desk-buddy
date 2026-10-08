import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import type { ProjectInput } from "@/shared/model/project";
import { useProjectStore } from "@/stores/projects";

const input: ProjectInput = {
  name: "空白目录新项目",
  platformUrl: "http://192.168.3.6:8055",
  dbHost: "192.168.3.6",
  dbPort: 3306,
  dbUser: "fixture",
  dbTlsEnabled: false,
  dbPassword: "fixture-secret",
  businessDb: "inxvision_iot_dev",
  workbenchDb: "inxaiot_desk_buddy"
};

describe("project store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
    localStorage.clear();
  });

  it("creates, tests, switches and logs in an independent project", async () => {
    const store = useProjectStore();
    await store.initialize();
    const test = await store.testConnection({ project: input });
    expect(test.successful).toBe(true);
    const created = await store.createProject(input);
    const switched = await store.switchProject(created.id);
    expect(switched.connectionState).toBe("login_required");
    const challenge = await store.createLoginChallenge(created.id);
    await store.login(created.id, {
      username: "operator",
      password: "secret",
      sessionUuid: challenge.sessionUuid,
      imageCode: "111"
    });
    expect(store.activeProject?.connectionState).toBe("ready");
    expect(store.session?.username).toBe("operator");
  });

  it("本机项目切换不访问远端库，平台读取不被共享库错误阻断", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    const schema = vi.spyOn(adapter, "getWorkbenchSchemaStatus");
    configureWorkbenchAdapter(adapter);
    const store = useProjectStore();
    const local = await store.createProject({ ...input, platformUrl: "", dbHost: "", dbUser: "", businessDb: "", dbPassword: undefined });
    await store.switchProject(local.id);
    expect(schema).not.toHaveBeenCalled();
    expect(store.allowsAccess("local")).toBe(true);
    expect(store.allowsAccess("shared")).toBe(false);
    expect(store.allowsAccess("platform")).toBe(false);
    const remote = await store.createProject(input);
    Object.assign(store.activeProject!, {
      session: { localProjectId: remote.id, state: "active", username: "tester" },
      connectionState: "connection_failed", databaseState: "failed"
    });
    expect(store.allowsAccess("platform")).toBe(true);
    expect(store.allowsAccess("shared")).toBe(false);
  });

  it("deletes only the local project entry", async () => {
    const store = useProjectStore();
    await store.initialize();
    const created = await store.createProject(input);
    await store.deleteProject(created.id);
    expect(store.projects.some((project) => project.id === created.id)).toBe(false);
  });

  it("平台登录成功后共享库故障不丢失会话", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    configureWorkbenchAdapter(adapter);
    const store = useProjectStore();
    const project = await store.createProject(input);
    vi.spyOn(adapter, "switchProject").mockRejectedValue(new Error("共享库暂不可用"));
    const session = await store.login(project.id, { username: "operator", password: "fixture-password", sessionUuid: "fixture-session", imageCode: "111" });
    expect(session.state).toBe("active");
    expect(store.session?.state).toBe("active");
    expect(store.allowsAccess("platform")).toBe(true);
    expect(store.allowsAccess("shared")).toBe(false);
    expect(store.error).toContain("共享库暂不可用");
  });

  it("在项目入口提示Schema未就绪，登录状态刷新不能重新开放业务入口", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    configureWorkbenchAdapter(adapter);
    const store = useProjectStore();
    await store.initialize();
    const projectId = store.activeProjectId;
    const schema = await adapter.getWorkbenchSchemaStatus();
    vi.spyOn(adapter, "getWorkbenchSchemaStatus").mockResolvedValue({ ...schema, state: "upgrade_required", message: "请先升级工作台数据库" });
    await store.switchProject(projectId);
    expect(store.activeProject?.connectionState).toBe("schema_required");
    expect(store.activeProject?.statusMessage).toContain("升级工作台数据库");
    expect(store.businessMenuEnabled).toBe(false);
    await store.checkSession(projectId);
    expect(store.activeProject?.connectionState).toBe("schema_required");
    expect(store.businessMenuEnabled).toBe(false);
  });

  it("keeps a late switch response from replacing the active project", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    configureWorkbenchAdapter(adapter);
    const store = useProjectStore();
    await store.initialize();
    const first = await store.createProject({ ...input, name: "First" });
    const second = await store.createProject({ ...input, name: "Second" });
    const firstResult = await adapter.switchProject(first.id);
    const original = adapter.switchProject.bind(adapter);
    let resolveFirst!: (value: typeof firstResult) => void;
    vi.spyOn(adapter, "switchProject").mockImplementation((projectId) => {
      if (projectId === first.id) {
        return new Promise((resolve) => {
          resolveFirst = resolve;
        });
      }
      return original(projectId);
    });

    const stale = store.switchProject(first.id);
    await store.switchProject(second.id);
    resolveFirst(firstResult);
    await stale;
    expect(store.activeProjectId).toBe(second.id);
    expect(store.activeProject?.id).toBe(second.id);
  });

  it("fails closed when remote session verification is unavailable", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    configureWorkbenchAdapter(adapter);
    const store = useProjectStore();
    await store.initialize();
    const projectId = store.activeProjectId;
    vi.spyOn(adapter, "checkProjectSession").mockRejectedValue(
      new Error("verification unavailable")
    );
    await expect(store.checkSession(projectId)).rejects.toThrow();
    expect(store.activeProject?.connectionState).toBe("connection_failed");
    expect(store.businessMenuEnabled).toBe(false);
  });

  it("keeps initialization retryable after a transient project-list failure", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    const original = adapter.listProjects.bind(adapter);
    vi.spyOn(adapter, "listProjects")
      .mockRejectedValueOnce(new Error("temporary list failure"))
      .mockImplementation(original);
    configureWorkbenchAdapter(adapter);
    const store = useProjectStore();

    await store.initialize();
    expect(store.initialized).toBe(false);
    expect(store.error).toContain("temporary list failure");

    await store.initialize();
    expect(store.initialized).toBe(true);
    expect(store.error).toBe("");
    expect(store.projects.length).toBeGreaterThan(0);
  });
});
