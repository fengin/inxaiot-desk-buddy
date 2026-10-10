import { invoke } from "@tauri-apps/api/core";
import type { ScreenAdapter } from "./screenAdapter";
import { commandErrorText } from "./errors";
import { listenTaskEvents } from "./activity";
import { useSystemDialogAdapter } from "./systemDialogAdapter";
import { getProjectSpacePath, projectSpacePath } from "@/shared/model/projectSpace";
import type { ScreenSnapshot, SmartScreen, LocalScreenInput, ScreenTask, ScreenStatusResult, ScreenMergeResult, ScreenPreflightItem, ScreenInspection } from "@/shared/model/screen";
import type { ScreenRegistrationPreview, ScreenPlatformFields } from "@/shared/model/screenRegistration";
import type { ScreenVersionPreview } from "@/shared/model/screenMaintenance";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import { screenPlatformFields } from "@/shared/model/screenRegistration";
import { confirmScreenTakeover, screenTakeoverConflicts } from "./screenTakeover";
import type { ScreenAppConfigPatch, ScreenAppConfigRead, ScreenAppConfigDraft } from "@/shared/model/screenAppConfig";
import type { ScreenNtpPatch, ScreenNtpRead } from "@/shared/model/screenNtp";

interface BackendSnapshot extends Omit<ScreenSnapshot, "screens" | "tasks"> {
  screens: (Pick<SmartScreen, "id" | "source" | "name" | "ip" | "mac" | "size" | "spaceId" | "location" | "revision" | "appVersion" | "platformStatus" | "aliases">)[];
  observations: Record<string, ScreenInspection[]>;
  tasks?: ScreenTask[];
}

async function call<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try { return await invoke<T>(command, args); }
  catch (error) { throw new Error(commandErrorText(error, "智能屏操作失败，请查看任务结果"), { cause: error }); }
}

export function screenSnapshotFromBackend(raw: BackendSnapshot): ScreenSnapshot {
  const screens: SmartScreen[] = raw.screens.map((asset) => {
    const observations = [asset.id, ...asset.aliases].flatMap((id) => raw.observations[id] ?? []).sort((a, b) => b.observedAt.localeCompare(a.observedAt));
    const current = observations.filter((item) => item.observedIp === asset.ip);
    const checked = current.find((item) => item.operationType !== "ping");
    const ping = current.find((item) => ["ping", "inspect", "diagnostics"].includes(item.operationType ?? "") || (!item.operationType && item.ping != null));
    const version = current.find((item) => item.appInstalled != null);
    const identity = current.find((item) => item.operationType === "mac" || item.observedMac || item.macCandidates?.length);
    const system = current.find((item) => item.android);
    const clock = current.find((item) => item.clockOffsetSeconds != null);
    const path = getProjectSpacePath(raw.spaces, asset.spaceId) ?? [];
    return {
      ...asset, spacePath: projectSpacePath(raw.spaces, asset.spaceId),
      building: path.find((p) => p.kind === "building")?.name ?? "", floor: path.find((p) => p.kind === "floor")?.name ?? "",
      buildingId: path.find((p) => p.kind === "building")?.id, floorId: path.find((p) => p.kind === "floor")?.id,
      observedMac: identity?.observedMac ?? "", ping: ping?.ping ?? null, checkedAt: ping?.observedAt ?? null,
      observedAppVersion: version?.observedAppVersion ?? null, appVersionCode: version?.appVersionCode ?? null,
      versionCheckedAt: version?.observedAt, versionCheckedIp: version?.observedIp,
      appVersion: asset.source === "local" ? version?.observedAppVersion ?? null : asset.appVersion,
      android: system?.android ?? "尚未检查", abi: system?.abis.join(" / ") || "尚未检查",
      adbAvailable: checked?.adbAvailable ?? null,
      adbStatus: !checked ? "uninspected" : checked.adbAvailable ? "available" : checked.errors.some((text) => text.includes("未授权")) ? "unauthorized" : "unavailable",
      adbCheckedAt: checked?.observedAt,
      persistentAdb: current.find((o) => o.persistentAdb != null)?.persistentAdb ?? null,
      freeSpaceMb: current.find((o) => o.freeSpaceMb != null)?.freeSpaceMb ?? null,
      clockOffsetSeconds: clock?.clockOffsetSeconds ?? null,
      deviceModel: system?.deviceModel ?? null, firmware: system?.firmware ?? null,
      macSource: identity?.macSource ?? null, macCandidates: identity?.macCandidates ?? [], macCheckedAt: identity?.observedAt,
      appInstalled: version?.appInstalled ?? null, appRunning: current.find((o) => o.appRunning != null)?.appRunning ?? null,
      deviceTime: clock?.deviceTime ?? null, computerTime: clock?.computerTime ?? null,
      timezone: current.find((o) => o.timezone != null)?.timezone ?? null,
      automaticTime: current.find((o) => o.automaticTime != null)?.automaticTime ?? null,
      automaticTimezone: current.find((o) => o.automaticTimezone != null)?.automaticTimezone ?? null,
      inspectionErrors: checked?.errors ?? []
    };
  });
  return { ...raw, mode: "real", screens, tasks: (raw.tasks ?? []).map(task => ({ ...task, mode: "real" })) };
}

export class RealScreenAdapter implements ScreenAdapter {
  readonly mode = "real" as const;
  async selectApk() {
    const filePath=await useSystemDialogAdapter().selectFile("选择智能小新安装包",[{name:"Android 安装包",extensions:["apk"]}]);
    return filePath ? call<import("@/shared/model/screen").ScreenApkSelection>("screen_parse_apk",{filePath}) : null;
  }
  private snapshots = new Map<string, ScreenSnapshot>();
  private loadRequests = new Map<string, number>();
  private previewRequests = new Map<string, number>();
  private preflights = new Map<string, string>();
  private configPatches = new Map<string, Record<string, ScreenAppConfigPatch>>();
  private ntpPatches = new Map<string, Record<string, ScreenNtpPatch>>();
  readNtp(projectId: string, screenIds: string[]) {
    return call<ScreenNtpRead[]>("screen_ntp_read", { localProjectId: projectId, screenIds });
  }
  async preflightNtp(projectId: string, input: Parameters<ScreenAdapter["execute"]>[1], patches: Record<string, ScreenNtpPatch>) {
    const generation = (this.previewRequests.get(projectId) ?? 0) + 1; this.previewRequests.set(projectId, generation);
    const snapshot = structuredClone(patches);
    const preview = await call<{ id: string; items: ScreenPreflightItem[] }>("screen_ntp_preflight", { localProjectId: projectId, input, patches: snapshot });
    if (this.previewRequests.get(projectId) === generation) { this.preflights.set(projectId, preview.id); this.ntpPatches.set(projectId, snapshot); }
    return preview.items;
  }
  loadAppConfigDraft(projectId: string) { return call<ScreenAppConfigDraft|null>('screen_app_config_draft_load',{localProjectId:projectId}); }
  saveAppConfigDraft(projectId: string,draft: ScreenAppConfigDraft|null) { return call<void>('screen_app_config_draft_save',{localProjectId:projectId,draft}); }
  readAppConfig(projectId: string, screenIds: string[]) {
    return call<ScreenAppConfigRead[]>("screen_app_config_read", { localProjectId: projectId, screenIds });
  }
  async preflightAppConfig(projectId: string, input: Parameters<ScreenAdapter["execute"]>[1], patches: Record<string, ScreenAppConfigPatch>) {
    const generation = (this.previewRequests.get(projectId) ?? 0) + 1; this.previewRequests.set(projectId, generation);
    const snapshot = structuredClone(patches);
    const preview = await call<{ id: string; items: ScreenPreflightItem[] }>("screen_app_config_preflight", { localProjectId: projectId, input, patches: snapshot });
    if (this.previewRequests.get(projectId) === generation) { this.preflights.set(projectId, preview.id); this.configPatches.set(projectId, snapshot); }
    return preview.items;
  }
  private registrationPreviews = new Map<string, ScreenRegistrationPreview>();
  private versionPreviews = new Map<string, ScreenVersionPreview>();
  private localRefresh = new Set<string>();
  private stopEvents?: () => void;
  private startingEvents = false;
  private timer?: ReturnType<typeof setInterval>;
  private listeners = new Set<(projectId: string, task?: ScreenTask) => void>();
  private notify(projectId: string) { this.localRefresh.add(projectId); for (const listener of this.listeners) listener(projectId); }
  private async freshPlatform(projectId:string) {
    const value=screenSnapshotFromBackend(await call<BackendSnapshot>('screen_load',{localProjectId:projectId,refresh:true}));
    if(!value.platformAvailable)throw new Error(value.platformMessage||'重新读取平台失败，本次操作未继续');
    this.snapshots.set(projectId,value);
    return value;
  }
  private async withTakeover<T>(projectId:string,submit:()=>Promise<T>,refresh:(snapshot:ScreenSnapshot,assertCurrent:()=>void)=>Promise<void>):Promise<T>{
    // 仅提交命令被后端明确判为锁冲突时询问；编辑、读取和普通失败不触发接手。
    for(let attempt=0;attempt<3;attempt++){
      try{return await submit();}
      catch(error){
        const conflicts=screenTakeoverConflicts(error);if(!conflicts)throw error;
        const confirmation=await confirmScreenTakeover({projectId,conflicts});
        try{
          confirmation.assertCurrent();
          await call<void>('screen_takeover_release',{localProjectId:projectId,expected:conflicts,confirmed:true});
          confirmation.assertCurrent();
          const latest=await this.freshPlatform(projectId);
          confirmation.assertCurrent();
          await refresh(latest,confirmation.assertCurrent);
          confirmation.assertCurrent();
          // 仍在同一页面继续原操作。若又出现新锁，下一轮重新展示新的操作人。
          try{return await submit();}catch(next){if(!screenTakeoverConflicts(next))throw next;}
        }finally{confirmation.finish();this.notify(projectId);}
      }
    }
    throw new Error('其他电脑仍在操作，请稍后再试');
  }
  async load(projectId: string, options?: { refreshPlatform?: boolean }) {
    const generation = (this.loadRequests.get(projectId) ?? 0) + 1; this.loadRequests.set(projectId, generation);
    const localOnly = this.localRefresh.delete(projectId);
    const refresh = options?.refreshPlatform === true || !localOnly;
    const value = screenSnapshotFromBackend(await call<BackendSnapshot>("screen_load", { localProjectId: projectId, refresh }));
    if (!refresh) { value.platformAvailable = this.snapshots.get(projectId)?.platformAvailable ?? false; value.availableProjects = this.snapshots.get(projectId)?.availableProjects; }
    if (this.loadRequests.get(projectId) === generation) this.snapshots.set(projectId, value);
    return value;
  }
  async selectBusinessProject(projectId: string, businessProjectId: string) {
    await call<void>("screen_select_project", { localProjectId: projectId, businessProjectId }); this.notify(projectId);
  }
  async saveLocal(projectId: string, input: LocalScreenInput, id?: string, expectedRevision?: number) {
    const revision = expectedRevision ?? this.snapshots.get(projectId)?.screens.find((screen) => screen.id === id)?.revision;
    await call("screen_save_local", { localProjectId: projectId, fields: input, id: id ?? null, expectedRevision: revision ?? null }); this.notify(projectId);
  }
  async importLocal(projectId: string, inputs: LocalScreenInput[]) { await call("screen_import_local", { localProjectId: projectId, fields: inputs }); this.notify(projectId); }
  async removeLocal(projectId: string, id: string) { await call("screen_remove_local", { localProjectId: projectId, id }); this.notify(projectId); }
  async savePlatformDraft(projectId: string, id: string, fields: ScreenPlatformFields, expectedRevision?: number, expectedDraftRevision?: number) {
    const snapshot = this.snapshots.get(projectId);
    if (expectedRevision !== snapshot?.screens.find((screen) => screen.id === id)?.revision) throw new Error("平台资料已变化，请重新打开编辑");
    await call("screen_save_draft", { localProjectId: projectId, id, fields, expectedRevision: expectedDraftRevision ?? 0, expectedAssetRevision: expectedRevision ?? null }); this.notify(projectId);
  }
  async discardPlatformDraft(projectId: string, id: string) {
    await call("screen_discard_draft", { localProjectId: projectId, id, expectedRevision: this.snapshots.get(projectId)?.platformDrafts?.[id]?.revision ?? 0 }); this.notify(projectId);
  }
  async previewPlatformRegistration(projectId: string, screenIds: string[]) {
    const value={ ...await call<ScreenRegistrationPreview>("screen_registration_preview", { localProjectId: projectId, screenIds }), mode: "real" as const };
    for(const [id,previous] of this.registrationPreviews)if(previous.projectId===projectId)this.registrationPreviews.delete(id);
    this.registrationPreviews.set(value.id,value);return value;
  }
  submitPlatformRegistration(projectId: string, input: Parameters<ScreenAdapter["submitPlatformRegistration"]>[1]) {
    let current={...input};const original=this.registrationPreviews.get(input.previewId);
    return this.withTakeover(projectId,()=>call<string>("screen_registration_submit",{localProjectId:projectId,input:current}),async(_latest,assertCurrent)=>{
      const fresh=await this.previewPlatformRegistration(projectId,input.screenIds);assertCurrent();
      for(const id of input.screenIds){
        const before=original?.items.find(item=>item.screenId===id),after=fresh.items.find(item=>item.screenId===id);
        if(!before||!after||after.state!=='ready'||JSON.stringify(before.after)!==JSON.stringify(after.after)||before.requiredMacConfirmation!==after.requiredMacConfirmation||before.needsSpaceConfirmation!==after.needsSpaceConfirmation){
          throw new Error(`重新读取后，${after?.after.name||'屏资料'}的检查结果已变化，请重新检查并确认。${after?.reason||''}`);
        }
      }
      current={...input,previewId:fresh.id};
    });
  }
  async previewVersionSync(projectId: string, screenIds: string[]) {
    const value=await call<ScreenVersionPreview>("screen_version_preview",{localProjectId:projectId,screenIds});
    for(const [id,previous] of this.versionPreviews)if(previous.projectId===projectId)this.versionPreviews.delete(id);
    this.versionPreviews.set(value.id,value);return value;
  }
  submitVersionSync(projectId: string, previewId: string, screenIds: string[]) {
    let current=previewId;const original=this.versionPreviews.get(previewId);
    return this.withTakeover(projectId,()=>call<string>("screen_version_submit",{localProjectId:projectId,previewId:current,screenIds}),async(_latest,assertCurrent)=>{
      const fresh=await this.previewVersionSync(projectId,screenIds);assertCurrent();
      for(const id of screenIds){const before=original?.items.find(item=>item.screenId===id),after=fresh.items.find(item=>item.screenId===id);
        if(!before||!after||after.state!=='ready'||after.ip!==before.ip||after.deviceVersion!==before.deviceVersion)throw new Error(`重新读取后版本检查结果已变化，请重新核对。${after?.reason||''}`);
      }
      current=fresh.id;
    });
  }
  merge(projectId: string, candidate: Parameters<ScreenAdapter["merge"]>[1], decision: Parameters<ScreenAdapter["merge"]>[2]) {
    let current=candidate;
    return this.withTakeover(projectId,()=>call<ScreenMergeResult|undefined>("screen_merge",{localProjectId:projectId,candidate:current,decision}),async(latest)=>{
      const local=latest.screens.find(screen=>screen.id===candidate.local.id&&screen.source==='local');
      const platform=latest.screens.find(screen=>screen.id===candidate.platform.id&&screen.source==='platform');
      for(const [before,after] of [[candidate.local,local],[candidate.platform,platform]] as const){
        if(!after||JSON.stringify(screenPlatformFields(before))!==JSON.stringify(screenPlatformFields(after))||before.appVersion!==after.appVersion)throw new Error('重新读取后合并资料已变化，请重新选择资料来源');
      }
      current={...candidate,local:local!,platform:platform!};
    });
  }
  async preflight(projectId: string, input: Parameters<ScreenAdapter["preflight"]>[1]) {
    const generation = (this.previewRequests.get(projectId) ?? 0) + 1; this.previewRequests.set(projectId, generation);
    const preview = await call<{ id: string; items: ScreenPreflightItem[] }>("screen_preflight", { localProjectId: projectId, input });
    if (this.previewRequests.get(projectId) === generation) this.preflights.set(projectId, preview.id);
    return preview.items;
  }
  execute(projectId: string, input: Parameters<ScreenAdapter["execute"]>[1]) {
    let preflightId = this.preflights.get(projectId);
    if (!preflightId) return Promise.reject(new Error("请先完成操作检查"));
    let current={...input};
    const configPatches=input.action==='app_config'?structuredClone(this.configPatches.get(projectId)):undefined;
    if(input.action==='app_config'&&!configPatches)return Promise.reject(new Error('请先检查配置修改内容'));
    const ntpPatches=input.action==='ntp'?structuredClone(this.ntpPatches.get(projectId)):undefined;
    if(input.action==='ntp'&&!ntpPatches)return Promise.reject(new Error('请先检查NTP设置'));
    const baseline=this.snapshots.get(projectId);
    return this.withTakeover(projectId,()=>call<string>("screen_execute",{localProjectId:projectId,preflightId,input:current}),async(latest,assertCurrent)=>{
      const fingerprints:Record<string,string>={};
      for(const id of input.targetIds){
        const after=latest.screens.find(screen=>screen.id===id),before=baseline?.screens.find(screen=>screen.id===id);
        const previous=input.expectedTargets?.[id]??(before?screenMaintenanceFingerprint(before):undefined);
        if(!after||!previous||JSON.stringify(JSON.parse(previous).slice(0,5))!==JSON.stringify(JSON.parse(screenMaintenanceFingerprint(after)).slice(0,5)))throw new Error('屏的地址、身份或尺寸已变化，请重新确认操作目标');
        fingerprints[id]=screenMaintenanceFingerprint(after);
      }
      current={...input,expectedTargets:fingerprints};
      const patches = configPatches ?? ntpPatches;
      const fresh=await call<{id:string;items:ScreenPreflightItem[]}>(input.action==='app_config'?'screen_app_config_preflight':input.action==='ntp'?'screen_ntp_preflight':'screen_preflight',{localProjectId:projectId,input:current,...(patches?{patches:Object.fromEntries(input.targetIds.map(id=>[id,patches[id]]))}:{})});assertCurrent();
      if(input.targetIds.some(id=>!fresh.items.some(item=>item.screenId===id&&item.state==='ready')))throw new Error(`重新检查后部分目标不能执行或无需执行，请核对后继续。${fresh.items.filter(item=>item.state!=='ready').map(item=>`${item.name}：${item.reason}`).join('；')}`);
      preflightId=fresh.id;this.preflights.set(projectId,fresh.id);
    });
  }
  cancel(projectId: string, taskId: string) { return call<void>("screen_cancel", { localProjectId: projectId, taskId }); }
  verify(projectId: string, taskId: string) { return call<void>("screen_verify", { localProjectId: projectId, taskId }); }
  coverStatus(projectId: string, changes: Parameters<ScreenAdapter["coverStatus"]>[1]) {
    let current=changes;
    return this.withTakeover(projectId,()=>call<ScreenStatusResult[]>("screen_cover_status",{localProjectId:projectId,changes:current}),async(latest)=>{
      current=changes.map(change=>{const screen=latest.screens.find(item=>item.id===change.id);
        if(!screen||screen.ip!==change.ip||![change.expected,change.next].includes(screen.platformStatus))throw new Error('平台资料或检查地址已变化，请重新核对状态差异');
        return {...change,expected:screen.platformStatus,revision:screen.revision};
      });
    });
  }
  readDiagnostics(projectId:string,taskId:string){return call<string>("screen_read_diagnostics",{localProjectId:projectId,taskId});}
  async exportDiagnostics(projectId:string,taskId:string){const directory=await useSystemDialogAdapter().selectDirectory("选择智能屏诊断导出文件夹");return directory?call<string>("screen_export_diagnostics",{localProjectId:projectId,taskId,directory}):null;}
  subscribe(listener: (projectId: string, task?: ScreenTask) => void) {
    this.listeners.add(listener);
    if (!this.stopEvents && !this.startingEvents) {
      this.startingEvents = true;
      void listenTaskEvents((event) => { if (event.domainType === "smart_screen") this.notify(event.localProjectId); }).then((stop) => {
        this.startingEvents = false;
        if (this.listeners.size) this.stopEvents = stop; else stop();
      }).catch(() => { this.startingEvents = false; });
    }
    this.timer ??= setInterval(() => {
      for (const [projectId, snapshot] of this.snapshots) if (snapshot.tasks.some(t => ["running", "cancelling"].includes(t.state))) this.notify(projectId);
    }, 2000);
    return () => {
      this.listeners.delete(listener);
      if (!this.listeners.size) { this.stopEvents?.(); this.stopEvents = undefined; if (this.timer) clearInterval(this.timer); this.timer = undefined; }
    };
  }
}
