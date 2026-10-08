import type { AioAdapter } from "@/shared/api/aioAdapter";
import { demoNodes } from "@/shared/fixtures/demoData";
import type {
  AioImportSession,
  AioNodeListItem,
  AioNodeStats,
  ImportSelection,
  InventoryValues
} from "@/shared/model/aio";
import type { EdgeNode } from "@/shared/model/demo";
import type { NodeServiceCheckSnapshot, ServiceCheckReport, ServiceObservation } from "@/shared/model/aio";
import { publishFixtureTaskEvent } from "@/dev-fixtures/activityFixtureAdapter";

function mapNode(node: EdgeNode): AioNodeListItem {
  const macNormalized = node.mac.replaceAll(":", "");
  return {
    mac: node.mac,
    macNormalized,
    name: node.name,
    ip: node.ip,
    location: node.location,
    managementState: node.managementState,
    deployLabel: node.deployLabel,
    platformState: node.platformState,
    platformUpdatedAt: node.platformUpdatedAt,
    serviceState: "unknown",
    serviceLabel: "无检查记录",
    lastOperation: node.lastOperation,
    lastOperationAt: undefined,
    platformId: node.platformId,
    source: node.platformId ? "merged" : "import",
    version: 1,
    conflicts: node.managementState === "conflict" ? [{
      code: "FIXTURE_CONFLICT",
      field: "ip",
      message: "名称和 IP 与平台记录不一致",
      workbenchValue: node.ip,
      platformValue: "10.20.13.37"
    }] : [],
    versions: node.versions.map((version) => ({
      macNormalized,
      serviceName: version.service,
      expectedImageName: version.image,
      expectedVersion: version.expectedVersion === "—" ? undefined : version.expectedVersion,
      observedVersion: undefined,
      observedAt: undefined
    }))
  };
}

function stats(nodes: AioNodeListItem[]): AioNodeStats {
  return {
    total: nodes.length,
    online: nodes.filter((node) => node.platformState === "online").length,
    offline: nodes.filter((node) => node.platformState === "offline").length,
    pending: nodes.filter((node) => node.managementState === "pending").length,
    conflicts: nodes.filter((node) => node.managementState === "conflict").length
  };
}

function previewSession(projectId: string, filePath: string): AioImportSession {
  const now = new Date().toISOString();
  return {
    id: "fixture-import",
    localProjectId: projectId,
    fileName: filePath.split(/[\\/]/).at(-1) ?? "inventory.csv",
    filePath,
    state: "preview",
    counts: { total: 3, newPending: 1, existingUnchanged: 1, existingChanged: 0, platformExisting: 1, conflicts: 0, invalid: 0, selected: 2 },
    createdAt: now,
    updatedAt: now,
    items: [
      { rowNumber: 2, values: { name: "AIO-C栋-1F", ip: "10.20.14.21", mac: "00:0C:29:3B:B9:39" }, macNormalized: "000C293BB939", displayMac: "00:0C:29:3B:B9:39", classification: "new_pending", selected: true, errors: [], conflicts: [] },
      { rowNumber: 3, values: { name: "AIO-B栋-2F", ip: "10.20.13.22", mac: "00:0C:29:3B:B9:36" }, macNormalized: "000C293BB936", displayMac: "00:0C:29:3B:B9:36", classification: "existing_unchanged", selected: false, errors: [], conflicts: [] },
      { rowNumber: 4, values: { name: "AIO-平台既有", ip: "10.20.15.21", mac: "00:0C:29:3B:B9:40" }, macNormalized: "000C293BB940", displayMac: "00:0C:29:3B:B9:40", classification: "platform_existing", selected: true, errors: [], conflicts: [], platformAioId: "fixture-platform-aio" }
    ]
  };
}

export class FixtureAioAdapter implements AioAdapter {
  readonly real = false;
  private nodes = structuredClone(demoNodes);
  private sessions = new Map<string, AioImportSession>();
  private serviceChecks = new Map<string, NodeServiceCheckSnapshot>();
  private adoptedMacs = new Set<string>();

  private mappedNode(projectId: string, node: EdgeNode) {
    const mapped = mapNode(node);
    mapped.serviceCheck = this.serviceChecks.get(`${projectId}:${mapped.macNormalized}`);
    return mapped;
  }

  async listNodes(_projectId: string, query: Parameters<AioAdapter["listNodes"]>[1]): ReturnType<AioAdapter["listNodes"]> {
    const all = this.nodes.map((node) => this.mappedNode(_projectId, node));
    const keyword = query.search?.toLocaleLowerCase();
    const filtered = all.filter((node) =>
      (!keyword || [node.name, node.ip, node.mac, node.location].some((value) => value.toLocaleLowerCase().includes(keyword)))
      && (!query.state || query.state === "all" || node.managementState === query.state || node.platformState === query.state)
    );
    const start = (query.page - 1) * query.pageSize;
    return {
      items: structuredClone(filtered.slice(start, start + query.pageSize)),
      total: filtered.length,
      page: query.page,
      pageSize: query.pageSize,
      stats: stats(all),
      platformIssues: [],
      latestImportSessionId: this.sessions.get(_projectId)?.state === "preview" ? this.sessions.get(_projectId)?.id : undefined,
      refreshedAt: new Date().toISOString()
    };
  }
  async getNodeDetail(_projectId: string, mac: string) {
    const node = this.nodes.find((item) => item.mac === mac);
    if (!node) throw new Error(`Fixture 一体机不存在：${mac}`);
    const mapped = this.mappedNode(_projectId, node);
    return { node: mapped, versions: mapped.versions };
  }
  async checkServices(projectId: string, mac: string) {
    const node = this.nodes.find((item) => item.mac.replaceAll(":", "").toUpperCase() === mac.replaceAll(":", "").toUpperCase());
    if (!node) throw new Error(`Fixture 一体机不存在：${mac}`);
    const taskId = `fixture-service-inspection-${crypto.randomUUID()}`;
    const startedAt = new Date().toISOString();
    const emit = (status: "running" | "succeeded", stage: string, sequence: number, message: string) => publishFixtureTaskEvent({
      eventId: `${taskId}-${sequence}`, localTaskId: taskId, sequence, localProjectId: projectId,
      domainType: "aio", resourceType: "aio", resourceKey: node.mac.replaceAll(":", ""),
      stage, status, progressCurrent: status === "succeeded" ? 1 : 0, progressTotal: 1,
      level: "info", messageCode: status === "succeeded" ? "SERVICE_INSPECTION_COMPLETED" : "SERVICE_INSPECTION_STARTED",
      messageParams: { operationType: "service_inspection", taskName: `检查服务 · ${node.name}`, targetCount: "1" },
      message, timestamp: new Date().toISOString()
    });
    emit("running", "service_inspection", 1, "正在读取服务和镜像（演示数据）");
    window.setTimeout(() => {
      const checkedAt = new Date().toISOString();
      const services: ServiceObservation[] = node.versions.map((version) => ({
        serviceName: version.service, state: "normal", runtimeState: "running", healthStatus: "healthy",
        expectedImage: `${version.image}:${version.expectedVersion}`,
        actualImage: `${version.image}:${version.expectedVersion}`,
        checkedAt, source: "manual", message: "演示服务运行正常"
      }));
      const expectedServices = services.map((service) => service.serviceName);
      const report: ServiceCheckReport = { startedAt, checkedAt, source: "manual", scope: "all", expectedServices, services, state: "succeeded" };
      this.serviceChecks.set(`${projectId}:${node.mac.replaceAll(":", "")}`, { expectedServices, services, lastFullCheckAt: checkedAt, lastAttempt: report });
      emit("succeeded", "completed", 2, "服务检查完成，结果已保存在本机（演示数据）");
    }, 350);
    return { taskId };
  }
  async previewImport(projectId: string, filePath: string): ReturnType<AioAdapter["previewImport"]> {
    if (this.sessions.get(projectId)?.state === "preview") throw new Error("当前项目已有未处理导入预览，请继续处理或先放弃");
    const session = previewSession(projectId, filePath);
    this.sessions.set(projectId, session);
    return { session: structuredClone(session), platformIssues: [] };
  }
  async previewCreate(projectId: string, input: InventoryValues): ReturnType<AioAdapter["previewCreate"]> {
    if (this.sessions.get(projectId)?.state === "preview") throw new Error("当前项目有未处理的导入预览，请先处理或放弃后再新增一体机");
    const values = Object.fromEntries(Object.entries(input).map(([key, value]) => [key, value?.trim()])) as unknown as InventoryValues;
    if (!values.name || !values.ip || !values.mac) throw new Error("名称、IP 和 MAC 不能为空");
    const macNormalized = values.mac.replace(/[:-]/g, "").toUpperCase();
    if (!/^[0-9A-F]{12}$/.test(macNormalized)) throw new Error("MAC 格式无效");
    if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(values.ip) || values.ip.split(".").some((part) => Number(part) > 255)) throw new Error("IP 格式无效");
    const existing = this.nodes.find((node) => node.mac.replace(/[:-]/g, "").toUpperCase() === macNormalized);
    if (existing && (existing.managementState !== "platform_existing" || this.adoptedMacs.has(`${projectId}:${macNormalized}`))) {
      throw new Error("该 MAC 对应的一体机已在工作台中，无需重复新增；如需修改，请使用清单导入核对变更");
    }
    if (existing && (existing.name !== values.name || existing.ip !== values.ip)) throw new Error(`该 MAC 已在平台登记为“${existing.name}”（${existing.ip}），请核对名称和 IP 后再接管`);
    const classification = existing ? "platform_existing" : "new_pending";
    const now = new Date().toISOString();
    const session: AioImportSession = {
      id: `fixture-create-${crypto.randomUUID()}`, localProjectId: projectId,
      fileName: "单台新增一体机", filePath: "manual-entry", state: "preview", createdAt: now, updatedAt: now,
      counts: { total: 1, newPending: existing ? 0 : 1, existingUnchanged: 0, existingChanged: 0, platformExisting: existing ? 1 : 0, conflicts: 0, invalid: 0, selected: 1 },
      items: [{ rowNumber: 1, values, macNormalized, displayMac: macNormalized.match(/.{2}/g)!.join(":"), classification, selected: true, errors: [], conflicts: [], platformAioId: existing?.platformId }]
    };
    this.sessions.set(projectId, session);
    return { session: structuredClone(session), platformIssues: [] };
  }
  async getLatestImport(projectId: string) {
    const session = this.sessions.get(projectId);
    return session?.state === "preview" ? structuredClone(session) : null;
  }
  async updateImportSelection(projectId: string, sessionId: string, selections: ImportSelection[]) {
    const session = this.requiredSession(projectId, sessionId);
    for (const selection of selections) {
      const item = session.items.find((row) => row.rowNumber === selection.rowNumber);
      if (item) item.selected = selection.selected;
    }
    session.counts.selected = session.items.filter((row) => row.selected).length;
    return structuredClone(session);
  }
  async applyImport(projectId: string, sessionId: string) {
    const session = this.requiredSession(projectId, sessionId);
    const appliedCount = session.counts.selected;
    session.state = "applied";
    if (session.filePath === "manual-entry" && session.items[0]?.classification === "platform_existing") {
      this.adoptedMacs.add(`${projectId}:${session.items[0].macNormalized}`);
    }
    const selectedNew = session.items.find((item) => item.selected && item.classification === "new_pending");
    if (selectedNew && !this.nodes.some((node) => node.mac === selectedNew.displayMac)) {
      this.nodes.unshift({
        mac: selectedNew.displayMac!, name: selectedNew.values.name, ip: selectedNew.values.ip,
        location: selectedNew.values.location ?? "", managementState: "pending", deployLabel: "待实施",
        platformState: "unknown", platformUpdatedAt: "尚未注册", serviceState: "unknown",
        serviceLabel: "无检查记录", lastOperation: "导入清单 · 刚刚", versions: []
      });
    }
    return { result: { operationId: "fixture-operation", appliedCount }, localSessionFinalized: true };
  }
  async discardImport(projectId: string, sessionId: string) {
    const session = this.requiredSession(projectId, sessionId);
    session.state = "discarded";
  }
  private requiredSession(projectId: string, sessionId: string) {
    const session = this.sessions.get(projectId);
    if (!session || session.id !== sessionId) throw new Error(`Fixture 导入会话不存在：${sessionId}`);
    return session;
  }
}
