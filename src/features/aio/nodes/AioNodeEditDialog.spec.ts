import { defineComponent, h, ref } from "vue";
import { mount, flushPromises } from "@vue/test-utils";
import { NMessageProvider, NPopconfirm } from "naive-ui";
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
  it("打开时保留自定义具体位置，换空间填建议，清空重输期间不回填且空白不能保存", async () => {
    const adapter = new FixtureAioAdapter(), detail = await localDetail(adapter);
    detail.node.buildingId = "floor-a-1";
    const context = render(adapter, detail), save = vi.spyOn(adapter, "updateNode");
    try {
      await flushPromises();
      const address = context.wrapper.get('[data-testid="aio-edit-address"] input');
      expect(address.element).toHaveProperty("value", "旧机柜");
      context.wrapper.getComponent(ProjectSpaceSelect).vm.$emit("update:modelValue", null); await flushPromises();
      expect(address.element).toHaveProperty("value", "旧机柜");
      context.wrapper.getComponent(ProjectSpaceSelect).vm.$emit("update:modelValue", "floor-b-2"); await flushPromises();
      expect(address.element).toHaveProperty("value", "B座_2F");
      await address.setValue(""); await flushPromises();
      expect(address.element).toHaveProperty("value", "");
      await address.setValue("　 ");
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click"); await flushPromises();
      expect(context.wrapper.get('[data-testid="aio-edit-error"]').text()).toContain("已选择空间，请填写具体位置");
      expect(save).not.toHaveBeenCalled();
      await address.setValue("新的自定义机柜");
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click"); await flushPromises();
      expect(save).toHaveBeenCalledWith("a", expect.objectContaining({ values: expect.objectContaining({ buildingId: "floor-b-2", addrAlias: "新的自定义机柜" }) }));
    } finally { context.wrapper.unmount(); }
  });

  it("历史记录有空间而位置空白，读取目录后补建议并可明确保存", async () => {
    const adapter = new FixtureAioAdapter(), detail = await localDetail(adapter);
    detail.node.buildingId = "area-a-1-room"; detail.node.location = "  ";
    const context = render(adapter, detail), save = vi.spyOn(adapter, "updateNode");
    try {
      await flushPromises();
      expect(context.wrapper.get('[data-testid="aio-edit-address"] input').element).toHaveProperty("value", "A座_1F");
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click"); await flushPromises();
      expect(save).toHaveBeenCalledWith("a", expect.objectContaining({ values: expect.objectContaining({ buildingId: "area-a-1-room", addrAlias: "A座_1F" }) }));
    } finally { context.wrapper.unmount(); }
  });

  it("空间目录迟到时不覆盖用户正在清空重写的位置", async () => {
    const adapter = new FixtureAioAdapter(), detail = await localDetail(adapter);
    detail.node.buildingId = "floor-a-1"; detail.node.location = "";
    const spaces = await adapter.listSpaces("a");
    let finish!: (value: typeof spaces) => void;
    vi.spyOn(adapter, "listSpaces").mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    const context = render(adapter, detail);
    try {
      const address = context.wrapper.get('[data-testid="aio-edit-address"] input');
      await address.setValue("重写中"); await address.setValue("");
      finish(spaces); await flushPromises();
      expect(address.element).toHaveProperty("value", "");
    } finally { context.wrapper.unmount(); }
  });

  it.each([false, true])("接手刷新后正确合并空间及位置，已改空间=%s", async (changedSpace) => {
    const adapter = new FixtureAioAdapter(), detail = await localDetail(adapter);
    detail.node.buildingId = "floor-a-1"; detail.node.location = "";
    const fresh = structuredClone(detail);
    fresh.node.buildingId = "floor-b-2"; fresh.node.location = "平台新机柜"; fresh.node.version += 1;
    const context = render(adapter, detail);
    const save = vi.spyOn(adapter, "updateNode").mockRejectedValueOnce({ code: "AIO_EDIT_LOCKED" }).mockResolvedValue(undefined);
    vi.spyOn(adapter, "getNodeDetail").mockResolvedValue(fresh);
    try {
      await flushPromises();
      if (changedSpace) {
        context.wrapper.getComponent(ProjectSpaceSelect).vm.$emit("update:modelValue", "area-a-1-room"); await flushPromises();
      }
      await context.wrapper.get('[data-testid="aio-edit-name"] input').setValue("本次名称修改");
      await context.wrapper.get('[data-testid="aio-edit-save"]').trigger("click"); await flushPromises();
      context.wrapper.getComponent(NPopconfirm).vm.$emit("positiveClick"); await flushPromises();
      expect(save).toHaveBeenLastCalledWith("a", expect.objectContaining({ forceTakeover: true, expectedVersion: fresh.node.version,
        values: expect.objectContaining({ name: "本次名称修改", buildingId: changedSpace ? "area-a-1-room" : "floor-b-2",
          addrAlias: changedSpace ? "A座_1F" : "平台新机柜" }) }));
    } finally { context.wrapper.unmount(); }
  });

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
