import { createPinia } from "pinia";
import { flushPromises, mount } from "@vue/test-utils";
import { NCheckbox } from "naive-ui";
import { expect, it, vi } from "vitest";
import { router } from "@/app/router";
import { i18n } from "@/app/i18n";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { useAioNodesStore } from "@/stores/aioNodes";
import { useProjectStore } from "@/stores/projects";
import { usePreferencesStore } from "@/stores/preferences";

async function renderList() {
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn(() => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() })) });
  const adapter = new FixtureAioAdapter();
  const base = await adapter.listNodes("test-project", {page:1,pageSize:20});
  const all = Array.from({length:45}, (_, index) => ({ ...base.items[0]!, name:`一体机${index}`, macNormalized:index.toString(16).padStart(12,"0"), mac:index.toString(16).padStart(12,"0"), managementState:"managed", conflicts:[] }));
  vi.spyOn(adapter, "listNodes").mockImplementation(async (_, query) => {
    const filtered = all.filter(node => !query.search || node.name.includes(query.search));
    return {...base, items:filtered.slice((query.page-1)*query.pageSize,query.page*query.pageSize), total:filtered.length};
  });
  const detail = vi.spyOn(adapter,"getNodeDetail");
  configureAioAdapter(adapter); configureActivityAdapter(new FixtureActivityAdapter()); configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
  const {default: App} = await import("@/app/App.vue");
  await router.push("/aio/nodes"); await router.isReady();
  const pinia = createPinia(); usePreferencesStore(pinia).pageSize=20;
  const wrapper = mount(App,{attachTo:document.body,global:{plugins:[pinia,router,i18n],stubs:{teleport:true}}});
  await flushPromises();
  return {wrapper,detail,all,aio:useAioNodesStore(pinia),projects:useProjectStore(pinia)};
}

it("当前页全选、单选和跨页保留，批量按钮只传所选目标且勾选不打开详情", async () => {
  const {wrapper,detail,all,aio,projects}=await renderList();
  const push=vi.spyOn(router,"push").mockResolvedValue();
  try {
    expect(wrapper.get('[data-testid="aio-batch-operations"]').attributes("disabled")).toBeDefined();
    const header=wrapper.get(".node-table-header").getComponent(NCheckbox);
    header.vm.$emit("update:checked",true);await flushPromises();
    expect(wrapper.get(".asset-footer-summary").text()).toContain("已选 20 台");
    await wrapper.get(".node-table tbody tr .n-checkbox").trigger("click");await flushPromises();
    expect(wrapper.get(".asset-footer-summary").text()).toContain("已选 19 台");
    expect(header.props("indeterminate")).toBe(true);expect(detail).not.toHaveBeenCalled();
    aio.page=2;await flushPromises();
    expect(wrapper.get(".node-table tbody tr").attributes("data-node-mac")).toBe(all[20]!.macNormalized);
    expect(header.props("checked")).toBe(false);
    await wrapper.get(".node-table tbody tr .n-checkbox").trigger("click");await flushPromises();
    await wrapper.get('[data-testid="aio-batch-operations"]').trigger("click");
    expect(push).toHaveBeenCalledExactlyOnceWith({name:"aio-operations",query:{targets:all.slice(1,21).map(node=>node.macNormalized),project:projects.activeProjectId}});
  } finally {push.mockRestore();wrapper.unmount();}
},30000);

it("筛选保留选择，切项目清空，冲突记录不参与全选",async()=>{
  const {wrapper,aio,projects}=await renderList();
  try {
    aio.nodes[0]!.managementState="conflict";
    await flushPromises();
    expect(wrapper.get(".node-table tbody tr").getComponent(NCheckbox).props("disabled")).toBe(true);
    wrapper.get(".node-table-header").getComponent(NCheckbox).vm.$emit("update:checked",true);await flushPromises();
    expect(wrapper.get(".asset-footer-summary").text()).toContain("已选 19 台");
    await wrapper.get(".search-input input").setValue("一体机44");
    await vi.waitFor(()=>expect(wrapper.findAll(".node-table tbody tr")).toHaveLength(1));
    expect(wrapper.get(".asset-footer-summary").text()).toContain("已选 19 台");
    const originalProject = projects.activeProjectId;
    await projects.switchProject("project-chengdu-center");await flushPromises();
    await projects.switchProject(originalProject);await flushPromises();
    expect(wrapper.get('[data-testid="aio-batch-operations"]').attributes("disabled")).toBeDefined();
    expect(wrapper.get(".asset-footer-summary").text()).toContain("已选 0 台");
    const buttons=wrapper.findAll(".page-actions button").map(button=>button.text());
    expect(buttons.indexOf("新增一体机")).toBe(buttons.indexOf("导入清单")+1);
  }finally {wrapper.unmount();}
},30000);
