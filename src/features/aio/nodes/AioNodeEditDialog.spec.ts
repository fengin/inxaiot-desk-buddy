import { defineComponent, h, ref } from "vue";
import { mount, flushPromises } from "@vue/test-utils";
import { NMessageProvider } from "naive-ui";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import type { AioNodeDetail } from "@/shared/model/aio";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";
import AioNodeEditDialog from "./AioNodeEditDialog.vue";

beforeEach(() => Object.defineProperty(window,"matchMedia",{configurable:true,value:vi.fn(() => ({matches:false,addEventListener:vi.fn(),removeEventListener:vi.fn()}))}));
function render(adapter: FixtureAioAdapter, detail: AioNodeDetail) {
  configureAioAdapter(adapter);
  const show=ref(true),projectId=ref("a"),saved=vi.fn();
  const wrapper=mount(defineComponent({setup:()=>()=>h(NMessageProvider,{}, {default:()=>h(AioNodeEditDialog,{show:show.value,projectId:projectId.value,detail,"onUpdate:show":(value:boolean)=>{show.value=value;},onSaved:saved})})}),{attachTo:document.body,global:{stubs:{teleport:true}}});
  return {wrapper,show,projectId,saved};
}
async function localDetail(adapter: FixtureAioAdapter) {
  const preview=await adapter.previewCreate("a",{name:"待实施一体机",ip:"192.0.2.7",mac:"001122334477",addrAlias:"旧机柜"});
  await adapter.applyImport("a",preview.session.id);
  return adapter.getNodeDetail("a","00:11:22:33:44:77");
}
describe("一体机详情编辑",()=>{
  it("未注册资料只更新当前项目本机记录，空间ID和具体位置分开提交",async()=>{
    const adapter=new FixtureAioAdapter(), detail=await localDetail(adapter), context=render(adapter,detail), save=vi.spyOn(adapter,"updateNode");
    try {
      await flushPromises();
      const spaces=await adapter.listSpaces("a"), id=spaces.at(-1)!.id;
      context.wrapper.getComponent(ProjectSpaceSelect).vm.$emit("update:modelValue",id);
      await context.wrapper.get('[data-testid="aio-edit-name"] input').setValue("修改后的名称");
      await context.wrapper.get('[data-testid="aio-edit-ip"] input').setValue("192.0.2.8");
      await context.wrapper.get('[data-testid="aio-edit-address"] input').setValue("门口机柜左侧");
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click"); await flushPromises();
      expect(save).toHaveBeenCalledWith("a",expect.objectContaining({platformBase:null,values:expect.objectContaining({buildingId:id,addrAlias:"门口机柜左侧"})}));
      expect((await adapter.getNodeDetail("a",detail.node.mac)).node).toMatchObject({name:"修改后的名称",ip:"192.0.2.8",location:"门口机柜左侧",buildingId:id});
      expect((await adapter.listNodes("b",{page:1,pageSize:100})).items.some(node=>node.mac===detail.node.mac)).toBe(false);
      expect(context.saved).toHaveBeenCalledOnce(); expect(context.show.value).toBe(false);
    } finally {context.wrapper.unmount();}
  });
  it("已注册资料携带平台原始编号，允许清空可选空间和位置",async()=>{
    const adapter=new FixtureAioAdapter(); const page=await adapter.listNodes("a",{page:1,pageSize:100});
    const node=page.items.find(node=>node.platformId)!; const detail=await adapter.getNodeDetail("a",node.mac);
    const context=render(adapter,detail), save=vi.spyOn(adapter,"updateNode");
    try {
      await flushPromises(); expect(context.wrapper.text()).toContain("保存后直接更新平台资料");
      context.wrapper.getComponent(ProjectSpaceSelect).vm.$emit("update:modelValue",null);
      await context.wrapper.get('[data-testid="aio-edit-address"] input').setValue("");
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click"); await flushPromises();
      expect(save).toHaveBeenCalledWith("a",expect.objectContaining({platformBase:expect.objectContaining({id:node.platformId}),values:expect.objectContaining({buildingId:undefined,addrAlias:undefined})}));
      expect((await adapter.getNodeDetail("a",node.mac)).platform?.addrAlias).toBe("");
    } finally {context.wrapper.unmount();}
  });
  it("保存失败保留表单；切换项目后旧保存响应不刷新新项目",async()=>{
    const adapter=new FixtureAioAdapter(), detail=await localDetail(adapter), context=render(adapter,detail);
    const save=vi.spyOn(adapter,"updateNode").mockRejectedValueOnce(new Error("IP 已被占用"));
    try {
      await flushPromises(); await context.wrapper.get('[data-testid="aio-edit-name"] input').setValue("待保存名称");
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click"); await flushPromises();
      expect(context.wrapper.get('[data-testid="aio-edit-error"]').text()).toContain("IP 已被占用"); expect(context.show.value).toBe(true);
      let finish!:()=>void; save.mockImplementationOnce(()=>new Promise<void>(resolve=>{finish=resolve;}));
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click");
      context.projectId.value="b"; context.show.value=false; await flushPromises(); finish(); await flushPromises();
      expect(context.saved).not.toHaveBeenCalled();
    } finally {context.wrapper.unmount();}
  });
});
