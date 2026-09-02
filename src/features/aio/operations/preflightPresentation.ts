import type { DeploymentPreflightCheck, DeploymentPreflightReport } from "@/shared/model/deploymentWorkflow";

export interface PreflightNodeIdentity {
  macNormalized: string;
  name: string;
  ip: string;
}

export interface PreflightItem {
  key: string;
  label: string;
  status: "passed" | "warning" | "failed" | "pending";
  message: string;
  issues: DeploymentPreflightCheck[];
}

export interface PreflightGroup {
  key: string;
  title: string;
  subtitle?: string;
  items: PreflightItem[];
}

const connectionCodes = new Set([
  "deployment_target", "resource_lease", "ssh_auth", "platform_endpoints",
  "host_key_observed", "host_key_changed", "ssh_disconnect"
]);

function item(key: string, label: string, checks: DeploymentPreflightCheck[], required: string[], summary: string): PreflightItem {
  const issues = checks.filter((check) => check.status !== "passed");
  const complete = required.every((code) => checks.some((check) => check.code === code && check.status === "passed"));
  const failed = issues.some((check) => check.blocking && check.status === "failed");
  return {
    key, label, issues,
    status: failed ? "failed" : issues.length ? "warning" : complete ? "passed" : "pending",
    message: failed ? "请处理以下问题后重新检查" : !complete ? "尚未完成检查" : summary
  };
}

export function preflightGroups(report: DeploymentPreflightReport | undefined, nodes: PreflightNodeIdentity[]): PreflightGroup[] {
  if (!report) return [];
  const common = report.checks.filter((check) => !check.targetMac);
  const groups: PreflightGroup[] = [{
    key: "common", title: "公共检查", items: [
      item("artifact", "发布物（镜像文件）", common.filter((check) => check.code === "artifact"), ["artifact"], common.find((check) => check.code === "artifact")?.message ?? ""),
      item("profile", "发布参数", common.filter((check) => check.code !== "artifact"), ["release_profile", "release_endpoints"], common.find((check) => check.code === "release_endpoints")?.message ?? "")
    ]
  }];
  for (const mac of report.normalizedPlan.targetMacs) {
    const node = nodes.find((value) => value.macNormalized === mac)
      ?? report.executionSnapshot?.targets.find((target) => target.node.macNormalized === mac)?.node;
    const checks = report.checks.filter((check) => check.targetMac === mac);
    groups.push({
      key: mac, title: node?.name || node?.ip || "待检查一体机", subtitle: node?.ip,
      items: [
        item("connectivity", "连通性", checks.filter((check) => connectionCodes.has(check.code)), ["ssh_auth", "platform_endpoints"], "SSH 连接、登录验证及一体机到平台 API/MQTT 连通性通过"),
        item("environment", "环境准备", checks.filter((check) => !connectionCodes.has(check.code)), ["runtime_os", "runtime_arch", "docker", "docker_compose", "remote_storage", "remote_ports"], "系统、Docker、Compose、目录、空间和服务端口检查通过")
      ]
    });
  }
  return groups;
}
