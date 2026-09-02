import type { WorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { demoProjects, demoReleaseProfile } from "@/shared/fixtures/demoData";
import type {
  ConfirmHostKeyRequest,
  HostKeyCaptureRequest,
  HostKeyObservation,
  PlatformLoginRequest,
  ProjectConnectionTestRequest,
  ProjectInput,
  ProjectOverview,
  ProjectSession,
  WorkbenchSchemaStatus
} from "@/shared/model/project";
import type {
  ReleaseMasterKeyOperationResult,
  ReleaseProfileDraft,
  ReleaseProfileView
} from "@/shared/model/releaseProfile";

const readySchema = (): WorkbenchSchemaStatus => ({
  state: "ready",
  currentVersion: 2,
  latestAvailableVersion: 2,
  appliedMigrationCount: 2,
  failedMigrationCount: 0,
  missingTables: [],
  forbiddenTables: [],
  message: "浏览器 Fixture：工作台数据库结构已就绪"
});

function fixtureProject(index: number): ProjectOverview {
  const source = demoProjects[index]!;
  const state = source.connectionState === "ready"
    ? "ready"
    : source.connectionState === "login_required"
      ? "login_required"
      : "connection_failed";
  return {
    id: source.id,
    name: source.name,
    platformUrl: source.platformUrl,
    dbHost: source.databaseHost.split(":")[0] ?? source.databaseHost,
    dbPort: Number(source.databaseHost.split(":")[1] ?? 3306),
    dbUser: "inxvision",
    dbTlsEnabled: false,
    businessDb: source.databaseName,
    workbenchDb: "inxaiot_desk_buddy",
    connectionState: state,
    databaseState: state === "connection_failed" ? "failed" : "connected",
    schemaState: "ready",
    session: source.username
      ? {
          localProjectId: source.id,
          username: source.username,
          state: "active",
          updatedAt: new Date().toISOString()
        }
      : undefined,
    connectionEncrypted: false,
    statusMessage: state === "ready" ? "Fixture 项目已就绪" : "Fixture 项目需要登录"
  };
}

function fixtureRelease(): ReleaseProfileView {
  return {
    profileKey: "default",
    version: demoReleaseProfile.version,
    updatedBy: demoReleaseProfile.updatedBy,
    updatedAt: demoReleaseProfile.updatedAt,
    values: {
      envTemplate: demoReleaseProfile.envTemplate,
      composeTemplate: demoReleaseProfile.composeTemplate,
      platformHost: demoReleaseProfile.platformHost,
      platformApiPort: Number(new URL(demoReleaseProfile.platformApi).port || 80),
      platformMqttHost: demoReleaseProfile.platformMqttHost,
      platformMqttPort: demoReleaseProfile.platformMqttPort,
      sshPort: 22,
      sshTimeoutSeconds: 15,
      aioDataRoot: "/opt/data",
      aioDeployRoot: "/opt/data/inxaiot"
    },
    credentials: {
      platformAuthKey: demoReleaseProfile.platformAuthKey,
      platformMqttUser: demoReleaseProfile.platformMqttUsername,
      platformMqttPassword: demoReleaseProfile.platformMqttPassword,
      aioMqttUser: demoReleaseProfile.aioMqttUsername,
      aioMqttPassword: demoReleaseProfile.aioMqttPassword,
      sshUser: demoReleaseProfile.sshUsername,
      sshPassword: demoReleaseProfile.sshPassword,
      sshPrivateKey: demoReleaseProfile.sshPrivateKey
    }
  };
}

export class FixtureWorkbenchAdapter implements WorkbenchAdapter {
  private projects = demoProjects.map((_, index) => fixtureProject(index));
  private profiles = new Map<string, ReleaseProfileView>([[this.projects[0]!.id, fixtureRelease()]]);
  private hostKeys = new Map<string, HostKeyObservation[]>();
  private keyVersions = new Map<string, number>();
  private idCounter = 0;

  async listProjects() { return structuredClone(this.projects); }

  async createProject(input: ProjectInput) {
    this.idCounter += 1;
    const project: ProjectOverview = {
      id: `fixture-project-${this.idCounter}`,
      name: input.name,
      platformUrl: input.platformUrl,
      dbHost: input.dbHost,
      dbPort: input.dbPort,
      dbUser: input.dbUser,
      dbTlsEnabled: input.dbTlsEnabled,
      businessDb: input.businessDb,
      workbenchDb: input.workbenchDb,
      connectionState: "disconnected",
      databaseState: "disconnected",
      connectionEncrypted: input.dbTlsEnabled,
      statusMessage: "项目入口已创建"
    };
    this.projects.unshift(project);
    return structuredClone(project);
  }

  async updateProject(projectId: string, input: ProjectInput) {
    const project = this.requiredProject(projectId);
    Object.assign(project, {
      name: input.name,
      platformUrl: input.platformUrl,
      dbHost: input.dbHost,
      dbPort: input.dbPort,
      dbUser: input.dbUser,
      dbTlsEnabled: input.dbTlsEnabled,
      businessDb: input.businessDb,
      workbenchDb: input.workbenchDb,
      connectionState: "disconnected",
      databaseState: "disconnected",
      session: undefined,
      connectionEncrypted: input.dbTlsEnabled,
      statusMessage: "项目入口已更新，请重新连接"
    });
    return structuredClone(project);
  }

  async deleteProject(projectId: string) {
    this.projects = this.projects.filter((item) => item.id !== projectId);
    this.profiles.delete(projectId);
    this.hostKeys.delete(projectId);
  }

  async testProjectConnection(request: ProjectConnectionTestRequest) {
    return {
      successful: true,
      platformDatabaseConnected: true,
      workbenchDatabaseConnected: true,
      platformSchemaCompatible: true,
      workbenchSchemaState: "ready",
      workbenchSchemaMessage: "Fixture Schema 已就绪",
      mysqlVersion: "8.0-fixture",
      connectionEncrypted: request.project.dbTlsEnabled,
      message: "Fixture 双数据库连接测试通过"
    };
  }

  async switchProject(projectId: string) {
    const project = this.requiredProject(projectId);
    project.databaseState = "connected";
    project.connectionEncrypted = project.dbTlsEnabled;
    project.schemaState = "ready";
    project.connectionState = project.session?.state === "active" ? "ready" : "login_required";
    project.statusMessage = project.connectionState === "ready" ? "Fixture 项目已就绪" : "请登录 Fixture 平台";
    return structuredClone(project);
  }

  async createLoginChallenge() {
    return {
      sessionUuid: "fixture-session-uuid",
      captchaImageDataUrl: undefined,
      requiresCaptcha: true,
      expiresAtEpochSeconds: Math.floor(Date.now() / 1000) + 300
    };
  }

  async loginProject(projectId: string, request: PlatformLoginRequest) {
    const project = this.requiredProject(projectId);
    const session: ProjectSession = {
      localProjectId: projectId,
      username: request.username,
      state: "active",
      updatedAt: new Date().toISOString()
    };
    project.session = session;
    project.connectionState = "ready";
    project.databaseState = "connected";
    project.statusMessage = "Fixture 登录成功";
    return structuredClone(session);
  }

  async getProjectSession(projectId: string) {
    return structuredClone(this.requiredProject(projectId).session ?? this.missingSession(projectId));
  }
  async checkProjectSession(projectId: string) { return this.getProjectSession(projectId); }
  async logoutProject(projectId: string) {
    const project = this.requiredProject(projectId);
    project.session = undefined;
    project.connectionState = "login_required";
    return this.missingSession(projectId);
  }

  async getWorkbenchSchemaStatus() { return readySchema(); }
  async initializeOrUpgradeWorkbenchSchema() { return readySchema(); }
  async getReleaseProfile(projectId: string) {
    return structuredClone(this.profiles.get(projectId) ?? null);
  }
  async validateReleaseProfile(draft: ReleaseProfileDraft) {
    const count = draft.values.envTemplate.match(/\{\{[^{}]+\}\}/g)?.length ?? 0;
    return { valid: true, recognizedPlaceholderCount: count, warnings: [] };
  }
  async saveReleaseProfile(projectId: string, draft: ReleaseProfileDraft) {
    const previous = this.profiles.get(projectId);
    if (previous && draft.expectedVersion !== previous.version) {
      throw { code: "CONFIG_VERSION_CONFLICT", params: { summary: "发布配置已被其他实例更新" } };
    }
    const profile: ReleaseProfileView = {
      profileKey: "default",
      values: { ...draft.values },
      credentials: { ...draft.credentials },
      version: (previous?.version ?? 0) + 1,
      updatedBy: this.requiredProject(projectId).session?.username ?? "Fixture 用户",
      updatedAt: new Date().toISOString()
    };
    this.profiles.set(projectId, profile);
    return structuredClone(profile);
  }

  async exportReleaseMasterKey(projectId: string): Promise<ReleaseMasterKeyOperationResult> {
    this.requiredProject(projectId);
    const keyVersion = this.keyVersions.get(projectId) ?? 1;
    return { keyVersion, message: `Fixture 项目主密钥v${keyVersion}已导出` };
  }
  async importReleaseMasterKey(projectId: string): Promise<ReleaseMasterKeyOperationResult> {
    this.requiredProject(projectId);
    const keyVersion = this.keyVersions.get(projectId) ?? 1;
    return { keyVersion, message: `Fixture 项目主密钥v${keyVersion}已导入` };
  }
  async listHostKeys(projectId: string) { return structuredClone(this.hostKeys.get(projectId) ?? []); }
  async captureHostKey(projectId: string, request: HostKeyCaptureRequest) {
    const port = request.port ?? 22;
    const known = (this.hostKeys.get(projectId) ?? []).find((item) => item.host === request.host && item.port === port);
    return {
      host: request.host,
      port,
      algorithm: "ssh-ed25519",
      fingerprint: `SHA256:fixture-${request.host.replaceAll(".", "-")}`,
      state: known ? "confirmed" as const : "unconfirmed" as const,
      expectedFingerprint: known?.fingerprint,
      acceptedAt: known?.acceptedAt
    };
  }
  async confirmHostKey(projectId: string, request: ConfirmHostKeyRequest) {
    const keys = this.hostKeys.get(projectId) ?? [];
    const next: HostKeyObservation = {
      ...request,
      state: "confirmed",
      expectedFingerprint: request.fingerprint,
      acceptedAt: new Date().toISOString()
    };
    const index = keys.findIndex((item) => item.host === request.host && item.port === request.port);
    if (index >= 0) keys[index] = next;
    else keys.push(next);
    this.hostKeys.set(projectId, keys);
    return structuredClone(next);
  }

  private requiredProject(projectId: string) {
    const project = this.projects.find((item) => item.id === projectId);
    if (!project) throw new Error(`Fixture 项目不存在：${projectId}`);
    return project;
  }

  private missingSession(projectId: string): ProjectSession {
    return { localProjectId: projectId, state: "missing" };
  }
}
