import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import type { DeploymentPlanInput } from "@/shared/model/release";
import type { DeploymentPreflightReport } from "@/shared/model/deploymentWorkflow";
import DeploymentPreflightGroups from "./DeploymentPreflightGroups.vue";
import { preflightGroups, preflightGroupPassed } from "./preflightPresentation";

const nodes = [
  { macNormalized: "000C290B71F4", name: "节点121", ip: "192.168.3.121" },
  { macNormalized: "000C293BB933", name: "节点79", ip: "192.168.3.79" }
];
const plan: DeploymentPlanInput = {
  mode: "first_deploy", targetMacs: nodes.map((node) => node.macNormalized),
  artifactPath: "D:/fixture", artifactName: "Release", artifactVersion: "fixture",
  batchSize: 2, concurrency: 2
};
const report = (): Promise<DeploymentPreflightReport> => new FixtureOperationsAdapter().preflight("project", plan);

describe("部署检查业务分组", () => {
  it("通过分组默认收起，状态图标仅在组名前，展开后检查项为普通文字列表", async () => {
    const value = await report();
    const wrapper = mount(DeploymentPreflightGroups, { props: { report: value, nodes } });
    expect(wrapper.findAll('.preflight-group')).toHaveLength(3);
    expect(wrapper.findAll('[data-testid="preflight-business-check"]')).toHaveLength(0);
    expect(wrapper.findAll('.preflight-group-status-icon[data-state="passed"]')).toHaveLength(3);
    expect(wrapper.findAll('.preflight-group-toggle').every((button) => button.attributes('aria-expanded') === 'false')).toBe(true);
    await wrapper.get('[data-testid="preflight-group-common"] .preflight-group-toggle').trigger('click');
    expect(wrapper.get('[data-testid="preflight-group-common"]').text()).toContain("发布物（镜像文件）");
    for (const node of nodes) {
      const group = wrapper.get('[data-testid="preflight-group-' + node.macNormalized + '"]');
      expect(group.text()).toContain(node.name);
      expect(group.text()).toContain(node.ip);
      await group.get('.preflight-group-toggle').trigger('click');
      expect(group.text()).toContain("连通性");
      expect(group.text()).toContain("环境准备");
    }
    expect(wrapper.findAll('[data-testid="preflight-business-check"]')).toHaveLength(6);
    expect(wrapper.findAll('.preflight-item-row svg')).toHaveLength(0);
    expect(wrapper.findAll('.preflight-item-row strong')).toHaveLength(0);
    expect(wrapper.findAll('.preflight-item-label').every((label) => label.element.tagName === 'SPAN')).toBe(true);
    for (const technical of ["平台会话", "项目数据库与Schema", "资源租约", "主机指纹", "CPU架构"]) {
      expect(wrapper.text()).not.toContain(technical);
    }
    wrapper.unmount();
  });

  it("一个节点失败不会污染另一个节点，也不会被折叠为成功", async () => {
    const value = await report();
    value.checks.push({ code: "runtime_arch", label: "运行环境兼容性", targetMac: nodes[0]!.macNormalized, status: "failed", blocking: true, message: "发布物与运行环境不匹配" });
    value.ready = false;
    const groups = preflightGroups(value, nodes);
    expect(groups[1]!.items[1]!.status).toBe("failed");
    expect(groups[1]!.items[1]!.issues[0]!.message).toContain("不匹配");
    expect(groups[2]!.items.every((item) => item.status === "passed")).toBe(true);
    const wrapper = mount(DeploymentPreflightGroups, { props: { report: value, nodes } });
    expect(wrapper.get('[data-testid="preflight-group-' + nodes[0]!.macNormalized + '"] .preflight-group-toggle').attributes('aria-expanded')).toBe('true');
    expect(wrapper.get('[data-testid="preflight-group-' + nodes[1]!.macNormalized + '"] .preflight-group-toggle').attributes('aria-expanded')).toBe('false');
    expect(wrapper.findAll('.preflight-group-status-icon[data-state="failed"]')).toHaveLength(1);
    expect(wrapper.findAll('.preflight-item-row')).toHaveLength(2);
    expect(wrapper.text()).toContain("发布物与运行环境不匹配");
    wrapper.unmount();
  });

  it("登录失败后未执行的环境检查必须显示未检查", async () => {
    const value = await report();
    value.checks = value.checks.filter((check) => !("targetMac" in check) || check.targetMac !== nodes[0]!.macNormalized);
    value.checks.push({ code: "ssh_auth", label: "SSH认证", targetMac: nodes[0]!.macNormalized, status: "failed", blocking: true, message: "登录失败，请检查凭据" });
    value.ready = false;
    const groups = preflightGroups(value, nodes);
    expect(groups[1]!.items[0]!.status).toBe("failed");
    expect(groups[1]!.items[1]!.status).toBe("pending");
    expect(preflightGroupPassed(groups[1]!)).toBe(false);
  });

  it("指纹变化只提示不增加检查项，隐藏的互斥检查异常仍可操作", async () => {
    const value = await report();
    value.checks.push({ code: "host_key_changed", label: "主机指纹变化", targetMac: nodes[0]!.macNormalized, status: "warning", blocking: false, message: "技术指纹详情" });
    value.checks.push({ code: "resource_lease", label: "资源租约", targetMac: nodes[1]!.macNormalized, status: "failed", blocking: true, message: "一体机正在执行其他任务", remediation: { action: "open_history", label: "查看占用操作" } });
    const wrapper = mount(DeploymentPreflightGroups, { props: { report: value, nodes } });
    expect(wrapper.findAll('[data-testid="preflight-business-check"]')).toHaveLength(2);
    const warningGroup = wrapper.get('[data-testid="preflight-group-' + nodes[0]!.macNormalized + '"]');
    expect(warningGroup.get('.preflight-group-status-icon').attributes('data-state')).toBe('passed');
    expect(warningGroup.get('.preflight-group-toggle').attributes('aria-expanded')).toBe('false');
    expect(warningGroup.text()).toContain('有提示');
    await warningGroup.get('.preflight-group-toggle').trigger('click');
    expect(wrapper.text()).toContain("连接信息与上次不同");
    expect(wrapper.text()).not.toContain("技术指纹详情");
    expect(wrapper.text()).toContain("正在执行其他任务");
    await wrapper.get('.preflight-issue button').trigger('click');
    expect(wrapper.emitted('remediate')?.[0]?.[0]).toMatchObject({ code: "resource_lease" });
    wrapper.unmount();
  });

  it("重新检查后按新结果重置折叠，名称刷新不覆盖用户手动展开", async () => {
    const value = await report();
    const wrapper = mount(DeploymentPreflightGroups, { props: { report: value, nodes } });
    const common = () => wrapper.get('[data-testid="preflight-group-common"] .preflight-group-toggle');
    await common().trigger('click');
    expect(common().attributes('aria-expanded')).toBe('true');
    await wrapper.setProps({ nodes: nodes.map((node) => ({ ...node, name: node.name + '更新' })) });
    expect(common().attributes('aria-expanded')).toBe('true');
    await wrapper.setProps({ report: await report() });
    expect(common().attributes('aria-expanded')).toBe('false');
    const failed = await report();
    failed.checks.push({ code: 'artifact', label: '发布物', status: 'failed', blocking: true, message: '镜像文件不可读' });
    failed.ready = false;
    await wrapper.setProps({ report: failed });
    expect(common().attributes('aria-expanded')).toBe('true');
    expect(wrapper.text()).toContain('镜像文件不可读');
    wrapper.unmount();
  });

  it("两百台全部通过时只渲染分组行，不挂载折叠的四百个检查项", async () => {
    const manyNodes = Array.from({ length: 200 }, (_, index) => ({ macNormalized: index.toString(16).padStart(12, '0'), name: '一体机' + index, ip: '192.0.2.' + (index + 1) }));
    const value = await new FixtureOperationsAdapter().preflight('project', { ...plan, targetMacs: manyNodes.map((node) => node.macNormalized) });
    const wrapper = mount(DeploymentPreflightGroups, { props: { report: value, nodes: manyNodes } });
    expect(wrapper.findAll('.preflight-group-toggle')).toHaveLength(201);
    expect(wrapper.findAll('.preflight-group-details')).toHaveLength(0);
    expect(wrapper.findAll('.preflight-group-status-icon[data-state="passed"]')).toHaveLength(201);
    wrapper.unmount();
  });
});
