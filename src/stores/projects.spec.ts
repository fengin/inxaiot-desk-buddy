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

  it("deletes only the local project entry", async () => {
    const store = useProjectStore();
    await store.initialize();
    const created = await store.createProject(input);
    await store.deleteProject(created.id);
    expect(store.projects.some((project) => project.id === created.id)).toBe(false);
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
});
