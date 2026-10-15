import { describe, expect, it } from "vitest";
import { appConfigChanges, appConfigRestartRequired, appConfigEffectiveAfter, appConfigReadTime } from "@/shared/model/screenAppConfig";
import type { ScreenAppConfiguration } from "@/shared/model/screenAppConfig";
function configuration(): ScreenAppConfiguration {
  const env={otaUrl:'https://config.example.org',wsUrl:null,h5Url:null,h5ReadyCheckEnabled:true,otaWsUrl:null,otaH5Url:null,effectiveWsUrl:null,effectiveH5Url:'assets/web/demo.html',wsSource:'none' as const,h5Source:'default' as const};
  return {customDeviceName:'原名称',environments:{current:'pre',test:{...env},pre:{...env},prod:{...env}}};
}
describe('配置修改字段与重启范围',()=>{
  it('已固定但未激活时显式开启仍产生差异，单独激活不重启',()=>{
    const config=configuration(),patch={set:{},clear:[],homeEnabled:true};
    expect(appConfigChanges(config,patch,{supported:true,enabled:true,active:false})).toEqual([{key:'homeEnabled',label:'固定小新为主屏',before:'已固定，激活未确认',after:'固定并激活'}]);
    expect(appConfigChanges(config,patch,{supported:true,enabled:true,active:true})).toEqual([]);
    expect(appConfigChanges(config,{set:{},clear:[]},{supported:true,enabled:true,active:false})).toEqual([]);
    expect(appConfigRestartRequired(config,patch)).toBe(false);
  });
  it('主屏修改独立于环境配置，解除固定无需重启且相同值不重复提交',()=>{
    const config=configuration(),patch={set:{},clear:[],homeEnabled:false};
    expect(appConfigChanges(config,patch,{supported:true,enabled:true})).toEqual([{key:'homeEnabled',label:'固定小新为主屏',before:'固定',after:'解除固定'}]);
    expect(appConfigChanges(config,patch,{supported:true,enabled:false})).toEqual([]);
    expect(appConfigRestartRequired(config,patch)).toBe(false);
    expect(appConfigChanges(config,{set:{},clear:[]},{supported:true,enabled:true})).toEqual([]);
  });
  it('名称与环境标识相同时仍按名称原文显示',()=>{
    const config=configuration();config.customDeviceName='test';
    expect(appConfigChanges(config,{set:{customDeviceName:'prod'},clear:[]})[0]).toMatchObject({before:'test',after:'prod'});
  });
  it('清空手动值的预览保留服务端值，并明确默认网页来源',()=>{
    const config=configuration();config.environments.pre.wsUrl='wss://manual.example.org';config.environments.pre.otaWsUrl='wss://server.example.org';
    const after=appConfigEffectiveAfter(config,{set:{},clear:['environments.pre.wsUrl','environments.pre.h5Url']},'pre');
    expect(after.ws).toEqual({source:'server',value:'wss://server.example.org'});expect(after.h5.source).toBe('default');expect(config.environments.pre.wsUrl).toBe('wss://manual.example.org');
  });
  it('不同偏移表示的同一读取时间显示一致',()=>{
    expect(appConfigReadTime('2026-10-03T15:00:00Z')).toBe(appConfigReadTime('2026-10-03T23:00:00+08:00'));
  });
  it('只列出指定字段，保持读取快照不变',()=>{
    const config=configuration();
    expect(appConfigChanges(config,{set:{customDeviceName:'新名称'},clear:[]})).toEqual([{key:'customDeviceName',label:'设备名',before:'原名称',after:'新名称'}]);
    expect(config.customDeviceName).toBe('原名称');
  });
  it('名称与非当前环境不重启；切换环境或改变当前环境需要重启',()=>{
    const config=configuration();
    expect(appConfigRestartRequired(config,{set:{customDeviceName:'新名称'},clear:[]})).toBe(false);
    expect(appConfigRestartRequired(config,{set:{'environments.test.h5ReadyCheckEnabled':false},clear:[]})).toBe(false);
    expect(appConfigRestartRequired(config,{set:{'environments.pre.h5ReadyCheckEnabled':false},clear:[]})).toBe(true);
    expect(appConfigRestartRequired(config,{set:{'environments.current':'prod'},clear:[]})).toBe(true);
  });
  it('相同值与已清空的字段不产生修改',()=>{
    const config=configuration(),patch={set:{'environments.current':'pre'},clear:['environments.pre.wsUrl']};
    expect(appConfigChanges(config,patch)).toEqual([]);expect(appConfigRestartRequired(config,patch)).toBe(false);
  });
});
