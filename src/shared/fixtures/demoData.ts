import type {
  DemoLogEntry,
  DemoProject,
  DemoTask,
  EdgeNode,
  OperationHistoryItem,
  ReleaseProfile
} from "@/shared/model/demo";

const versions = (edgeVersion: string, webVersion = "1.8.2") => [
  { service: "device-edge", image: "inx/device-edge", expectedVersion: edgeVersion, observedVersion: edgeVersion, observedAt: "今天 10:26" },
  { service: "rule-engine", image: "inx/rule-engine", expectedVersion: "2.4.1", observedVersion: "2.4.1", observedAt: "今天 10:26" },
  { service: "device-edge-web", image: "inx/device-edge-web", expectedVersion: webVersion, observedVersion: webVersion, observedAt: "今天 10:26" },
  { service: "emqx", image: "emqx/emqx", expectedVersion: "5.8.6", observedVersion: "5.8.6", observedAt: "今天 10:26" }
];

export const demoProjects: DemoProject[] = [
  {
    id: "project-shenzhen-bay",
    name: "深圳湾智慧园区",
    code: "SZBAY",
    platformUrl: "https://iot.szb.example.com",
    databaseHost: "10.20.1.18:3306",
    databaseName: "inxvision_iot_szb",
    connectionState: "ready",
    username: "实施管理员"
  },
  {
    id: "project-chengdu-center",
    name: "成都金融中心",
    code: "CDFC",
    platformUrl: "https://iot.cdfc.example.com",
    databaseHost: "10.30.8.26:3306",
    databaseName: "inxvision_iot_cdfc",
    connectionState: "login_required"
  },
  {
    id: "project-lab",
    name: "研发联调环境",
    code: "LAB",
    platformUrl: "http://192.168.3.6:8055",
    databaseHost: "192.168.3.6:3306",
    databaseName: "inxvision_iot_dev",
    connectionState: "offline"
  }
];

export const demoNodes: EdgeNode[] = [
  {
    mac: "00:0C:29:3B:B9:31",
    name: "AIO-1F-弱电间",
    ip: "10.20.12.31",
    location: "A栋 1F 弱电间",
    managementState: "managed",
    deployLabel: "已部署",
    platformState: "online",
    platformUpdatedAt: "28秒前",
    serviceState: "healthy",
    serviceLabel: "4项正常",
    lastOperation: "整包升级 · 08/25 18:42",
    platformId: "1829038412681187329",
    versions: versions("3.2.7")
  },
  {
    mac: "00:0C:29:3B:B9:32",
    name: "AIO-2F-弱电间",
    ip: "10.20.12.32",
    location: "A栋 2F 弱电间",
    managementState: "managed",
    deployLabel: "已部署",
    platformState: "online",
    platformUpdatedAt: "41秒前",
    serviceState: "healthy",
    serviceLabel: "4项正常",
    lastOperation: "整包升级 · 08/25 18:42",
    platformId: "1829038412681187330",
    versions: versions("3.2.7")
  },
  {
    mac: "00:0C:29:3B:B9:33",
    name: "AIO-3F-弱电间",
    ip: "10.20.12.33",
    location: "A栋 3F 弱电间",
    managementState: "managed",
    deployLabel: "已升级",
    platformState: "online",
    platformUpdatedAt: "1分钟前",
    serviceState: "warning",
    serviceLabel: "1项需检查",
    lastOperation: "单服升级 · 今天 09:18",
    platformId: "1829038412681187331",
    versions: versions("3.2.8")
  },
  {
    mac: "00:0C:29:3B:B9:34",
    name: "AIO-B1-设备间",
    ip: "10.20.12.34",
    location: "A栋 B1 设备间",
    managementState: "managed",
    deployLabel: "已部署",
    platformState: "offline",
    platformUpdatedAt: "23分钟前",
    serviceState: "unreachable",
    serviceLabel: "暂不可达",
    lastOperation: "首次部署 · 08/19 14:06",
    platformId: "1829038412681187332",
    versions: versions("3.2.7")
  },
  {
    mac: "00:0C:29:3B:B9:35",
    name: "AIO-B栋-1F",
    ip: "10.20.13.21",
    location: "B栋 1F 弱电间",
    managementState: "pending",
    deployLabel: "待实施",
    platformState: "unknown",
    platformUpdatedAt: "尚未注册",
    serviceState: "unknown",
    serviceLabel: "待检查",
    lastOperation: "无工作台历史",
    versions: versions("—", "—")
  },
  {
    mac: "00:0C:29:3B:B9:36",
    name: "AIO-B栋-2F",
    ip: "10.20.13.22",
    location: "B栋 2F 弱电间",
    managementState: "platform_existing",
    deployLabel: "平台已存在",
    platformState: "online",
    platformUpdatedAt: "35秒前",
    serviceState: "unknown",
    serviceLabel: "等待接管",
    lastOperation: "无工作台历史",
    platformId: "1829038412681187341",
    versions: versions("3.1.9", "1.8.0")
  },
  {
    mac: "00:0C:29:3B:B9:37",
    name: "AIO-B栋-3F",
    ip: "10.20.13.23",
    location: "B栋 3F 弱电间",
    managementState: "conflict",
    deployLabel: "信息冲突",
    platformState: "online",
    platformUpdatedAt: "2分钟前",
    serviceState: "unknown",
    serviceLabel: "禁止执行",
    lastOperation: "导入匹配 · 今天 10:08",
    platformId: "1829038412681187342",
    versions: versions("3.2.6")
  },
  {
    mac: "00:0C:29:3B:B9:38",
    name: "AIO-能源站",
    ip: "10.20.20.10",
    location: "能源站控制室",
    managementState: "managed",
    deployLabel: "已部署",
    platformState: "online",
    platformUpdatedAt: "18秒前",
    serviceState: "healthy",
    serviceLabel: "4项正常",
    lastOperation: "整包升级 · 08/25 18:42",
    platformId: "1829038412681187350",
    versions: versions("3.2.7")
  }
];

export const demoReleaseProfile: ReleaseProfile = {
  version: 12,
  updatedBy: "实施管理员",
  updatedAt: "2026-08-26 16:42",
  platformHost: "10.20.1.18",
  platformApi: "http://10.20.1.18:8055",
  platformAuthKey: "szbay-edge-auth-2026",
  platformMqttHost: "10.20.1.19",
  platformMqttPort: 1883,
  platformMqttUsername: "edge_platform",
  platformMqttPassword: "mqtt-platform-2026",
  aioMqttUsername: "aio_local",
  aioMqttPassword: "aio-local-2026",
  sshUsername: "root",
  sshPassword: "inx-edge-ssh",
  sshPrivateKey: "",
  envTemplate: `# 项目公共参数\nPLATFORM_HOST={{ project.platform_host }}\nPLATFORM_API={{ project.platform_api }}\nPLATFORM_MQTT_HOST={{ mqtt.platform_host }}\nPLATFORM_MQTT_PORT={{ mqtt.platform_port }}\nAIO_AUTH_KEY={{ project.auth_key }}\nAIO_NAME={{ node.name }}\nAIO_IP={{ node.ip }}\nAIO_MAC={{ node.mac }}\nDEVICE_EDGE_IMAGE={{ image.device_edge }}\nRULE_ENGINE_IMAGE={{ image.rule_engine }}`,
  composeTemplate: `services:\n  device-edge:\n    image: \${DEVICE_EDGE_IMAGE}\n    restart: always\n    env_file: .env\n    volumes:\n      - /opt/data/device-edge:/opt/data/device-edge\n  rule-engine:\n    image: \${RULE_ENGINE_IMAGE}\n    restart: always\n    env_file: .env\n    volumes:\n      - /opt/data/rule-engine:/opt/data/rule-engine`
};

export const demoTasks: DemoTask[] = [
  {
    id: "task-history-1",
    projectId: "project-shenzhen-bay",
    name: "A栋一体机整包升级",
    mode: "full_upgrade",
    state: "succeeded",
    stage: "已完成",
    progress: 100,
    targetCount: 4,
    completedCount: 4,
    updatedAt: "昨天 18:42"
  }
];

export const demoLogs: DemoLogEntry[] = [
  { id: "log-1", taskId: "task-history-1", time: "18:41:22", level: "INFO", source: "10.20.12.31", message: "服务健康检查通过，4 个容器运行正常" },
  { id: "log-2", taskId: "task-history-1", time: "18:41:36", level: "INFO", source: "10.20.12.32", message: "发布版本 2026.08.25 已生效" },
  { id: "log-3", taskId: "task-history-1", time: "18:42:04", level: "INFO", source: "批次", message: "4/4 台一体机升级成功，结果已记录到项目侧" }
];

export const demoHistory: OperationHistoryItem[] = [
  { id: "OP-20260825-004", type: "整包升级", operator: "实施管理员", targetSummary: "4 台 · A栋", artifact: "Release 2026.08.25", result: "成功", finishedAt: "2026-08-25 18:42" },
  { id: "OP-20260821-003", type: "单服升级", operator: "陈工", targetSummary: "1 台 · AIO-3F-弱电间", artifact: "device-edge 3.2.8", result: "成功", finishedAt: "2026-08-21 09:18" },
  { id: "OP-20260819-002", type: "首次部署", operator: "实施管理员", targetSummary: "3 台 · A栋", artifact: "Release 2026.08.18", result: "部分成功", finishedAt: "2026-08-19 14:06" }
];

