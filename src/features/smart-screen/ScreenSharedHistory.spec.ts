import { flushPromises, mount } from '@vue/test-utils';
import { afterEach, expect, it, vi } from 'vitest';
import ScreenSharedHistory from './ScreenSharedHistory.vue';
import * as api from '@/shared/api/operationHistory';

vi.mock('@/shared/api/operationHistory', () => ({listBusinessOperationHistory:vi.fn(),getBusinessOperationHistoryDetail:vi.fn(),previewScreenLocks:vi.fn(),releaseScreenLocks:vi.fn()}));
afterEach(()=>vi.clearAllMocks());
const operation={id:'op-a',domainType:'smart_screen',operationType:'reboot',operationName:'重启屏',operatorName:'原操作人',instanceId:'电脑A',state:'running',targetCount:1,successCount:0,failureCount:0,cancelledCount:0,startedAt:'2026-10-03T00:00:00Z'};
const locks=[{resourceType:'smart_screen',resourceKey:'777:900',ownerInstanceId:'电脑A',ownerUser:'原操作人',fencingToken:4}];
it('配置共享结果显示修改字段、允许共享的前后值及三个处理阶段',async()=>{
  const item={...operation,operationType:'app_config',operationName:'小新配置',state:'succeeded'};
  vi.mocked(api.listBusinessOperationHistory).mockResolvedValue({items:[item],total:1,page:1,pageSize:20});
  vi.mocked(api.getBusinessOperationHistoryDetail).mockResolvedValue({operation:item,targets:[{resourceType:'smart_screen',resourceKey:'777:900',state:'succeeded',resultSummary:'配置已保存',details:{targetName:'办公室屏',configuration:{fields:['customDeviceName','environments.pre.h5ReadyCheckEnabled'],save:'saved',restart:'succeeded',readback:'succeeded',changes:[{field:'customDeviceName',before:'旧名称',after:'新名称'}]}}}]});
  const wrapper=mount(ScreenSharedHistory,{props:{projectId:'项目A'},global:{stubs:{teleport:true}}});
  try{await flushPromises();await wrapper.findAll('button').find(button=>button.text().includes('小新配置'))!.trigger('click');await flushPromises();expect(wrapper.text()).toContain('旧名称 → 新名称');expect(wrapper.text()).toContain('预发环境 · H5检测');expect(wrapper.text()).toContain('保存：已保存');expect(wrapper.text()).not.toContain('应用版本');}finally{wrapper.unmount();}
});
async function setup(){
  vi.mocked(api.listBusinessOperationHistory).mockResolvedValue({items:[operation],total:1,page:1,pageSize:20});
  vi.mocked(api.getBusinessOperationHistoryDetail).mockResolvedValue({operation,targets:[]});
  vi.mocked(api.previewScreenLocks).mockResolvedValue(locks);
  vi.mocked(api.releaseScreenLocks).mockResolvedValue();
  const wrapper=mount(ScreenSharedHistory,{props:{projectId:'项目A'},global:{stubs:{teleport:true}}});
  await flushPromises();await wrapper.findAll('button').find(b=>b.text().includes('重启屏'))!.trigger('click');await flushPromises();
  return wrapper;
}
it('先展示原电脑和影响，再次确认才释放；保留原操作编号',async()=>{
  const wrapper=await setup();
  try{
    await wrapper.findAll('button').find(b=>b.text()==='查看并释放操作占用')!.trigger('click');await flushPromises();
    expect(api.releaseScreenLocks).not.toHaveBeenCalled();expect(wrapper.text()).toContain('释放不会撤销这些命令');expect(wrapper.text()).toContain('电脑A');
    await wrapper.findAll('button').find(b=>b.text()==='已确认，强制释放')!.trigger('click');await flushPromises();
    expect(api.releaseScreenLocks).toHaveBeenCalledExactlyOnceWith('项目A','op-a',locks);expect(wrapper.text()).toContain('占用已释放');
  }finally{wrapper.unmount();}
});
it('换项目后关闭旧确认框，不释放旧项目占用',async()=>{
  const wrapper=await setup();
  try{
    await wrapper.findAll('button').find(b=>b.text()==='查看并释放操作占用')!.trigger('click');await flushPromises();
    await wrapper.setProps({projectId:'项目B'});await flushPromises();
    expect(wrapper.findAll('button').some(b=>b.text()==='已确认，强制释放')).toBe(false);
    expect(api.releaseScreenLocks).not.toHaveBeenCalled();
  }finally{wrapper.unmount();}
});
