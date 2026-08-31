import type { OperationsAdapter } from "@/shared/api/operationsAdapter";
import { demoHistory } from "@/shared/fixtures/demoData";
import type { DemoTask, OperationMode } from "@/shared/model/demo";
import type {
  DeploymentTaskView,
  OperationHistoryDetail,
  OperationHistoryPage
} from "@/shared/model/deploymentWorkflow";

export class FixtureOperationsAdapter implements OperationsAdapter {
  readonly real = false;
  private readonly tasks = new Map<string, DeploymentTaskView>();
  async inspectArtifact(mode: OperationMode, path: string) {
    if (mode === "service_upgrade") {
      return { imageInspection: { archive: { path, size: 1024, repoTags: ["inx/device-edge:fixture"] }, expectedMatches: true } };
    }
    return {
      releaseValidation: {
        valid: true,
        packageDir: path,
        manifest: {
          schemaVersion: 1,
          version: "2026.08.27-fixture",
          composeFile: "docker-compose.yml",
          images: [],
          templates: { env: "templates/env.template", hostInfo: "templates/host-info.json.template" },
          runtime: { os: "anolis", arch: "x86_64", docker: "26.1.3", compose: "v2.27.0" }
        },
        images: [],
        errors: [],
        warnings: []
      }
    };
  }
  async preflight(
    _projectId: string,
    plan: Parameters<OperationsAdapter["preflight"]>[1]
  ) {
    return {
      ready: true,
      checks: [
        {
          code: "fixture_remote_runtime",
          label: "浏览器Fixture远端门禁",
          status: "passed" as const,
          blocking: false,
          message: "Fixture Adapter独立模拟通过，不代表Tauri真实环境"
        }
      ],
      normalizedPlan: structuredClone(plan),
      profileVersion: 1,
      checkedAt: new Date().toISOString()
    };
  }
  async submit(
    projectId: string,
    plan: Parameters<OperationsAdapter["submit"]>[1]
  ) {
    const task = this.startFixtureTask(
      plan.mode,
      projectId,
      plan.targetMacs,
      plan.artifactName
    );
    return {
      taskId: task.id,
      state: task.state,
      submittedAt: task.updatedAt
    };
  }
  async getTask(_projectId: string, taskId: string) {
    const task = this.tasks.get(taskId);
    if (!task) throw new Error("Fixture任务不存在：" + taskId);
    return structuredClone(task);
  }
  async listHistory(
    _projectId: string,
    query: Parameters<OperationsAdapter["listHistory"]>[1]
  ): Promise<OperationHistoryPage> {
    const items = demoHistory.map((record) => ({
      id: record.id,
      domainType: "aio",
      operationType: record.type,
      operationName: record.type,
      operatorName: record.operator,
      instanceId: "fixture-instance",
      state: record.result === "成功" ? "succeeded" : record.result === "部分成功" ? "partially_succeeded" : "failed",
      targetCount: Number.parseInt(record.targetSummary, 10) || 0,
      successCount: record.result === "失败" ? 0 : Number.parseInt(record.targetSummary, 10) || 0,
      failureCount: record.result === "部分成功" ? 1 : record.result === "失败" ? 1 : 0,
      cancelledCount: 0,
      artifactName: record.artifact,
      startedAt: record.finishedAt,
      endedAt: record.finishedAt
    }));
    return { items, total: items.length, page: query.page, pageSize: query.pageSize };
  }
  async getHistoryDetail(
    projectId: string,
    operationId: string
  ): Promise<OperationHistoryDetail> {
    const page = await this.listHistory(projectId, { page: 1, pageSize: 100 });
    const operation = page.items.find((item) => item.id === operationId);
    if (!operation) throw new Error("Fixture操作不存在：" + operationId);
    return { operation, targets: [] };
  }
  private startFixtureTask(mode: OperationMode, projectId: string, targetMacs: string[], artifact: string): DemoTask {
    const task: DemoTask = {
      id: `fixture-task-${Date.now()}`,
      projectId,
      name: `${mode} · ${artifact}`,
      mode,
      state: "succeeded",
      stage: "已完成",
      progress: 100,
      targetCount: targetMacs.length,
      completedCount: targetMacs.length,
      updatedAt: "刚刚"
    };
    this.tasks.set(task.id, {
      ...task,
      operationType: mode,
      successCount: targetMacs.length,
      failureCount: 0,
      cancelledCount: 0,
      cancellable: false,
      targets: targetMacs.map((mac) => ({
        mac,
        state: "succeeded",
        stage: "completed",
        progress: 100,
        updatedAt: task.updatedAt
      })),
      steps: []
    });
    return task;
  }
}
