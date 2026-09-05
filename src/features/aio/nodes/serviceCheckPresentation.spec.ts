import { describe, expect, it } from "vitest";
import type { AioNodeListItem, ServiceObservation } from "@/shared/model/aio";
import { nodeServicePresentation, nodeServiceRows, projectVersionRows, serviceCheckCoverage } from "./serviceCheckPresentation";

function node(): AioNodeListItem {
  return {
    mac: "00:11:22:33:44:55", macNormalized: "001122334455", name: "测试一体机", ip: "192.0.2.1", location: "",
    managementState: "managed", deployLabel: "已管理", platformState: "online", platformUpdatedAt: "2026-09-01T00:00:00Z",
    serviceState: "healthy", serviceLabel: "正常", lastOperation: "整包升级", source: "merged", version: 1, conflicts: [],
    versions: ["device-edge", "rule-engine", "web", "emqx"].map((serviceName) => ({
      macNormalized: "001122334455", serviceName, expectedImageName: `inx/${serviceName}`, expectedVersion: "1.0",
      observedImageName: `inx/${serviceName}`, observedVersion: "1.0", observedAt: "2026-09-01T00:00:00Z"
    }))
  };
}

function observation(serviceName: string, state: ServiceObservation["state"] = "normal"): ServiceObservation {
  return { serviceName, state, runtimeState: "running", actualImage: `inx/${serviceName}:1.0`, checkedAt: "2026-09-01T00:00:00Z", source: "full_upgrade" };
}

describe("服务检查事实展示", () => {
  it("历史版本和平台在线均不能冒充真实服务检查", () => {
    const item = node();
    expect(nodeServicePresentation(item)).toEqual({ label: "无检查记录", tone: "default" });
    expect(nodeServiceRows(item).every((row) => !row.observation && row.label === "未检查")).toBe(true);
    expect(serviceCheckCoverage(item)).toEqual({ checked: 0, total: 4 });
    expect(projectVersionRows([item]).every((row) => row.checked === 0 && row.recorded === 1)).toBe(true);
  });

  it("历史真实观测不会因超过十五分钟自动变成故障", () => {
    const item = node();
    item.serviceCheck = { expectedServices: item.versions.map((version) => version.serviceName), services: item.versions.map((version) => observation(version.serviceName)), lastFullCheckAt: "2026-09-01T00:00:00Z" };
    expect(nodeServicePresentation(item)).toEqual({ label: "4项正常", tone: "success" });
  });

  it("单服检查保留其他服务时间，不填充整机检查时间", () => {
    const item = node();
    item.serviceCheck = { expectedServices: item.versions.map((version) => version.serviceName), services: [observation("device-edge")] };
    expect(nodeServicePresentation(item).label).toBe("已检查1/4项");
    expect(nodeServiceRows(item).filter((row) => row.observation)).toHaveLength(1);
    expect(item.serviceCheck.lastFullCheckAt).toBeUndefined();
  });

  it("服务异常、版本偏差以及采集失败分别展示", () => {
    const item = node();
    item.serviceCheck = { services: [observation("device-edge", "version_mismatch")] };
    expect(nodeServicePresentation(item)).toEqual({ label: "1项版本不符", tone: "warning" });
    item.serviceCheck.services.push(observation("rule-engine", "abnormal"));
    expect(nodeServicePresentation(item)).toEqual({ label: "1项异常、1项版本不符", tone: "warning" });
    item.serviceCheck.lastAttempt = { startedAt: "2026-09-05T00:00:00Z", checkedAt: "2026-09-05T00:00:01Z", source: "manual", scope: "all", services: [], state: "failed", error: "读取失败" };
    expect(nodeServicePresentation(item).label).toBe("检查失败");
    expect(nodeServiceRows(item)[0]?.observation?.checkedAt).toBe("2026-09-01T00:00:00Z");
    expect(nodeServiceRows(item).find((row) => row.serviceName === "rule-engine")?.label).toBe("异常");
  });

  it("多种已部署版本保持中性分布，只计真实观测偏差", () => {
    const first = node();
    const second = node();
    second.versions[0]!.expectedVersion = "2.0";
    second.serviceCheck = { services: [observation("device-edge", "version_mismatch")] };
    const row = projectVersionRows([first, second])[0]!;
    expect([...row.versions]).toEqual([["1.0", 1], ["2.0", 1]]);
    expect(row.recorded).toBe(2);
    expect(row.checked).toBe(1);
    expect(row.deviations).toBe(1);
  });

  it("未知结果不计入已检查，移除的旧服务不参与当前异常聚合", () => {
    const item = node();
    item.serviceCheck = { expectedServices: ["device-edge"], services: [observation("device-edge"), observation("removed-service", "abnormal")], lastFullCheckAt: "2026-09-01T00:00:00Z" };
    expect(nodeServicePresentation(item).label).toBe("1项正常");
    expect(serviceCheckCoverage(item)).toEqual({ checked: 1, total: 1 });
    expect(nodeServiceRows(item).map((row) => row.serviceName)).toEqual(["device-edge"]);
    expect(projectVersionRows([item]).map((row) => row.service)).toEqual(["device-edge"]);
    item.serviceCheck.services[0]!.state = "unknown";
    expect(serviceCheckCoverage(item).checked).toBe(0);
    expect(nodeServicePresentation(item).label).toBe("已检查0/1项");
  });

  it("分别检查全部服务后按逐项事实展示正常，不要求曾进行整机检查", () => {
    const item = node();
    item.serviceCheck = {
      expectedServices: item.versions.map((version) => version.serviceName),
      services: item.versions.map((version, index) => ({ ...observation(version.serviceName), checkedAt: `2026-09-0${index + 1}T00:00:00Z`, source: "service_upgrade" }))
    };
    expect(nodeServicePresentation(item)).toEqual({ label: "4项正常", tone: "success" });
    expect(item.serviceCheck.lastFullCheckAt).toBeUndefined();
    expect(new Set(nodeServiceRows(item).map((row) => row.observation?.checkedAt)).size).toBe(4);
  });

  it("生效服务集合显式为空时不重新引入已删除服务的历史版本和观测", () => {
    const item = node();
    item.serviceCheck = { expectedServices: [], services: [observation("device-edge", "abnormal")] };
    expect(nodeServiceRows(item)).toEqual([]);
    expect(serviceCheckCoverage(item)).toEqual({ checked: 0, total: 0 });
    expect(projectVersionRows([item])).toEqual([]);
  });
});
