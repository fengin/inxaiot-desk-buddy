import { flushPromises, mount } from '@vue/test-utils';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import ScreenTakeoverDialog from './ScreenTakeoverDialog.vue';
import { RealScreenAdapter, screenSnapshotFromBackend } from '@/shared/api/realScreenAdapter';
import { screenMaintenanceFingerprint } from '@/shared/model/screenMaintenance';
import type { ScreenOperationInput } from '@/shared/model/screen';

const {invoke}=vi.hoisted(()=>({invoke:vi.fn()}));
vi.mock('@tauri-apps/api/core',()=>({invoke}));
const raw=()=>({screens:[{id:'900',source:'platform' as const,name:'办公室屏',ip:'192.0.2.1',mac:'02:11:22:33:44:55',size:'4' as const,location:'门口',spaceId:'100',revision:1,appVersion:'2.0.8',platformStatus:'online' as const,aliases:[]}],spaces:[],observations:{},ignoredPairs:[],platformAvailable:true});
const conflict={operationId:'operation-a',operationName:'安装/升级小新',operationType:'install',targetCount:3,ownerUser:'张工',ownerInstanceId:'办公室电脑-001122AABBCC-192.0.2.2',targets:['办公室屏（192.0.2.1）'],locks:[{resourceType:'smart_screen',resourceKey:'777:900',ownerUser:'张工',ownerInstanceId:'电脑A',fencingToken:1}]};
const locked={code:'SCREEN_TAKEOVER_REQUIRED',params:{summary:'屏正在由其他电脑操作',details:JSON.stringify({conflicts:[conflict]})}};
const wrappers:ReturnType<typeof mount>[]=[];
beforeEach(()=>{invoke.mockReset();});
afterEach(()=>wrappers.splice(0).forEach(wrapper=>wrapper.unmount()));
async function setup(fresh=raw()){
  let previews=0,submissions=0;
  invoke.mockImplementation(async(command:string)=>{
    if(command==='screen_load')return fresh;
    if(command==='screen_preflight')return {id:++previews===1?'check-old':'check-new',items:[{screenId:'900',name:'办公室屏',ip:'192.0.2.1',state:'ready',reason:'通过'}]};
    if(command==='screen_execute'){if(++submissions===1)throw locked;return 'task-b';}
    if(command==='screen_takeover_release')return;
    throw new Error(command);
  });
  const wrapper=mount(ScreenTakeoverDialog,{props:{projectId:'project',contextKey:'operations'},global:{stubs:{teleport:true}}});wrappers.push(wrapper);
  const adapter=new RealScreenAdapter();
  const input:ScreenOperationInput={action:'install',targetIds:['900'],expectedTargets:{'900':screenMaintenanceFingerprint(screenSnapshotFromBackend(raw()).screens[0])},applicationId:'xiaoxin',appVersion:'2.0.9',abi:'universal',reinstall:false,concurrency:1,apk:{name:'小新.apk',size:100,path:'D:/packages/小新.apk',lastModified:0}};
  await adapter.preflight('project',input);
  // 立即收集失败，取消分支不产生未处理的 Promise。
  const result=adapter.execute('project',input).then(value=>({value,error:undefined}),error=>({value:undefined,error:error as Error}));
  await flushPromises();
  return {wrapper,result,input};
}
it('提交遇锁时显示原操作人和电脑，确认后重新读取、检查并继续，保留已选安装包',async()=>{
  const {wrapper,result,input}=await setup();
  expect(wrapper.text()).toContain('办公室屏');expect(wrapper.text()).toContain('张工');expect(wrapper.text()).toContain('办公室电脑 · 192.0.2.2');
  expect(wrapper.text()).toContain('已经发送到屏上的操作可能仍在执行');
  expect(wrapper.text()).toContain('该操作共 3 台屏');
  expect(invoke.mock.calls.some(([command])=>command==='screen_takeover_release')).toBe(false);
  await wrapper.findAll('button').find(button=>button.text()==='接手并继续')!.trigger('click');await flushPromises();
  expect(await result).toEqual({value:'task-b',error:undefined});
  expect(invoke.mock.calls.map(([command])=>command)).toEqual(['screen_preflight','screen_execute','screen_takeover_release','screen_load','screen_preflight','screen_execute']);
  expect(invoke).toHaveBeenLastCalledWith('screen_execute',expect.objectContaining({preflightId:'check-new',input:expect.objectContaining({apk:input.apk,targetIds:['900']})}));
});
it('取消时不释放对方锁，也不重复提交',async()=>{
  const {wrapper,result}=await setup();
  await wrapper.findAll('button').find(button=>button.text()==='取消')!.trigger('click');await flushPromises();
  expect((await result).error?.message).toContain('已取消接手');
  expect(invoke.mock.calls.filter(([command])=>command==='screen_execute')).toHaveLength(1);
  expect(invoke.mock.calls.some(([command])=>command==='screen_takeover_release')).toBe(false);
});
it('接手后发现IP改变，停止原操作，不把安装包发到新的地址',async()=>{
  const changed=raw();changed.screens[0].ip='192.0.2.9';changed.screens[0].revision=2;
  const {wrapper,result}=await setup(changed);
  await wrapper.findAll('button').find(button=>button.text()==='接手并继续')!.trigger('click');await flushPromises();
  expect((await result).error?.message).toContain('地址、身份或尺寸已变化');
  expect(invoke.mock.calls.filter(([command])=>command==='screen_execute')).toHaveLength(1);
});
it('确认前切换项目，旧确认自动关闭且不释放锁',async()=>{
  const {wrapper,result}=await setup();await wrapper.setProps({projectId:'other'});await flushPromises();
  expect((await result).error?.message).toContain('取消');
  expect(invoke.mock.calls.some(([command])=>command==='screen_takeover_release')).toBe(false);
});
it('重新读取时离开页面，不继续提交旧项目操作',async()=>{
  const {wrapper,result}=await setup();let complete!:()=>void;
  invoke.mockImplementation(async(command:string)=>{if(command==='screen_takeover_release')return;if(command==='screen_load'){await new Promise<void>(resolve=>{complete=resolve;});return raw();}throw new Error(command);});
  await wrapper.findAll('button').find(button=>button.text()==='接手并继续')!.trigger('click');await flushPromises();
  await wrapper.setProps({contextKey:'nodes'});complete();await flushPromises();
  expect((await result).error?.message).toContain('页面已切换');
  expect(invoke.mock.calls.filter(([command])=>command==='screen_execute')).toHaveLength(1);
});
