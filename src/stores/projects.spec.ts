import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

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
});
