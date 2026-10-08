import type { OperationsAdapter } from "@/shared/api/operationsAdapter";
import { publishFixtureTaskEvent } from "@/dev-fixtures/activityFixtureAdapter";
import { demoHistory } from "@/shared/fixtures/demoData";
import type { DemoTask, OperationMode } from "@/shared/model/demo";
import type {
  DeploymentExecutionSnapshot,
  DeploymentTaskView,
  OperationHistoryDetail,
  OperationHistoryPage
} from "@/shared/model/deploymentWorkflow";

export class FixtureOperationsAdapter implements OperationsAdapter {
  readonly real = false;
  private readonly tasks = new Map<string, DeploymentTaskView>();
  private readonly preflightSnapshots = new Map<string, {
    projectId: string;
    executionSnapshot: DeploymentExecutionSnapshot;
  }>();
  async inspectImage(path: string, expectedImage?: string) {
    const repoTags = [expectedImage || "inx/device-edge:fixture"];
    return { archive: { path, size: 1024, repoTags }, expectedImage, expectedMatches: true };
  }
  async preflight(
    projectId: string,
    preflightTaskId: string,
    plan: Parameters<OperationsAdapter["preflight"]>[2]
  ): ReturnType<OperationsAdapter["preflight"]> {
    let sequence = 0;
    const totalWorkItems = preflightWorkTotal(plan.mode, plan.targetMacs.length);
    const emitProgress = (
      current: number,
      message: string,
      messageCode: string,
      stage: string,
      resourceKey?: string,
      status = "checking"
    ) => publishFixtureTaskEvent({
      eventId: `${preflightTaskId}-${++sequence}`,
      localTaskId: preflightTaskId,
      sequence,
      localProjectId: projectId,
      domainType: "aio",
      resourceType: resourceKey ? "aio" : null,
      resourceKey: resourceKey ?? null,
      stage,
      status,
      progressCurrent: current,
      progressTotal: totalWorkItems,
      level: status === "failed" ? "error" : "info",
      messageCode,
      messageParams: {
        operationType: "deployment_preflight",
        taskName: `${modeName(plan.mode)}检查`,
        deploymentMode: plan.mode,
        targetCount: String(plan.targetMacs.length)
      },
      message,
      timestamp: new Date().toISOString()
    });
    emitProgress(0, "正在检查发布参数", "PREFLIGHT_PARAMETERS_STARTED", "检查参数");
    await fixtureProgressYield();
    emitProgress(1, "发布参数检查通过", "PREFLIGHT_PARAMETERS_FINISHED", "检查参数");
    await fixtureProgressYield();
    emitProgress(1, "正在检查镜像文件", "PREFLIGHT_IMAGES_STARTED", "检查镜像");
    await fixtureProgressYield();
    emitProgress(2, "镜像文件检查通过", "PREFLIGHT_IMAGES_FINISHED", "检查镜像");
    await fixtureProgressYield();
    emitProgress(2, "正在检查批次与并发配置", "PREFLIGHT_BATCH_STARTED", "检查批次");
    await fixtureProgressYield();
    emitProgress(3, "批次与并发配置检查通过", "PREFLIGHT_BATCH_FINISHED", "检查批次");
    await fixtureProgressYield();
    let completedWorkItems = 3;
    for (const [index, targetMac] of plan.targetMacs.entries()) {
      const targetLabel = `一体机 ${index + 1}/${plan.targetMacs.length}`;
      emitProgress(completedWorkItems, `正在检查${targetLabel}连通性和运行环境`, "PREFLIGHT_TARGET_RUNTIME_STARTED", "检查一体机", targetMac);
      await fixtureProgressYield();
      completedWorkItems += 1;
      emitProgress(completedWorkItems, `${targetLabel}连通性和运行环境检查通过`, "PREFLIGHT_TARGET_RUNTIME_FINISHED", "检查一体机", targetMac);
      await fixtureProgressYield();
      if (plan.mode !== "service_upgrade") {
        emitProgress(completedWorkItems, `正在为${targetLabel}渲染发布模板`, "PREFLIGHT_TARGET_RENDER_STARTED", "渲染模板", targetMac);
        await fixtureProgressYield();
        completedWorkItems += 1;
        emitProgress(completedWorkItems, `${targetLabel}发布模板渲染通过`, "PREFLIGHT_TARGET_RENDER_FINISHED", "渲染模板", targetMac);
        await fixtureProgressYield();
      }
    }
    emitProgress(totalWorkItems, "部署执行条件检查完成", "PREFLIGHT_SUCCEEDED", "检查完成", undefined, "succeeded");
    const normalizedPlan = {
      ...structuredClone(plan),
      artifactPath: plan.imageFiles[0]?.filePath ?? "",
      artifactName: plan.mode === "service_upgrade"
        ? plan.imageFiles[0]?.serviceName ?? "服务镜像"
        : `${plan.imageFiles.length}个服务镜像`,
      artifactVersion: plan.mode === "service_upgrade" ? "fixture" : "bundle-fixture",
      serviceName: plan.mode === "service_upgrade" ? plan.imageFiles[0]?.serviceName : undefined,
      imageName: plan.mode === "service_upgrade" ? plan.imageFiles[0]?.imageTag : undefined,
      images: Object.fromEntries(plan.imageFiles.map((image) => [image.serviceName, image.imageTag]))
    };
    const executionSnapshot: DeploymentExecutionSnapshot = {
      schemaVersion: 2,
      localProjectId: projectId,
      checkedAt: new Date().toISOString(),
      profileVersion: 1,
      artifactFingerprint: "a".repeat(64),
      plan: normalizedPlan,
      targets: plan.targetMacs.map((mac, index) => ({
        node: {
          macNormalized: mac,
          name: `Fixture AIO ${index + 1}`,
          ip: `192.0.2.${index + 10}`,
          managementState: "managed",
          source: "fixture",
          version: 1
        },
        sshHost: `192.0.2.${index + 10}`,
        sshPort: 22,
        hostKeyAlgorithm: "ssh-ed25519",
        hostKeyFingerprint: `SHA256:fixture-${index + 1}`,
        hostKeyAcceptedAt: new Date().toISOString()
      }))
    };
    this.preflightSnapshots.set(preflightTaskId, {
      projectId,
      executionSnapshot: structuredClone(executionSnapshot)
    });
    return {
      ready: true,
      checks: [
        ...["artifact", "release_profile", "release_endpoints"].map((code) => ({
          code,
          label: "Fixture公共检查",
          status: "passed" as const,
          blocking: false,
          message: "Fixture模拟通过，不代表真实环境"
        })),
        ...plan.targetMacs.flatMap((targetMac) => ["ssh_auth", "platform_endpoints", "runtime_os", "runtime_arch", "docker", "docker_compose", "remote_storage", "remote_ports", ...(plan.mode === "first_deploy" ? [] : ["remote_current_release"])].map((code) => ({
          code, targetMac, label: "Fixture节点检查", status: "passed" as const, blocking: false,
          message: "Fixture模拟通过，不代表真实环境"
        })))
      ],
      normalizedPlan,
      profileVersion: 1,
      executionSnapshot,
      checkedAt: new Date().toISOString()
    };
  }
  async submit(
    projectId: string,
    preflightTaskId: Parameters<OperationsAdapter["submit"]>[1],
    executionSnapshot: Parameters<OperationsAdapter["submit"]>[2]
  ): ReturnType<OperationsAdapter["submit"]> {
    const checked = this.preflightSnapshots.get(preflightTaskId);
    if (!checked || checked.projectId !== projectId) {
      throw new Error("检查结果已失效，请重新检查");
    }
    if (JSON.stringify(checked.executionSnapshot) !== JSON.stringify(executionSnapshot)) {
      throw new Error("执行快照与检查结果不一致，请重新检查");
    }
    const plan = executionSnapshot.plan;
    const task = this.startFixtureTask(
      plan.mode,
      projectId,
      plan.targetMacs,
      plan.artifactName ?? "服务镜像"
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
      instanceId: "DEMO-PC-001122AABBCC-192.0.2.142",
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

function modeName(mode: OperationMode) {
  if (mode === "first_deploy") return "首次部署";
  if (mode === "full_upgrade") return "整包升级";
  return "单服升级";
}

function preflightWorkTotal(mode: OperationMode, targetCount: number) {
  return 3 + targetCount * (mode === "service_upgrade" ? 1 : 2);
}

function fixtureProgressYield() {
  if (import.meta.env.MODE === "test") return Promise.resolve();
  return new Promise<void>((resolve) => window.setTimeout(resolve, 120));
}
