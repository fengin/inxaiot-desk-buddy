import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import type { DeploymentPreflightReport } from "@/shared/model/deploymentWorkflow";
import type { DeploymentPlanInput } from "@/shared/model/release";
import { useDeploymentWorkflowStore } from "@/stores/deploymentWorkflow";

describe("deployment workflow store", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("loads fixture task and history only through adapter contracts", async () => {
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

    await store.loadTask("fixture-project", submission.taskId);
    expect(store.currentTask?.successCount).toBe(1);
    expect(store.currentTask?.targets[0]?.state).toBe("succeeded");

    await store.loadHistory("fixture-project");
    expect(store.history.items.length).toBeGreaterThan(0);
  });

  it("ignores a late preflight response from the previous project", async () => {
    const adapter = new FixtureOperationsAdapter();
    let resolveOld!: (report: DeploymentPreflightReport) => void;
    const original = adapter.preflight.bind(adapter);
    vi.spyOn(adapter, "preflight").mockImplementation((projectId, plan) => {
      if (projectId === "project-old") {
        return new Promise((resolve) => {
          resolveOld = resolve;
        });
      }
      return original(projectId, plan);
    });
    configureOperationsAdapter(adapter);
    const store = useDeploymentWorkflowStore();
    const plan: DeploymentPlanInput = {
      mode: "full_upgrade",
      targetMacs: ["001122334455"],
      artifactPath: "C:/fixture/release",
      artifactName: "Release fixture",
      artifactVersion: "fixture",
      batchSize: 1,
      concurrency: 1
    };
    try {
      const oldRequest = store.runPreflight("project-old", plan);
      store.bindProject("project-new");
      const current = await store.runPreflight("project-new", plan);
      resolveOld({
        ...current,
        executionSnapshot: current.executionSnapshot
          ? { ...current.executionSnapshot, localProjectId: "project-old" }
          : null
      });
      await oldRequest;
      expect(store.preflightProjectId).toBe("project-new");
      expect(store.preflight?.executionSnapshot?.localProjectId).toBe("project-new");
    } finally {
      configureOperationsAdapter(new FixtureOperationsAdapter());
    }
  });
});
