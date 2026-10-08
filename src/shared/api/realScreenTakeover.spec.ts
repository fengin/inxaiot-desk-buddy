import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { RealScreenAdapter, screenSnapshotFromBackend } from './realScreenAdapter';
import { registerScreenTakeoverHandler } from './screenTakeover';
import { screenPlatformFields } from '@/shared/model/screenRegistration';
import type { ScreenRegistrationPreview } from '@/shared/model/screenRegistration';
import type { ScreenMergeCandidate } from '@/shared/model/screen';

const {invoke}=vi.hoisted(()=>({invoke:vi.fn()}));
vi.mock('@tauri-apps/api/core',()=>({invoke}));
const asset=(source:'platform'|'local'='platform')=>({id:source==='platform'?'900':'local',source,name:'办公室屏',ip:'192.0.2.1',mac:'02:11:22:33:44:55',size:'4' as const,location:'门口',spaceId:'100',revision:1,appVersion:null,platformStatus:'online' as const,aliases:[]});
const raw=()=>({screens:[asset()],spaces:[],observations:{},ignoredPairs:[],platformAvailable:true});
const conflict={operationId:'op-a',operationName:'注册/更新到平台',operationType:'register',ownerUser:'张工',ownerInstanceId:'电脑A',targets:['办公室屏'],locks:[{resourceType:'smart_screen',resourceKey:'777:900',ownerUser:'张工',ownerInstanceId:'电脑A',fencingToken:1}]};
const locked={code:'SCREEN_TAKEOVER_REQUIRED',params:{details:JSON.stringify({conflicts:[conflict]})}};
let unregister:()=>void;
const confirm=vi.fn();
beforeEach(()=>{invoke.mockReset();confirm.mockReset();unregister=registerScreenTakeoverHandler(async request=>{confirm(request);return {assertCurrent(){},finish(){}};});});
afterEach(()=>{unregister();});
it('小新配置接手后只刷新目标和能力，保留明确修改字段，不比较配置旧值',async()=>{
  let previews=0,submits=0;
  invoke.mockImplementation(async(command:string)=>{
    if(command==='screen_load')return raw();
    if(command==='screen_app_config_preflight')return {id:++previews===1?'old':'new',items:[{screenId:'900',name:'办公室屏',ip:'192.0.2.1',state:'ready',reason:'通过'}]};
    if(command==='screen_execute'){if(++submits===1)throw locked;return 'config-task';}
    if(command==='screen_takeover_release')return;
    throw new Error(command);
  });
  const adapter=new RealScreenAdapter();await adapter.load('p');
  const input={action:'app_config' as const,targetIds:['900'],appVersion:'',abi:'universal' as const,reinstall:false,concurrency:1};
  const patches={'900':{set:{'environments.pre.wsUrl':'wss://configured.example.org'},clear:[]}};
  await adapter.preflightAppConfig('p',input,patches);
  expect(await adapter.execute('p',input)).toBe('config-task');
  const checks=invoke.mock.calls.filter(([command])=>command==='screen_app_config_preflight');
  expect(checks).toHaveLength(2);expect(checks[1]![1].patches).toEqual(patches);
  expect(invoke).toHaveBeenLastCalledWith('screen_execute',expect.objectContaining({preflightId:'new'}));expect(confirm).toHaveBeenCalledOnce();
});
function registration(id:string):ScreenRegistrationPreview{return {id,projectId:'p',createdAt:'2026-10-03T00:00:00Z',items:[{screenId:'900',mode:'update',state:'ready',reason:'可提交',before:screenPlatformFields(asset()),after:{...screenPlatformFields(asset()),name:'新名称'},diffs:[{field:'name',label:'名称',before:'办公室屏',after:'新名称'}],expectedRevision:1,needsSpaceConfirmation:false,macSource:'unchanged',macMessage:'未变化',requiredMacConfirmation:null,duplicateIds:[]}]};}
it('登记接手后生成新预览，保留草稿和原确认再提交',async()=>{
  let previews=0,submits=0;
  invoke.mockImplementation(async(command:string)=>{if(command==='screen_registration_preview')return registration(++previews===1?'old':'new');if(command==='screen_registration_submit'){if(++submits===1)throw locked;return 'continued';}if(command==='screen_load')return raw();if(command==='screen_takeover_release')return;throw new Error(command);});
  const adapter=new RealScreenAdapter();await adapter.previewPlatformRegistration('p',['900']);
  expect(await adapter.submitPlatformRegistration('p',{previewId:'old',screenIds:['900'],macConfirmations:{},spaceConfirmations:[]})).toBe('continued');
  expect(invoke).toHaveBeenLastCalledWith('screen_registration_submit',{localProjectId:'p',input:{previewId:'new',screenIds:['900'],macConfirmations:{},spaceConfirmations:[]}});
  expect(confirm).toHaveBeenCalledOnce();
});
it('重新预览发现待提交资料变化，不自动覆盖',async()=>{
  let previews=0;
  invoke.mockImplementation(async(command:string)=>{if(command==='screen_registration_preview'){const data=registration(++previews===1?'old':'new');if(previews===2)data.items[0].after.mac='02:99:99:99:99:99';return data;}if(command==='screen_registration_submit')throw locked;if(command==='screen_load')return raw();if(command==='screen_takeover_release')return;throw new Error(command);});
  const adapter=new RealScreenAdapter();await adapter.previewPlatformRegistration('p',['900']);
  await expect(adapter.submitPlatformRegistration('p',{previewId:'old',screenIds:['900'],macConfirmations:{},spaceConfirmations:[]})).rejects.toThrow('检查结果已变化');
  expect(invoke.mock.calls.filter(([command])=>command==='screen_registration_submit')).toHaveLength(1);
});
it('版本同步接手后再次读取设备版本，用新检查编号继续',async()=>{
  let reads=0,submits=0;
  invoke.mockImplementation(async(command:string)=>{if(command==='screen_version_preview')return {id:++reads===1?'old':'new',projectId:'p',createdAt:'now',items:[{screenId:'900',ip:'192.0.2.1',state:'ready',deviceVersion:'2.0.9'}]};if(command==='screen_version_submit'){if(++submits===1)throw locked;return 'version-task';}if(command==='screen_load')return raw();if(command==='screen_takeover_release')return;throw new Error(command);});
  const adapter=new RealScreenAdapter();await adapter.previewVersionSync('p',['900']);
  expect(await adapter.submitVersionSync('p','old',['900'])).toBe('version-task');
  expect(invoke).toHaveBeenLastCalledWith('screen_version_submit',{localProjectId:'p',previewId:'new',screenIds:['900']});
});
it('状态更新接手后使用最新资料修改次数，保留目标状态',async()=>{
  let submits=0;const latest=raw();latest.screens[0].revision=2;
  invoke.mockImplementation(async(command:string)=>{if(command==='screen_cover_status'){if(++submits===1)throw locked;return [{id:'900',name:'办公室屏',ok:true,message:'已更新'}];}if(command==='screen_load')return latest;if(command==='screen_takeover_release')return;throw new Error(command);});
  await new RealScreenAdapter().coverStatus('p',[{id:'900',ip:'192.0.2.1',expected:'online',next:'offline',revision:1}]);
  expect(invoke).toHaveBeenLastCalledWith('screen_cover_status',{localProjectId:'p',changes:[{id:'900',ip:'192.0.2.1',expected:'online',next:'offline',revision:2}]});
});
it('人工合并接手后重新读取两侧资料，保留用户的字段来源选择',async()=>{
  const latest={...raw(),screens:[asset(),asset('local')]};const screens=screenSnapshotFromBackend(latest).screens;
  const candidate={local:screens[1],platform:screens[0]} as ScreenMergeCandidate;
  let submits=0;invoke.mockImplementation(async(command:string)=>{if(command==='screen_merge'){if(++submits===1)throw locked;return {taskId:'merge-task'};}if(command==='screen_load')return latest;if(command==='screen_takeover_release')return;throw new Error(command);});
  const decision={kind:'merge' as const,identityConfirmed:true,choices:{name:'local' as const,ip:'platform' as const,mac:'platform' as const,size:'platform' as const,space:'platform' as const,location:'platform' as const,appVersion:'platform' as const}};
  await new RealScreenAdapter().merge('p',candidate,decision);
  expect(invoke).toHaveBeenLastCalledWith('screen_merge',expect.objectContaining({decision,candidate:expect.objectContaining({local:expect.objectContaining({id:'local'}),platform:expect.objectContaining({id:'900'})})}));
});
it('确认后出现另一把锁，必须再次显示新持有人供用户确认',async()=>{
  let submits=0;const other={...conflict,operationId:'op-c',ownerUser:'李工'};
  invoke.mockImplementation(async(command:string)=>{if(command==='screen_cover_status'){submits++;if(submits===1)throw locked;if(submits<=3)throw {code:'SCREEN_TAKEOVER_REQUIRED',params:{details:JSON.stringify({conflicts:[other]})}};return [];}if(command==='screen_load')return raw();if(command==='screen_takeover_release')return;throw new Error(command);});
  await new RealScreenAdapter().coverStatus('p',[{id:'900',ip:'192.0.2.1',expected:'online',next:'offline',revision:1}]);
  expect(confirm).toHaveBeenCalledTimes(2);expect(confirm.mock.calls[1][0].conflicts[0].ownerUser).toBe('李工');
});
