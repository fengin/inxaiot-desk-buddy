import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { useOperationsAdapter } from "@/shared/api/operationsAdapter";
import { useDeploymentWorkflowStore } from "@/stores/deploymentWorkflow";

describe("deployment workflow store", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("loads fixture task and history only through adapter contracts", async () => {
    const adapter = useOperationsAdapter();
    const fixture = adapter.startFixtureTask(
      "full_upgrade",
      "fixture-project",
      ["001122334455"],
      "Release fixture"
    );
    const store = useDeploymentWorkflowStore();
    const preflight = await store.runPreflight("fixture-project", {
      mode: "full_upgrade",
      targetMacs: ["001122334455"],
      artifactPath: "C:/fixture/release",
      artifactName: "Release fixture",
      artifactVersion: "fixture",
      batchSize: 1,
      concurrency: 1
    });
    expect(preflight.ready).toBe(true);

    const submission = await store.submit(
      "fixture-project",
      preflight.normalizedPlan
    );
    expect(submission.taskId).toContain("fixture-task-");
    expect(store.currentTask?.id).toBe(submission.taskId);

    await store.loadTask("fixture-project", fixture.id);
    expect(store.currentTask?.successCount).toBe(1);
    expect(store.currentTask?.targets[0]?.state).toBe("succeeded");

    await store.loadHistory("fixture-project");
    expect(store.history.items.length).toBeGreaterThan(0);
  });
});
