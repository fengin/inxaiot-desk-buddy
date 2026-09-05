import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import type { DeploymentPreflightReport } from "@/shared/model/deploymentWorkflow";
import type { DeploymentPlanInput } from "@/shared/model/release";
import { useDeploymentWorkflowStore } from "@/stores/deploymentWorkflow";

describe("deployment workflow store", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("keeps changed-host-key warnings visible without blocking submission", async () => {
    const adapter = new FixtureOperationsAdapter();
    const original = adapter.preflight.bind(adapter);
    vi.spyOn(adapter, "preflight").mockImplementation(async (projectId, preflightTaskId, plan) => {
      const report = await original(projectId, preflightTaskId, plan);
      return {
        ...report,
        checks: [...report.checks, {
          code: "host_key_changed", label: "主机指纹变化", status: "warning" as const,
          blocking: false, targetMac: "001122334455", message: "指纹变化，已记录并继续连接"
        }]
      };
    });
    configureOperationsAdapter(adapter);
    const store = useDeploymentWorkflowStore();
    const report = await store.runPreflight("fixture-project", "preflight-warning", {
      mode: "full_upgrade", targetMacs: ["001122334455"], artifactPath: "C:/fixture/release",
      imageFiles: [{ serviceName: "device-edge", filePath: "C:/fixture/device-edge.tar", imageTag: "device-edge:fixture" }],
      artifactName: "Release fixture", artifactVersion: "fixture", batchSize: 1, concurrency: 1
    });
    expect(store.preflight?.checks.some((check) => check.code === "host_key_changed" && !check.blocking)).toBe(true);
    expect(report.ready).toBe(true);
    expect((await store.submit(
      "fixture-project",
      "preflight-warning",
      report.executionSnapshot!
    )).taskId).toContain("fixture-task-");
  });

  it("loads fixture task and history only through adapter contracts", async () => {
    configureOperationsAdapter(new FixtureOperationsAdapter());
    const store = useDeploymentWorkflowStore();
    const preflight = await store.runPreflight("fixture-project", "preflight-load", {
      mode: "full_upgrade",
      targetMacs: ["001122334455"],
      imageFiles: [{ serviceName: "device-edge", filePath: "C:/fixture/device-edge.tar", imageTag: "device-edge:fixture" }],
      artifactPath: "C:/fixture/release",
      artifactName: "Release fixture",
      artifactVersion: "fixture",
      batchSize: 1,
      concurrency: 1
    });
    expect(preflight.ready).toBe(true);

    const submission = await store.submit(
      "fixture-project",
      "preflight-load",
      preflight.executionSnapshot!
    );
    expect(submission.taskId).toContain("fixture-task-");
    expect(store.currentTask?.id).toBe(submission.taskId);

    await store.loadTask("fixture-project", submission.taskId);
    expect(store.currentTask?.successCount).toBe(1);
    expect(store.currentTask?.targets[0]?.state).toBe("succeeded");

    await store.loadHistory("fixture-project");
    expect(store.history.items.length).toBeGreaterThan(0);
    await store.loadHistory("fixture-project", { page: 2, pageSize: 1 });
    expect(store.history.page).toBe(2);
    expect(store.history.pageSize).toBe(1);
    expect(store.history.total).toBeGreaterThan(0);
  });

  it("keeps a successful submission when the first task-detail read fails", async () => {
    const adapter = new FixtureOperationsAdapter();
    configureOperationsAdapter(adapter);
    const store = useDeploymentWorkflowStore();
    const preflight = await store.runPreflight("fixture-project", "preflight-detail-delay", {
      mode: "service_upgrade",
      targetMacs: ["001122334455"],
      imageFiles: [{ serviceName: "device-edge", filePath: "C:/fixture/device-edge.tar", imageTag: "device-edge:fixture" }],
      artifactPath: "C:/fixture/device-edge.tar",
      artifactName: "device-edge",
      artifactVersion: "fixture",
      batchSize: 1,
      concurrency: 1
    });
    vi.spyOn(adapter, "getTask").mockRejectedValue({
      code: "NOT_FOUND",
      params: { summary: "部署任务详情暂不可读" }
    });

    const submission = await store.submit(
      "fixture-project",
      "preflight-detail-delay",
      preflight.executionSnapshot!
    );

    expect(store.submission?.taskId).toBe(submission.taskId);
    expect(store.currentTask).toBeUndefined();
    expect(store.error).toBe("部署任务详情暂不可读");
  });

  it("ignores a late preflight response from the previous project", async () => {
    const adapter = new FixtureOperationsAdapter();
    let resolveOld!: (report: DeploymentPreflightReport) => void;
    const original = adapter.preflight.bind(adapter);
    vi.spyOn(adapter, "preflight").mockImplementation((projectId, preflightTaskId, plan) => {
      if (projectId === "project-old") {
        return new Promise((resolve) => {
          resolveOld = resolve;
        });
      }
      return original(projectId, preflightTaskId, plan);
    });
    configureOperationsAdapter(adapter);
    const store = useDeploymentWorkflowStore();
    const plan: DeploymentPlanInput = {
      mode: "full_upgrade",
      targetMacs: ["001122334455"],
      imageFiles: [{ serviceName: "device-edge", filePath: "C:/fixture/device-edge.tar", imageTag: "device-edge:fixture" }],
      artifactPath: "C:/fixture/release",
      artifactName: "Release fixture",
      artifactVersion: "fixture",
      batchSize: 1,
      concurrency: 1
    };
    try {
      const oldRequest = store.runPreflight("project-old", "preflight-old", plan);
      store.bindProject("project-new");
      const current = await store.runPreflight("project-new", "preflight-new", plan);
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
