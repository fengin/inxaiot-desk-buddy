import type { AioNodeListItem, ServiceObservation, ServiceVersionRecord } from "@/shared/model/aio";

type Tone = "default" | "success" | "warning" | "error";

export function serviceCheckCoverage(node: AioNodeListItem) {
  const snapshot = node.serviceCheck;
  const expected = snapshot?.expectedServices ?? nodeServiceRows(node).map((row) => row.serviceName);
  const checked = expected.filter((name) => snapshot?.services.some((item) => item.serviceName === name && item.state !== "unknown")).length;
  return { checked, total: expected.length };
}

export function observationPresentation(observation?: ServiceObservation): { label: string; tone: Tone } {
  if (!observation) return { label: "未检查", tone: "default" };
  switch (observation.state) {
    case "normal": return { label: "正常", tone: "success" };
    case "abnormal": return { label: "异常", tone: "error" };
    case "version_mismatch": return { label: "版本偏差", tone: "warning" };
    default: return { label: "未知", tone: "default" };
  }
}

export function nodeServicePresentation(node: AioNodeListItem): { label: string; tone: Tone } {
  const snapshot = node.serviceCheck;
  if (snapshot?.lastAttempt?.state === "failed") return { label: "检查失败", tone: "error" };
  if (!snapshot) return { label: "无检查记录", tone: "default" };
  const observations = nodeServiceRows(node).flatMap((row) => row.observation ? [row.observation] : []);
  if (!observations.length && !snapshot.expectedServices?.length) return { label: "无检查记录", tone: "default" };
  const abnormal = observations.filter((item) => item.state === "abnormal").length;
  const mismatch = observations.filter((item) => item.state === "version_mismatch").length;
  if (abnormal || mismatch) return {
    label: [abnormal ? `${abnormal}项异常` : "", mismatch ? `${mismatch}项版本不符` : ""].filter(Boolean).join("、"),
    tone: "warning"
  };
  const coverage = serviceCheckCoverage(node);
  if (coverage.checked < coverage.total) return { label: `已检查${coverage.checked}/${coverage.total}项`, tone: "default" };
  return { label: `${coverage.total}项正常`, tone: "success" };
}

export function deployedImage(version?: ServiceVersionRecord) {
  if (!version) return "未记录";
  if (!version.expectedImageName) return version.expectedVersion || "未记录";
  if (!version.expectedVersion) return version.expectedImageName;
  const name = version.expectedImageName;
  return name.includes("@") || name.slice(name.lastIndexOf("/") + 1).includes(":")
    ? name : `${name}:${version.expectedVersion}`;
}

export function nodeServiceRows(node: AioNodeListItem, versions = node.versions) {
  const observations = new Map((node.serviceCheck?.services ?? []).map((item) => [item.serviceName, item]));
  const recorded = new Map(versions.map((item) => [item.serviceName, item]));
  const effectiveServices = node.serviceCheck?.expectedServices ?? [...recorded.keys(), ...observations.keys()];
  return [...new Set(effectiveServices)].map((serviceName) => ({
    serviceName,
    deployedImage: deployedImage(recorded.get(serviceName)),
    observation: observations.get(serviceName),
    ...observationPresentation(observations.get(serviceName))
  }));
}

export function serviceCheckSourceLabel(source?: string) {
  if (!source) return "未检查";
  return ({
    manual: "手动检查", service_inspection: "手动检查", preflight: "部署前检查",
    deployment_preflight: "部署前检查", first_deploy: "首次部署后检查",
    full_upgrade: "整包升级后检查", service_upgrade: "单服升级后检查",
    deployment: "部署后检查", post_deploy: "部署后检查", rollback: "回滚后检查"
  } as Record<string, string>)[source] ?? source;
}

export function serviceRuntimeLabel(observation?: ServiceObservation) {
  if (!observation) return "未采集";
  const runtime = ({
    running: "运行中", exited: "已退出", stopped: "已停止", missing: "容器缺失",
    restarting: "重启中", paused: "已暂停", created: "已创建", dead: "已停止", unknown: "未知"
  } as Record<string, string>)[observation.runtimeState] ?? observation.runtimeState;
  const health = ({ healthy: "健康", unhealthy: "不健康", starting: "启动中", none: "未配置健康检查" } as Record<string, string>)[observation.healthStatus ?? ""] ?? observation.healthStatus;
  return health ? `${runtime} · ${health}` : runtime || "未知";
}

export function serviceInspectionStageLabel(stage?: string) {
  return ({
    queued: "等待检查", running: "检查服务中", prepare_target: "读取一体机信息",
    ssh_connect: "连接一体机", inspect_services: "读取服务和镜像", service_inspection: "读取服务和镜像",
    service_check: "读取服务和镜像", finalizing: "保存本机检查结果", finalizing_failed: "本机检查结果待补写",
    completed: "检查完成", succeeded: "检查完成", failed: "检查失败", interrupted: "检查已中断", cancelled: "检查已取消"
  } as Record<string, string>)[stage ?? ""] ?? (stage && /[㐀-鿿]/u.test(stage) ? stage : "正在准备检查");
}

export function projectVersionRows(nodes: AioNodeListItem[]) {
  const rows = new Map<string, { service: string; versions: Map<string, number>; recorded: number; checked: number; deviations: number; unrecorded: number }>();
  for (const node of nodes) {
    for (const service of nodeServiceRows(node)) {
      const row = rows.get(service.serviceName) ?? { service: service.serviceName, versions: new Map(), recorded: 0, checked: 0, deviations: 0, unrecorded: 0 };
      const version = node.versions.find((item) => item.serviceName === service.serviceName);
      const distribution = version?.expectedVersion || version?.expectedImageName;
      if (distribution) {
        row.versions.set(distribution, (row.versions.get(distribution) ?? 0) + 1);
        row.recorded += 1;
      } else row.unrecorded += 1;
      if (service.observation && service.observation.state !== "unknown") row.checked += 1;
      if (service.observation?.state === "version_mismatch") row.deviations += 1;
      rows.set(service.serviceName, row);
    }
  }
  return [...rows.values()];
}
