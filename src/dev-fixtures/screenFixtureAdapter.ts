import type { ScreenAdapter } from "@/shared/api/screenAdapter";
import { createScreenSnapshot, screenSpaces } from "./screenData";
import { buildScreenMergeFields, effectiveScreenMac, normalizeScreenMac, screenActionLabel, screenInstallSizeWarning, screenMergeCandidates, screenTaskActive, validateLocalScreen, validateScreenApk, validateScreenMergeFields } from "@/shared/model/screen";
import type { LocalScreenInput, ScreenMergeCandidate, ScreenMergeDecision, ScreenMergeResult, ScreenOperationInput, ScreenPreflightItem, ScreenSnapshot, ScreenStatusChange, ScreenStatusResult, ScreenTask, ScreenTaskTarget } from "@/shared/model/screen";
import { getProjectSpacePath, projectSpacePath } from "@/shared/model/projectSpace";
import { screenSpaceFields } from "@/shared/model/screenSpace";
import { buildScreenRegistrationItems, screenPlatformFields, screenRegistrationFingerprint, validRegistrationMac } from "@/shared/model/screenRegistration";
import type { ScreenPlatformFields, ScreenRegistrationExecutionRecord, ScreenRegistrationPreview, ScreenRegistrationSubmission } from "@/shared/model/screenRegistration";
import { isScreenReadOnlyAction, screenCriticalDraftWarning, screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import type { ScreenVersionPreview, ScreenVersionSyncRecord } from "@/shared/model/screenMaintenance";
import { validateNtpServer } from "@/shared/model/screenNtp";
import type { ScreenNtpConfig, ScreenNtpEvidence, ScreenNtpPatch, ScreenNtpRead } from "@/shared/model/screenNtp";

/** 故障注入仅供测试使用，不是项目属性，不持久化也不暴露于页面适配端口。 */
type ScreenScenario = "normal" | "platform_offline" | "partial_failure" | "needs_review" | "ntp_sync_unconfirmed" | "write_denied" | "status_changed";

const copy = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
const now = () => new Date().toISOString();
const versionNumber = (value: string) => value.split(".").reduce((sum, part) => sum * 1000 + Number(part), 0);
export const SCREEN_STORAGE_PREFIX = "inx.screen-prototype.v1.";

/** 仅开发原型分支装配。这里没有 HTTP、Tauri Invoke、SQL 或设备进程调用。 */
export class FixtureScreenAdapter implements ScreenAdapter {
  readonly mode = "prototype" as const;
  private snapshots = new Map<string, ScreenSnapshot>();
  private listeners = new Set<(projectId: string, task?: ScreenTask) => void>();
  private timers = new Map<string, ReturnType<typeof setInterval>>();
  private scenarios = new Map<string, ScreenScenario>();
  private deviceVersions = new Map<string, Record<string, string | null>>();
  private versionPreviews = new Map<string, { preview: ScreenVersionPreview; fingerprints: Record<string, string> }>();
  private registrationPreviews = new Map<string, { preview: ScreenRegistrationPreview; fingerprints: Record<string, string>; directory: string }>();
  private ntpConfigs = new Map<string, Record<string, ScreenNtpConfig>>();
  private ntpPatches = new Map<string, Record<string, ScreenNtpPatch>>();
  constructor(private readonly storage: Storage = localStorage, private readonly tickMs = 650) {}

  private state(projectId: string) {
    if (!projectId) throw new Error("请先选择项目");
    const existing = this.snapshots.get(projectId);
    if (existing) return existing;
    let snapshot = createScreenSnapshot();
    try {
      const raw = this.storage.getItem(SCREEN_STORAGE_PREFIX + projectId);
      if (raw) {
        const saved = JSON.parse(raw) as ScreenSnapshot & { schemaVersion?: number; fixtureDeviceVersions?: Record<string, string | null>; fixtureNtpConfigs?: Record<string, ScreenNtpConfig> };
        if (saved.fixtureDeviceVersions) this.deviceVersions.set(projectId, saved.fixtureDeviceVersions);
        if (saved.fixtureNtpConfigs) this.ntpConfigs.set(projectId, saved.fixtureNtpConfigs);
        if (Array.isArray(saved.screens) && Array.isArray(saved.tasks) && Array.isArray(saved.ignoredPairs)) {
          snapshot = { screens: saved.screens, tasks: saved.tasks, ignoredPairs: saved.ignoredPairs, mergeHistory: saved.mergeHistory ?? [], platformDrafts: saved.platformDrafts ?? {}, platformReadAt: saved.platformReadAt, spacesAvailable: saved.spacesAvailable ?? true,
            spaces: Array.isArray(saved.spaces) ? saved.spaces : screenSpaces.map((space) => ({ ...space })), platformAvailable: true };
          // 补齐原型内已知种子平台记录的空间ID；本机手填/导入文本不自动关联。
          for (const screen of snapshot.screens) if (saved.schemaVersion !== 2 && saved.schemaVersion !== 3 && !screen.buildingId && screen.id.startsWith("platform-screen-")) {
            const seed = createScreenSnapshot().screens.find((item) => item.id === screen.id);
            if (seed) { screen.buildingId = seed.buildingId; screen.floorId = seed.floorId; }
          }
          if (saved.schemaVersion !== 3) {
            // 给原型目录补充深层示例节点，不改已有节点或原记录的空间归属。
            for (const node of screenSpaces) if (node.kind === "area" && snapshot.spaces.some((item) => item.id === node.parentId) && !snapshot.spaces.some((item) => item.id === node.id)) snapshot.spaces.push({ ...node });
            for (const screen of snapshot.screens) {
              const id = screen.spaceId !== undefined ? screen.spaceId : screen.floorId || screen.buildingId || null;
              const previousPath = screen.spacePath || [screen.building, screen.floor].filter(Boolean).join(" / ");
              screen.spaceId = id;
              screen.spacePath = projectSpacePath(snapshot.spaces, id) || previousPath;
              if (getProjectSpacePath(snapshot.spaces, id)) Object.assign(screen, screenSpaceFields(snapshot.spaces, id));
            }
          }
        }
      }
    } catch { /* 损坏的原型缓存不影响其他业务，重新使用样例。 */ }
    for (const task of snapshot.tasks.filter((item) => ["running", "cancelling"].includes(item.state))) {
      task.state = "needs_review";
      for (const target of task.targets) {
        if (target.state === "running") { target.state = "needs_review"; target.message = "预览已重新加载，请继续核实"; }
        else if (target.state === "queued") { target.state = "cancelled"; target.message = "预览重新加载，未继续派发"; }
      }
    }
    snapshot.platformDrafts ??= {};
    const versions = this.deviceVersions.get(projectId) ?? {};
    for (const screen of snapshot.screens) if (!(screen.id in versions)) versions[screen.id] = screen.observedAppVersion ?? screen.appVersion;
    this.deviceVersions.set(projectId, versions);
    this.snapshots.set(projectId, snapshot);
    return snapshot;
  }

  private changed(projectId: string, task?: ScreenTask) {
    const snapshot = this.state(projectId);
    this.persist(projectId, snapshot);
    this.notify(projectId, task);
  }

  private persist(projectId: string, snapshot: ScreenSnapshot) {
    this.storage.setItem(SCREEN_STORAGE_PREFIX + projectId, JSON.stringify({ schemaVersion: 3, screens: snapshot.screens, spaces: snapshot.spaces, spacesAvailable: snapshot.spacesAvailable ?? true, tasks: snapshot.tasks, ignoredPairs: snapshot.ignoredPairs, mergeHistory: snapshot.mergeHistory ?? [], platformDrafts: snapshot.platformDrafts ?? {}, platformReadAt: snapshot.platformReadAt, fixtureDeviceVersions: this.deviceVersions.get(projectId) ?? {}, fixtureNtpConfigs: this.ntpConfigs.get(projectId) ?? {} }));
  }

  private notify(projectId: string, task?: ScreenTask) {
    for (const listener of this.listeners) listener(projectId, task ? copy(task) : undefined);
  }

  private find(projectId: string, id: string) {
    const screen = this.state(projectId).screens.find((item) => item.id === id);
    if (!screen) throw new Error("屏记录已变化，请刷新列表");
    return screen;
  }

  private busy(snapshot: ScreenSnapshot, id: string) {
    return snapshot.tasks.some((task) => screenTaskActive(task) && task.targets.some((target) => target.screenId === id));
  }

  private directoryAvailable(projectId: string, snapshot = this.state(projectId)) { return this.scenarios.get(projectId) !== "platform_offline" && snapshot.spacesAvailable !== false; }

  async load(projectId: string) {
    const state = this.state(projectId);
    const available = this.scenarios.get(projectId) !== "platform_offline";
    if (available) {
      state.platformReadAt = now();
      try { this.persist(projectId, state); } catch { /* 读取仍可用；后续资料保存单独报告持久化失败。 */ }
    }
    return { ...copy(state), platformAvailable: available, spacesAvailable: this.directoryAvailable(projectId) };
  }

  async savePlatformDraft(projectId: string, screenId: string, values: ScreenPlatformFields, expectedRevision?: number) {
    const state = this.state(projectId), screen = this.find(projectId, screenId);
    if (screen.source !== "platform") throw new Error("平台待提交资料只适用于已注册屏");
    if (expectedRevision !== undefined && expectedRevision !== screen.revision) throw new Error("平台资料已变化，请重新打开编辑并核对");
    if ([screen.id, ...screen.aliases].some((id) => this.busy(state, id))) throw new Error("存在活动任务或待核实结果，请先处理");
    const clean = screenPlatformFields(values), current = screenPlatformFields(screen);
    const errors = validateLocalScreen(clean);
    if (errors.length) throw new Error(errors.join("；"));
    if (clean.spaceId && !this.directoryAvailable(projectId) && clean.spaceId !== screen.spaceId && clean.spaceId !== state.platformDrafts?.[screenId]?.values.spaceId) throw new Error("空间目录暂不可用，不能分配新的空间");
    if (clean.spaceId && this.directoryAvailable(projectId) && !getProjectSpacePath(state.spaces, clean.spaceId)) throw new Error("空间关联无效，请从空间树重新选择或明确清空");
    const next = { ...copy(state), tasks: state.tasks };
    const existing = next.platformDrafts?.[screenId];
    next.platformDrafts ??= {};
    if (JSON.stringify(clean) === JSON.stringify(current)) delete next.platformDrafts[screenId];
    else next.platformDrafts[screenId] = { screenId, baseRevision: expectedRevision !== undefined ? screen.revision : existing?.baseRevision ?? screen.revision, base: expectedRevision !== undefined ? current : existing?.base ?? current, values: clean, updatedAt: now() };
    this.persist(projectId, next); this.snapshots.set(projectId, next); this.notify(projectId);
  }

  async discardPlatformDraft(projectId: string, screenId: string) {
    const state = this.state(projectId), screen = this.find(projectId, screenId);
    if ([screen.id, ...screen.aliases].some((id) => this.busy(state, id))) throw new Error("存在活动任务或待核实结果，请先处理");
    const next = { ...copy(state), tasks: state.tasks };
    delete next.platformDrafts?.[screenId];
    this.persist(projectId, next); this.snapshots.set(projectId, next); this.notify(projectId);
  }

  private registrationItems(projectId: string, screenIds: string[], snapshot = this.state(projectId)) {
    return buildScreenRegistrationItems(snapshot, screenIds, {
      platformAvailable: this.scenarios.get(projectId) !== "platform_offline", spacesAvailable: this.directoryAvailable(projectId, snapshot),
      collectMac: (screen) => {
        if (!screen.adbAvailable) return null;
        const previous = screen.observedMac || screen.mac;
        if (validRegistrationMac(previous)) return previous;
        const identityHash = [...screen.id].reduce((value, char) => Math.imul(value ^ char.charCodeAt(0), 16777619) >>> 0, 2166136261);
        return `02:FE:${[24, 16, 8, 0].map((shift) => ((identityHash >>> shift) & 255).toString(16).padStart(2, "0")).join(":")}`.toUpperCase();
      }
    });
  }

  async previewPlatformRegistration(projectId: string, screenIds: string[]): Promise<ScreenRegistrationPreview> {
    if (!screenIds.length) throw new Error("请先选择要注册或更新的屏");
    const state = this.state(projectId);
    const preview: ScreenRegistrationPreview = { id: `registration-preview-${crypto.randomUUID()}`, projectId, createdAt: now(), items: this.registrationItems(projectId, screenIds) };
    const fingerprints = Object.fromEntries(preview.items.map((item) => { const screen = state.screens.find((row) => row.id === item.screenId); return [item.screenId, screen ? screenRegistrationFingerprint(screen, state.platformDrafts?.[screen.id]) : ""]; }));
    this.registrationPreviews.set(preview.id, { preview: copy(preview), fingerprints, directory: JSON.stringify(state.spaces) });
    return copy(preview);
  }

  private applyRegistration(snapshot: ScreenSnapshot, record: ScreenRegistrationExecutionRecord) {
    const screen = snapshot.screens.find((item) => item.id === record.screenId);
    if (!screen || screenRegistrationFingerprint(screen, snapshot.platformDrafts?.[screen.id]) !== record.sourceFingerprint) throw new Error("资料在提交后已变化，请核实，不能继续覆盖");
    if (!getProjectSpacePath(snapshot.spaces, record.after.spaceId)) throw new Error("所选空间已失效，请重新核实");
    if (snapshot.screens.some((other) => other.id !== screen.id && (other.ip === record.after.ip || (validRegistrationMac(record.after.mac) && normalizeScreenMac(effectiveScreenMac(other)) === normalizeScreenMac(record.after.mac))))) throw new Error("出现重复 IP 或 MAC，请先核对候选记录");
    const oldId = screen.id;
    if (screen.ip !== record.after.ip) { screen.ping = null; screen.checkedAt = null; screen.observedMac = ""; }
    Object.assign(screen, copy(record.after), screenSpaceFields(snapshot.spaces, record.after.spaceId), { revision: screen.revision + 1 });
    if (record.mode === "create") {
      screen.id = record.platformId!; screen.source = "platform";
      screen.aliases = [...new Set([...screen.aliases, oldId])];
      screen.observedAppVersion ??= screen.appVersion;
      screen.appVersion = null;
      screen.platformStatus = "online"; // 模拟平台登记默认值，不是本机 Ping 或设备健康观测。
    }
    delete snapshot.platformDrafts?.[oldId];
  }

  async submitPlatformRegistration(projectId: string, input: ScreenRegistrationSubmission): Promise<string> {
    const state = this.state(projectId);
    const previous = state.tasks.find((task) => task.registration?.previewId === input.previewId);
    const ids = [...new Set(input.screenIds)];
    if (previous) {
      if (JSON.stringify([...previous.targets.map((target) => target.screenId)].sort()) !== JSON.stringify([...ids].sort())) throw new Error("该预览已提交，不能改变原提交范围，请重新检查");
      return previous.id;
    }
    const stored = this.registrationPreviews.get(input.previewId);
    if (!stored || stored.preview.projectId !== projectId) throw new Error("预览已失效或不属于当前项目，请重新检查");
    if (!ids.length) throw new Error("请选择检查通过的提交目标");
    if (this.scenarios.get(projectId) === "platform_offline") throw new Error("平台不可用，未提交任何注册或更新");
    if (!this.directoryAvailable(projectId) || JSON.stringify(state.spaces) !== stored.directory) throw new Error("空间目录已变化或不可用，请重新预览");
    const currentItems = this.registrationItems(projectId, ids);
    const items = ids.map((id) => {
      const preview = stored.preview.items.find((item) => item.screenId === id), current = currentItems.find((item) => item.screenId === id), screen = state.screens.find((item) => item.id === id);
      if (!preview || preview.state !== "ready") throw new Error("只能提交本次预览中通过检查的目标");
      if (!screen || screenRegistrationFingerprint(screen, state.platformDrafts?.[id]) !== stored.fingerprints[id] || !current || current.state !== "ready" || JSON.stringify(current.after) !== JSON.stringify(preview.after) || this.scenarios.get(projectId) === "status_changed") throw new Error("资料或目标条件已变化，请重新预览，未提交任何注册或更新");
      if (preview.requiredMacConfirmation && input.macConfirmations?.[id] !== preview.requiredMacConfirmation) throw new Error(`请逐屏确认 ${preview.after.name} 的 MAC 采集失败处理`);
      if (preview.needsSpaceConfirmation && !input.spaceConfirmations?.includes(id)) throw new Error(`请确认 ${preview.after.name} 的空间变更`);
      return preview;
    });
    const next = { ...copy(state), tasks: [...state.tasks] };
    const task: ScreenTask = { id: `screen-prototype-${crypto.randomUUID()}`, projectId, action: "register", state: "running", createdAt: now(), updatedAt: now(), targets: [], logs: [{ time: now(), level: "INFO", message: "交互原型：模拟平台登记/资料更新，不执行 SQL 或 ADB。" }], registration: { previewId: input.previewId, records: [] } };
    items.forEach((item, index) => {
      const macSourceLabel = { collected: "本次模拟采集", unchanged: "身份资料未变更", history: "历史记录", empty: "空值", conflict: "身份冲突" }[item.macSource];
      const macDecision = item.requiredMacConfirmation === "existing" ? "；已逐屏确认沿用历史 MAC" : item.requiredMacConfirmation === "empty" ? "；已逐屏确认以空 MAC 继续" : "";
      task.logs.push({ time: now(), level: "INFO", message: `${item.after.name} · 预览时间 ${stored.preview.createdAt}；MAC 来源：${macSourceLabel}；${item.macMessage}${macDecision}` });
      if (item.needsSpaceConfirmation) {
        const spaceChange = item.diffs.find((diff) => diff.field === "spaceId");
        task.logs.push({ time: now(), level: "INFO", message: `${item.after.name} · 已确认空间变更：${spaceChange?.before} → ${spaceChange?.after}` });
      }
      const record: ScreenRegistrationExecutionRecord = { screenId: item.screenId, mode: item.mode, before: copy(item.before), after: copy(item.after), sourceFingerprint: stored.fingerprints[item.screenId]! };
      task.registration!.records.push(record);
      const target: ScreenTaskTarget = { screenId: item.screenId, name: item.after.name, ip: item.after.ip, state: "succeeded", progress: 100, message: "" };
      task.targets.push(target);
      const scenario = this.scenarios.get(projectId);
      try {
        if (scenario === "write_denied" || (scenario === "partial_failure" && index === Math.min(1, items.length - 1))) throw new Error("模拟平台拒绝本台资料写入，原资料和待提交资料均保留");
        record.platformId = item.mode === "create" ? `platform-${crypto.randomUUID()}` : item.screenId;
        if (scenario === "needs_review" && index === 0) {
          target.state = "needs_review"; target.message = "模拟提交已受理但回读中断，请先核实；不能重复创建平台记录";
        } else {
          this.applyRegistration(next, record);
          target.message = item.mode === "create" ? "模拟登记与回读成功；平台在线为登记默认值，非实时检查" : "模拟资料更新与回读成功；在线状态与应用版本未修改";
        }
      } catch (cause) { target.state = "failed"; target.message = (cause as Error).message; }
      task.logs.push({ time: now(), level: target.state === "succeeded" ? "INFO" : "WARN", message: `${target.name} · ${target.message}` });
    });
    this.finish(task); next.tasks.unshift(task);
    this.persist(projectId, next); this.snapshots.set(projectId, next); this.notify(projectId, task);
    return task.id;
  }

  private verifyRegistration(projectId: string, original: ScreenTask) {
    if (this.scenarios.get(projectId) === "platform_offline") throw new Error("平台不可用，请恢复后核实，不能重复提交");
    const state = this.state(projectId), task = copy(original);
    const next = { ...copy(state), tasks: state.tasks.map((item) => item.id === task.id ? task : item) };
    if (!this.directoryAvailable(projectId)) throw new Error("空间目录不可用，暂不能完成回读核实");
    for (const target of task.targets.filter((item) => item.state === "needs_review")) {
      const record = task.registration!.records.find((item) => item.screenId === target.screenId);
      if (!record?.platformId) { target.message = "没有平台受理凭据，保持待核实，不重复创建"; continue; }
      try { this.applyRegistration(next, record); target.state = "succeeded"; target.message = "已模拟回读核实原提交，未重复创建或更新平台记录"; }
      catch (cause) { target.message = (cause as Error).message; }
      task.logs.push({ time: now(), level: target.state === "succeeded" ? "INFO" : "WARN", message: `${target.name} · ${target.message}` });
    }
    this.finish(task); task.updatedAt = now();
    this.persist(projectId, next); this.snapshots.set(projectId, next); this.notify(projectId, task);
  }

  private add(snapshot: ScreenSnapshot, input: LocalScreenInput, id?: string, spacesAvailable = true) {
    const errors = validateLocalScreen(input);
    if (errors.length) throw new Error(errors.join("；"));
    const existing = id ? snapshot.screens.find((s) => s.id === id) : undefined;
    const spaceId = input.spaceId || null;
    if (!spaceId && !existing && input.spacePath?.trim()) throw new Error("空间路径尚未关联有效节点，请先匹配空间或明确设为待定空间");
    if (spaceId && !spacesAvailable && (!existing || existing.spaceId !== spaceId)) throw new Error("空间目录暂不可用，不能分配新的空间");
    if (spaceId && spacesAvailable && !getProjectSpacePath(snapshot.spaces, spaceId)) throw new Error("空间关联无效，请从空间树重新选择或明确清空");
    const space = spaceId && !spacesAvailable && existing
      ? { spaceId, spacePath: existing.spacePath ?? "", building: existing.building, floor: existing.floor, buildingId: existing.buildingId, floorId: existing.floorId }
      : screenSpaceFields(snapshot.spaces, spaceId);
    input = { ...input, ...space };
    if (snapshot.screens.some((s) => s.source === "local" && s.ip === input.ip && s.id !== id)) throw new Error(`${input.ip} 已存在于本机记录中`);
    if (id && (!existing || existing.source !== "local")) throw new Error("只支持编辑本机未注册屏");
    if (existing && this.busy(snapshot, existing.id)) throw new Error("该屏有活动任务或待核实结果，请先处理");
    if (existing) {
      if (existing.ip !== input.ip) { existing.observedMac = ""; existing.ping = null; existing.checkedAt = null; }
      Object.assign(existing, copy(input), { ...space, name: input.name.trim() || `智能屏 ${input.ip}`, revision: existing.revision + 1 });
    } else snapshot.screens.push({
      ...copy(input), ...space, name: input.name.trim() || `智能屏 ${input.ip}`, id: `local-${crypto.randomUUID()}`, source: "local", observedMac: "", revision: 1,
      platformStatus: "offline", ping: null, checkedAt: null, appVersion: null, android: input.size === "4" ? "Android 8.1" : "Android 10",
      abi: input.size === "4" ? "arm64-v8a" : "armeabi-v7a", adbAvailable: true, persistentAdb: false, freeSpaceMb: 2400, clockOffsetSeconds: 100, aliases: []
    });
  }

  async saveLocal(projectId: string, input: LocalScreenInput, id?: string) {
    const state = this.state(projectId);
    const next = { ...copy(state), tasks: state.tasks };
    this.add(next, input, id, this.directoryAvailable(projectId, next));
    this.persist(projectId, next);
    this.snapshots.set(projectId, next);
    this.notify(projectId);
  }

  async importLocal(projectId: string, inputs: LocalScreenInput[]) {
    const state = this.state(projectId);
    const next = { ...copy(state), tasks: state.tasks };
    for (const input of inputs) this.add(next, input, undefined, this.directoryAvailable(projectId, next));
    this.persist(projectId, next);
    this.snapshots.set(projectId, next);
    this.notify(projectId);
  }

  async removeLocal(projectId: string, id: string) {
    const state = this.state(projectId);
    const screen = this.find(projectId, id);
    if (screen.source !== "local") throw new Error("不能在工作台删除平台资产");
    if (this.busy(state, id)) throw new Error("该屏有活动任务或待核实结果");
    state.screens = state.screens.filter((item) => item.id !== id);
    this.changed(projectId);
  }

  async merge(projectId: string, candidate: ScreenMergeCandidate, decision: ScreenMergeDecision): Promise<ScreenMergeResult | undefined> {
    const state = this.state(projectId);
    if (this.scenarios.get(projectId) === "platform_offline") throw new Error("平台不可用，暂不能确认合并");
    const next = { ...copy(state), tasks: state.tasks };
    const current = screenMergeCandidates(next).find((item) => item.key === candidate.key);
    if (!current) throw new Error("候选记录已变化，请重新核对");
    if (JSON.stringify(current.local) !== JSON.stringify(candidate.local) || JSON.stringify(current.platform) !== JSON.stringify(candidate.platform)) throw new Error("候选字段已变化，请重新核对后再提交");
    let result: ScreenMergeResult | undefined;
    if (decision.kind === "ignore") next.ignoredPairs.push(current.key);
    else if (decision.kind === "merge") {
      if (current.conflict && decision.identityConfirmed !== true) throw new Error("MAC 冲突，请先核实并确认是同一台设备");
      if ([current.local.id, current.platform.id, ...current.platform.aliases].some((id) => this.busy(state, id))) throw new Error("请在活动任务或待核实结果处理完成后重新确认");
      const fields = buildScreenMergeFields(current, decision.choices);
      const errors = validateScreenMergeFields(fields, next.spaces, this.directoryAvailable(projectId, next));
      if (errors.length) throw new Error(errors.join("；"));
      Object.assign(fields, screenSpaceFields(next.spaces, fields.spaceId));
      if (next.screens.some((screen) => screen.source === "platform" && screen.id !== current.platform.id && (screen.ip === fields.ip || (fields.mac && normalizeScreenMac(effectiveScreenMac(screen)) === normalizeScreenMac(fields.mac))))) throw new Error("所选 IP 或 MAC 对应其他平台记录，请先核对多条候选，不能直接覆盖");
      if (this.scenarios.get(projectId) === "write_denied") throw new Error("模拟平台拒绝字段写入；两侧原记录均已保留，未执行合并");
      if (this.scenarios.get(projectId) === "status_changed") throw new Error("平台记录在确认后发生变化，请重新核对");
      result = { localId: current.local.id, platformId: current.platform.id, fields: copy(fields) };
      (next.mergeHistory ??= []).unshift({ ...copy(result), time: now(), choices: copy(decision.choices), beforeLocal: copy(current.local), beforePlatform: copy(current.platform) });
      // 地址变化后不能沿用另一地址的可达性；已有采集事实和历史不伪装成这次重新检查。
      if (current.platform.ip !== fields.ip) {
        const localChecked = current.local.ip === fields.ip && current.local.checkedAt;
        current.platform.ping = localChecked ? current.local.ping : null;
        current.platform.checkedAt = localChecked ? current.local.checkedAt : null;
      }
      Object.assign(current.platform, copy(fields), { ...screenSpaceFields(next.spaces, fields.spaceId), revision: current.platform.revision + 1 });
      current.platform.aliases = [...new Set([...current.platform.aliases, current.local.id, ...current.local.aliases])];
      current.platform.observedMac ||= current.local.observedMac;
      current.platform.observedAppVersion ??= current.local.observedAppVersion ?? current.local.appVersion;
      next.screens = next.screens.filter((item) => item.id !== current.local.id);
    } else throw new Error("请选择合并或保留独立记录");
    // 模拟平台字段更新和本机统一记录作为一次提交；保存失败不改变内存中的任一侧。
    this.persist(projectId, next);
    this.snapshots.set(projectId, next);
    this.notify(projectId);
    return result;
  }

  /** 设备版本来自独立模拟设备状态；不把可编辑的平台版本当作实时读取结果。 */
  private readVersion(projectId: string, screen: import("@/shared/model/screen").SmartScreen) {
    const versions = this.deviceVersions.get(projectId)!;
    const identity = [screen.id, ...screen.aliases].find((id) => id in versions);
    const value = identity ? versions[identity] ?? null : null;
    screen.observedAppVersion = value; screen.versionCheckedAt = now(); screen.versionCheckedIp = screen.ip;
    return value;
  }

  async previewVersionSync(projectId: string, ids: string[]): Promise<ScreenVersionPreview> {
    const state = this.state(projectId);
    const targets = [...new Set(ids.flatMap((id) => {
      const screen = state.screens.find((s) => s.id === id) ?? state.screens.find((s) => s.aliases.includes(id));
      return screen ? [screen.id] : [id];
    }))];
    if (!targets.length) throw new Error("请选择要核对版本的屏");
    const preview: ScreenVersionPreview = { id: crypto.randomUUID(), projectId, createdAt: now(), items: targets.map((id) => {
      const screen = state.screens.find((s) => s.id === id);
      const row: ScreenVersionPreview["items"][number] = { screenId: id, name: screen?.name ?? "已移除的屏", ip: screen?.ip ?? "—", platformVersion: screen?.source === "platform" ? screen.appVersion : null, deviceVersion: null, checkedAt: now(), state: "blocked", reason: "记录已变化，请重新选择" };
      if (!screen) return row;
      if ([screen.id, ...screen.aliases].some((key) => this.busy(state, key))) { row.reason = "存在活动任务或待核实结果，请先处理"; return row; }
      if (!screen.adbAvailable) { row.reason = "管理连接不可用，本次未读到应用版本；不使用旧缓存覆盖平台"; return row; }
      row.deviceVersion = this.readVersion(projectId, screen); row.checkedAt = screen.versionCheckedAt!;
      if (!row.deviceVersion) { row.reason = "本次未发现已安装的小新应用，保留平台版本"; return row; }
      const pending = screenCriticalDraftWarning(screen, state.platformDrafts?.[id]);
      if (pending) { row.reason = `${pending}；已读取当前确认地址，暂不同步版本`; return row; }
      if (screen.source === "platform" && validRegistrationMac(screen.mac) && validRegistrationMac(screen.observedMac) && normalizeScreenMac(screen.mac) !== normalizeScreenMac(screen.observedMac)) { row.reason = "采集身份与平台 MAC 不一致，请先核实，暂不同步版本"; return row; }
      if (screenMergeCandidates({ ...state, ignoredPairs: [] }).some((c) => c.conflict && [c.local.id, c.platform.id].includes(id))) { row.reason = "设备身份存在冲突，请先核实，暂不同步版本"; return row; }
      if (screen.source === "local") { row.state = "local"; row.reason = "平台未注册，本次版本只保留本机；注册后可再次核对并同步"; return row; }
      if (this.scenarios.get(projectId) === "platform_offline") { row.reason = "版本已读取并保留本机；平台不可用，恢复后重新核对再同步"; return row; }
      row.state = screen.appVersion === row.deviceVersion ? "skip" : "ready";
      row.reason = row.state === "skip" ? "平台版本一致，无需同步" : "可将本次读取的小新版本同步到平台";
      return row;
    }) };
    const fingerprints = Object.fromEntries(targets.map((id) => { const screen = state.screens.find((s) => s.id === id); return [id, screen ? screenMaintenanceFingerprint(screen) : ""]; }));
    this.changed(projectId);
    this.versionPreviews.set(preview.id, { preview: copy(preview), fingerprints });
    return copy(preview);
  }

  private applyVersionSync(projectId: string, snapshot: ScreenSnapshot, record: ScreenVersionSyncRecord) {
    const screen = snapshot.screens.find((s) => s.id === record.screenId);
    if (!screen || screen.source !== "platform" || screenMaintenanceFingerprint(screen) !== record.fingerprint || screen.ip !== record.ip) throw new Error("设备地址或身份已变化，请重新核对版本");
    const pending = screenCriticalDraftWarning(screen, snapshot.platformDrafts?.[screen.id]);
    if (pending) throw new Error(pending);
    if (validRegistrationMac(screen.mac) && validRegistrationMac(screen.observedMac) && normalizeScreenMac(screen.mac) !== normalizeScreenMac(screen.observedMac)) throw new Error("设备采集身份与平台记录不一致，请先核实");
    if ([screen.id, ...screen.aliases].some((id) => this.busy(snapshot, id))) throw new Error("该屏存在活动任务或待核实结果");
    const versions = this.deviceVersions.get(projectId)!;
    const key = [screen.id, ...screen.aliases].find((id) => id in versions);
    if (!screen.adbAvailable || !key || versions[key] !== record.after) throw new Error("本次设备版本或连接条件已变化，请重新读取");
    if (screen.appVersion !== record.before && screen.appVersion !== record.after) throw new Error("平台版本已被其他操作修改，请重新预览");
    if (screenMergeCandidates({ ...snapshot, ignoredPairs: [] }).some((c) => c.conflict && [c.local.id, c.platform.id].includes(screen.id))) throw new Error("设备身份出现冲突，请先核实");
    screen.appVersion = record.after;
  }

  async submitVersionSync(projectId: string, previewId: string, ids: string[]) {
    const state = this.state(projectId), selected = [...new Set(ids)];
    const previous = state.tasks.find((task) => task.versionSync?.previewId === previewId);
    if (previous) {
      if (JSON.stringify(previous.targets.map((t) => t.screenId).sort()) !== JSON.stringify([...selected].sort())) throw new Error("本次预览已提交，重新选择范围需再次核对");
      return previous.id;
    }
    const stored = this.versionPreviews.get(previewId);
    if (!stored || stored.preview.projectId !== projectId || !selected.length) throw new Error("版本预览已失效，请重新核对");
    if (this.scenarios.get(projectId) === "platform_offline") throw new Error("平台不可用，本机版本观测保留，未提交更新");
    const next = { ...copy(state), tasks: [...state.tasks] };
    const task: ScreenTask = { id: `screen-prototype-${crypto.randomUUID()}`, projectId, action: "version_sync", state: "running", createdAt: now(), updatedAt: now(), targets: [], logs: [], versionSync: { previewId, records: [] } };
    for (const id of selected) {
      const row = stored.preview.items.find((item) => item.screenId === id);
      if (!row || row.state !== "ready" || !row.deviceVersion) throw new Error("只能同步本次读取成功且有版本差异的目标");
      const record: ScreenVersionSyncRecord = { screenId: id, ip: row.ip, before: row.platformVersion, after: row.deviceVersion, fingerprint: stored.fingerprints[id]! };
      const target: ScreenTaskTarget = { screenId: id, name: row.name, ip: row.ip, progress: 100, state: "succeeded", message: "" };
      task.versionSync!.records.push(record); task.targets.push(target);
      try {
        if (this.scenarios.get(projectId) === "write_denied") throw new Error("模拟平台拒绝版本写入，本机观测和平台原值保留");
        if (this.scenarios.get(projectId) === "status_changed") throw new Error("模拟平台记录已变化，请重新核对");
        this.applyVersionSync(projectId, next, record);
        if (this.scenarios.get(projectId) === "needs_review") { next.screens.find((s) => s.id === id)!.appVersion = record.before; target.state = "needs_review"; target.message = "模拟版本提交后回读中断，请核实原结果，不重新安装"; }
        else target.message = `模拟平台版本已更新为 ${record.after} 并回读确认；没有执行安装`;
      } catch (cause) { target.state = "failed"; target.message = (cause as Error).message; }
      task.logs.push({ time: now(), level: target.state === "succeeded" ? "INFO" : "WARN", message: `${row.name} · ${row.ip} · 模拟设备读取 ${row.checkedAt} · 平台 ${record.before || "未记录"} → ${record.after} · ${target.message}` });
    }
    this.finish(task); next.tasks.unshift(task);
    this.persist(projectId, next); this.snapshots.set(projectId, next); this.notify(projectId, task);
    return task.id;
  }

  private verifyVersionSync(projectId: string, original: ScreenTask) {
    if (this.scenarios.get(projectId) === "platform_offline") throw new Error("平台不可用，请恢复后核实");
    const state = this.state(projectId), task = copy(original);
    const next = { ...copy(state), tasks: state.tasks.filter((item) => item.id !== task.id) };
    for (const target of task.targets.filter((item) => item.state === "needs_review")) {
      const record = task.versionSync!.records.find((item) => item.screenId === target.screenId)!;
      try { this.applyVersionSync(projectId, next, record); target.state = "succeeded"; target.message = "已模拟回读核实原版本同步结果，未重新安装"; }
      catch (cause) { target.message = (cause as Error).message; }
    }
    this.finish(task); task.updatedAt = now(); next.tasks = state.tasks.map((item) => item.id === task.id ? task : item);
    this.persist(projectId, next); this.snapshots.set(projectId, next); this.notify(projectId, task);
  }

  private ntpConfig(projectId: string, id: string) {
    const values = this.ntpConfigs.get(projectId) ?? {};
    values[id] ??= { server: "", autoTime: true, autoTimeZone: false, timeZone: "Asia/Shanghai" };
    this.ntpConfigs.set(projectId, values);
    return values[id]!;
  }
  async readNtp(projectId: string, ids: string[]): Promise<ScreenNtpRead[]> {
    const state = this.state(projectId);
    return ids.map(id => {
      const screen = state.screens.find(screen => screen.id === id);
      return { screenId: id, readAt: now(), config: screen?.adbAvailable ? copy(this.ntpConfig(projectId, id)) : null,
        capabilities: screen?.adbAvailable ? { activation: "reboot", rebootRequired: true } : null,
        message: screen?.adbAvailable ? "原型模拟读取，未连接真实屏" : "模拟 ADB 不可用，无法读取" };
    });
  }
  async preflightNtp(projectId: string, input: ScreenOperationInput, patches: Record<string, ScreenNtpPatch>) {
    if (input.action !== "ntp") throw new Error("NTP 设置必须使用专用操作");
    for (const id of input.targetIds) {
      if (!patches[id]) throw new Error("缺少本次 NTP 地址设置");
      const error = validateNtpServer(patches[id]!.server);
      if (error) throw new Error(error);
    }
    this.ntpPatches.set(projectId, copy(patches));
    const checks = await this.preflight(projectId, input);
    if (this.scenarios.get(projectId) === "platform_offline" && this.state(projectId).screens.some(screen => input.targetIds.includes(screen.id) && screen.source === "platform")) {
      return checks.map(row => ({ ...row, state: "blocked" as const, reason: "批次包含已注册屏，平台不可用时整批暂停；离线维护请另选未注册屏" }));
    }
    return checks.map(row => row.state === "ready" ? { ...row, reason: "模拟检查通过，保存后重启屏并验证授时" } : row);
  }

  async preflight(projectId: string, input: ScreenOperationInput): Promise<ScreenPreflightItem[]> {
    if(input.action==='app_config')throw new Error('原型不模拟配置保存，请使用正式桌面程序');
    if (input.action === "ntp" && input.targetIds.some(id => !this.ntpPatches.get(projectId)?.[id])) throw new Error("请先检查 NTP 设置");
    if (input.action === "register") throw new Error("注册与更新必须使用专用资料预览和逐屏确认流程");
    const state = this.state(projectId);
    const selected = state.screens.filter((screen) => input.targetIds.includes(screen.id));
    if (input.action === "install") {
      const sizeWarning = screenInstallSizeWarning(selected);
      if (sizeWarning) throw new Error(sizeWarning);
      if (input.applicationId !== "xiaoxin") throw new Error("当前仅支持智能小新应用");
      const packageErrors = validateScreenApk(input.apk);
      if (packageErrors.length) throw new Error(packageErrors.join("；"));
    }
    const seenIps = new Set<string>();
    const seenMacs = new Set<string>();
    return input.targetIds.filter((id, i, all) => all.indexOf(id) === i).map((id) => {
      const screen = selected.find((s) => s.id === id);
      if (!screen) return { screenId: id, name: "已移除的屏", ip: "—", state: "blocked", reason: "目标已变化，请重新选择" };
      let reason = "连接与执行条件满足";
      let result: ScreenPreflightItem["state"] = "ready";
      const block = (message: string) => { reason = message; result = "blocked"; };
      const mac = normalizeScreenMac(effectiveScreenMac(screen));
      const installedVersion = screen.observedAppVersion ?? screen.appVersion;
      if (seenIps.has(screen.ip) || (mac && seenMacs.has(mac))) block("本批次存在重复设备，请核对合并后重试");
      seenIps.add(screen.ip); if (mac) seenMacs.add(mac);
      if (input.expectedTargets?.[id] && input.expectedTargets[id] !== screenMaintenanceFingerprint(screen)) block("确认后的设备地址或身份已变化，请重新检查实际目标");
      else if (!isScreenReadOnlyAction(input.action) && screenCriticalDraftWarning(screen, state.platformDrafts?.[id])) block(screenCriticalDraftWarning(screen, state.platformDrafts?.[id]));
      else if (!isScreenReadOnlyAction(input.action) && screen.source === "platform" && this.scenarios.get(projectId) === "platform_offline") block("平台不可用，设备写操作暂不能取得共享操作条件；仍可执行只读检查");
      else if (this.busy(state, id)) block("该屏有活动任务或待核实结果");
      else if (["install", "time", "ntp", "reboot", "restart", "adb"].includes(input.action) && screenMergeCandidates({ ...state, ignoredPairs: [] }).some((c) => c.conflict && [c.local.id, c.platform.id].includes(id))) block("相同 IP 的 MAC 冲突，请先核实身份");
      else if (input.action !== "ping" && !screen.adbAvailable) block("ADB 无法连接，请先检查调试端口和授权");
      else if (input.action === "adb" && screen.size !== "10") block("仅适用于已经验证的 10 寸屏");
      else if (input.action === "ntp" && !["4", "10"].includes(screen.size)) block("请先确认屏尺寸与支持的固件");
      else if (input.action === "restart" && !installedVersion) block("未安装小新应用，请先安装");
      else if (input.action === "install") {
        if (input.appVersion && input.abi !== "universal" && input.abi !== screen.abi) block(`安装包不兼容：设备为 ${screen.abi}`);
        else if (screen.freeSpaceMb == null) block("尚未取得可用空间，请先检查设备");
        else if (screen.freeSpaceMb < 300) block(`可用空间不足：仅 ${screen.freeSpaceMb} MB`);
        else if (input.appVersion && installedVersion && versionNumber(input.appVersion) < versionNumber(installedVersion)) block("所选版本低于当前版本，不自动降级");
        else if (input.appVersion && result === "ready" && installedVersion === input.appVersion && !input.reinstall) { result = "skip"; reason = "已经是目标版本，无需重复安装"; }
        else if (result === "ready" && !input.appVersion) reason = "APK 元数据未解析，仅演示流程；未完成包名、版本和架构校验";
      }
      return { screenId: id, name: screen.name, ip: screen.ip, state: result, reason };
    });
  }

  async execute(projectId: string, input: ScreenOperationInput) {
    if(input.action==='app_config')throw new Error('原型不模拟配置保存，请使用正式桌面程序');
    if (!Number.isInteger(input.concurrency) || input.concurrency < 1 || input.concurrency > 5) throw new Error("并发台数必须为 1 到 5");
    const checks = await this.preflight(projectId, input);
    if (!checks.length || checks.some((row) => row.state !== "ready")) throw new Error("目标条件已变化，请只选择检查通过的设备并重新检查");
    const task: ScreenTask = {
      id: `screen-prototype-${crypto.randomUUID()}`, projectId, action: input.action, state: "running", createdAt: now(), updatedAt: now(), input: copy(input),
      targets: checks.map((row) => ({ screenId: row.screenId, name: row.name, ip: row.ip, state: "queued", progress: 0, message: "等待派发" })),
      logs: [{ time: now(), level: "INFO", message: `交互原型：模拟${screenActionLabel(input.action)}，不会连接设备或修改数据库。` }]
    };
    if (input.action === "ntp") for (const target of task.targets) target.result = { device: "pending", business: "not_required", shared: "not_required", evidence: { ntp: {
      before: copy(this.ntpConfig(projectId, target.screenId)), targetServer: this.ntpPatches.get(projectId)![target.screenId]!.server,
      save: "pending", activation: "pending", sync: "pending", rebootRequired: true,
    } } };
    this.state(projectId).tasks.unshift(task);
    const outcomeScenario = this.scenarios.get(projectId) ?? "normal";
    this.changed(projectId, task);
    this.timers.set(task.id, setInterval(() => this.tick(projectId, task, outcomeScenario), this.tickMs));
    return task.id;
  }

  private applyResult(projectId: string, task: ScreenTask, target: ScreenTaskTarget) {
    const screen = this.find(projectId, target.screenId);
    if (screen.ip !== target.ip || (task.input?.expectedTargets?.[screen.id] && task.input.expectedTargets[screen.id] !== screenMaintenanceFingerprint(screen))) {
      target.state = "needs_review"; target.message = "原执行地址与当前记录已不一致，结果保留待核实，不写入新目标"; return;
    }
    if (task.action === "install" && !task.input!.appVersion) {
      target.message = "安装流程演示完成；APK 元数据未解析，未执行真实安装，版本记录未更新";
      return;
    }
    screen.checkedAt = now();
    if (["ping", "inspect"].includes(task.action)) screen.ping = ["platform-screen-3", "platform-screen-17"].includes(screen.id) ? "offline" : "online";
    if (task.action === "ping") target.result = {
      device: "succeeded", business: "not_required", shared: "not_required",
      observation: { id: crypto.randomUUID(), operationType: "ping", observedIp: target.ip, observedAt: screen.checkedAt,
        ping: screen.ping, adbAvailable: false, abis: [], errors: [] }
    };
    if (["mac", "inspect"].includes(task.action) && !screen.observedMac) screen.observedMac = screen.mac || `02:AA:00:00:00:${this.state(projectId).screens.indexOf(screen).toString(16).padStart(2, "0").toUpperCase()}`;
    if (task.action === "install" && task.input!.appVersion) {
      this.deviceVersions.get(projectId)![screen.id] = task.input!.appVersion;
      screen.observedAppVersion = task.input!.appVersion; screen.appVersion = task.input!.appVersion;
      screen.versionCheckedAt = now(); screen.versionCheckedIp = screen.ip;
    }
    if (task.action === "inspect") this.readVersion(projectId, screen);
    if (task.action === "ntp") {
      const evidence = target.result!.evidence!.ntp as ScreenNtpEvidence;
      const current = this.ntpConfig(projectId, screen.id), unchanged = current.server === evidence.targetServer && current.autoTime;
      const previouslyActive = this.state(projectId).tasks.some(previous => previous.id !== task.id && previous.action === "ntp" && previous.targets.some(previousTarget => {
        const previousNtp = previousTarget.result?.evidence?.ntp as ScreenNtpEvidence | undefined;
        return previousTarget.screenId === screen.id && previousTarget.ip === screen.ip && previousNtp?.targetServer === current.server && previousNtp.activation === "succeeded" && previousNtp.after?.autoTime;
      }));
      const after = { ...current, server: evidence.targetServer, autoTime: true };
      this.ntpConfigs.get(projectId)![screen.id] = after;
      Object.assign(evidence, { after: copy(after), save: unchanged ? "unchanged" : "succeeded", activation: "succeeded", sync: "succeeded", rebootRequired: !unchanged || !previouslyActive, syncEvidence: { server: after.server || "固件默认（模拟）", clockOffsetSeconds: 1, sourceConfirmedBy: "原型模拟" } });
      target.result!.device = "succeeded";
      screen.clockOffsetSeconds = 1;
      target.message = unchanged && previouslyActive ? "模拟同值 NTP 只验证授时，未写入地址或重启；未连接真实设备" : "模拟地址保存、生效及授时已确认；未连接真实设备";
      return;
    }
    if (task.action === "time") screen.clockOffsetSeconds = 1;
    if (task.action === "adb") screen.persistentAdb = true;
    target.message = task.action === "ping" ? `本机 IP ${screen.ping === "online" ? "可达" : "不可达"}`
      : task.action === "install" ? `小新 ${screen.appVersion} 安装完成，启动检查通过`
        : task.action === "adb" ? "重启后 5555 已恢复，系统启动完成"
          : task.action === "time" ? "时间回读偏差 1 秒，时区与自动校时设置保留"
            : task.action === "mac" ? `已采集网卡 MAC：${screen.observedMac}`
              : task.action === "reboot" ? "系统已重新启动，管理连接恢复"
                : task.action === "restart" ? "小新应用已重新启动"
                  : task.action === "diagnostics" ? `诊断已采集：${screen.android}，${screen.abi}，可用 ${screen.freeSpaceMb} MB`
                    : `设备与管理通道检查完成；小新${screen.observedAppVersion ? `版本 ${screen.observedAppVersion}` : "未安装"}，版本观测只保存在本机`;
  }

  private tick(projectId: string, task: ScreenTask, scenario: ScreenScenario) {
    for (const target of task.targets.filter((item) => item.state === "running")) {
      target.progress = Math.min(100, target.progress + 50);
      if (target.progress < 100) { target.message = "模拟执行并等待设备回读"; continue; }
      const index = task.targets.indexOf(target);
      if (scenario === "partial_failure" && index === Math.min(1, task.targets.length - 1)) {
        target.state = "failed"; target.message = "连接中断，未取得成功结果；保留原版本，请检查连接后重试";
      } else if (task.action === "ntp" && scenario === "ntp_sync_unconfirmed") {
        this.applyResult(projectId, task, target);
        if (target.state !== "needs_review") {
          Object.assign(target.result!.evidence!.ntp as ScreenNtpEvidence, { sync: "unknown", syncEvidence: { server: null, clockOffsetSeconds: null, sourceConfirmedBy: "unconfirmed" } });
          target.result!.device = "failed"; target.state = "failed";
          target.message = "模拟地址保存和生效已确认，但授时未确认；本次已结束，请恢复服务器后发起同值验证新任务";
        }
      } else if (scenario === "needs_review" && index === 0) {
        if (task.action === "ntp") {
          this.applyResult(projectId, task, target);
          Object.assign(target.result!.evidence!.ntp as ScreenNtpEvidence, { activation: "unknown", sync: "unknown", syncEvidence: undefined });
          target.result!.device = "unknown";
        }
        target.state = "needs_review"; target.message = "动作已发送，但回读中断；请先核实，勿重复执行";
      } else { target.state = "succeeded"; this.applyResult(projectId, task, target); }
      task.logs.push({ time: now(), level: target.state === "succeeded" ? "INFO" : "WARN", message: `${target.name} · ${target.message}` });
    }
    if (task.state === "cancelling") {
      for (const target of task.targets.filter((item) => item.state === "queued")) { target.state = "cancelled"; target.message = "取消后未派发"; }
    } else {
      const available = Math.max(0, Math.min(5, task.input!.concurrency) - task.targets.filter((item) => item.state === "running").length);
      for (const target of task.targets.filter((item) => item.state === "queued").slice(0, available)) {
        const screen = this.state(projectId).screens.find((item) => item.id === target.screenId);
        const changed = !screen || screen.ip !== target.ip || (task.input?.expectedTargets?.[target.screenId] && task.input.expectedTargets[target.screenId] !== screenMaintenanceFingerprint(screen));
        const draftWarning = screen && !isScreenReadOnlyAction(task.action as ScreenOperationInput["action"]) ? screenCriticalDraftWarning(screen, this.state(projectId).platformDrafts?.[screen.id]) : "";
        if (changed || draftWarning) { target.state = "failed"; target.message = draftWarning || "派发前目标地址或身份已变化，未连接设备，请重新检查"; continue; }
        target.state = "running"; target.progress = 0; target.message = "模拟连接目标设备";
      }
    }
    if (!task.targets.some((target) => ["queued", "running"].includes(target.state))) {
      this.finish(task);
      clearInterval(this.timers.get(task.id)); this.timers.delete(task.id);
    }
    task.updatedAt = now(); this.changed(projectId, task);
  }

  private finish(task: ScreenTask) {
    task.state = task.targets.some((t) => t.state === "needs_review") ? "needs_review"
      : task.targets.some((t) => t.state === "cancelled") ? "cancelled"
        : task.targets.every((t) => t.state === "succeeded") ? "succeeded"
          : task.targets.some((t) => t.state === "succeeded") ? "partially_succeeded" : "failed";
  }

  async cancel(projectId: string, taskId: string) {
    const task = this.state(projectId).tasks.find((item) => item.id === taskId);
    if (!task || task.state !== "running") throw new Error("该任务当前不可取消");
    task.state = "cancelling";
    task.logs.push({ time: now(), level: "WARN", message: "停止后续派发；已开始的目标继续完成结果回读。" });
    this.changed(projectId, task);
  }

  async verify(projectId: string, taskId: string) {
    const task = this.state(projectId).tasks.find((item) => item.id === taskId);
    if (!task || task.state !== "needs_review") throw new Error("该任务没有待核实结果");
    if (task.action === "register") return this.verifyRegistration(projectId, task);
    if (task.action === "version_sync") return this.verifyVersionSync(projectId, task);
    if (task.action === "ntp") {
      for (const target of task.targets.filter(target => target.state === "needs_review")) {
        const evidence = target.result?.evidence?.ntp as ScreenNtpEvidence | undefined;
        const current = this.ntpConfig(projectId, target.screenId), screen = this.find(projectId, target.screenId);
        if (!evidence || screen.ip !== target.ip || (task.input?.expectedTargets?.[screen.id] && task.input.expectedTargets[screen.id] !== screenMaintenanceFingerprint(screen)) || current.server !== evidence.targetServer || !current.autoTime) {
          target.message = "模拟当前设置与本次目标不一致，仅核实，未重复修改或重启";
          continue;
        }
        const configurationKnown = ["succeeded", "unchanged"].includes(evidence.save) && ["succeeded", "not_required"].includes(evidence.activation);
        const syncUnconfirmed = this.scenarios.get(projectId) === "ntp_sync_unconfirmed" || (configurationKnown && evidence.sync !== "succeeded");
        Object.assign(evidence, { after: copy(current), save: evidence.save === "unchanged" ? "unchanged" : "succeeded", activation: "succeeded", sync: syncUnconfirmed ? "unknown" : "succeeded", syncEvidence: syncUnconfirmed ? { server: null, clockOffsetSeconds: null, sourceConfirmedBy: "unconfirmed" } : { server: current.server || "固件默认（模拟）", clockOffsetSeconds: 1, sourceConfirmedBy: "原型模拟" } });
        target.result!.device = syncUnconfirmed ? "failed" : "succeeded"; target.state = syncUnconfirmed ? "failed" : "succeeded"; target.progress = 100;
        target.message = syncUnconfirmed ? "已模拟只读核实保存及生效，授时仍未确认；本次已结束，请发起同值验证新任务" : "已模拟只读核实当前 NTP 设置及授时，未重复修改或重启";
      }
      this.finish(task); task.updatedAt = now(); this.changed(projectId, task); return;
    }
    for (const target of task.targets.filter((t) => t.state === "needs_review")) { target.state = "succeeded"; this.applyResult(projectId, task, target); target.progress = 100; }
    this.finish(task); task.updatedAt = now();
    task.logs.push({ time: now(), level: "INFO", message: "模拟回读确认已有动作结果，没有重新派发设备操作。" });
    this.changed(projectId, task);
  }

  async coverStatus(projectId: string, changes: ScreenStatusChange[]): Promise<ScreenStatusResult[]> {
    const state = this.state(projectId);
    if (this.scenarios.get(projectId) === "platform_offline") throw new Error("平台不可用，本机检查结果已保留");
    const results = changes.map((change, index) => {
      const screen = this.find(projectId, change.id);
      if (this.scenarios.get(projectId) === "write_denied") return { id: screen.id, name: screen.name, ok: false, message: "模拟数据库拒绝：没有状态字段写权限" };
      if (this.scenarios.get(projectId) === "status_changed" && index === 0) screen.revision++;
      if (screen.source !== "platform" || change.ip !== screen.ip || change.expected !== screen.platformStatus || change.revision !== screen.revision || change.next !== screen.ping) return { id: screen.id, name: screen.name, ok: false, message: "确认后目标条件已变化，请刷新差异并重新确认" };
      const before = screen.platformStatus;
      screen.platformStatus = change.next; screen.revision++;
      return { id: screen.id, name: screen.name, ok: true, message: `模拟覆盖成功：${before === "online" ? "在线" : "离线"} → ${change.next === "online" ? "在线" : "离线"}，回读一致` };
    });
    if (!results.length) return [];
    const task: ScreenTask = {
      id: `screen-prototype-${crypto.randomUUID()}`, projectId, action: "status", state: "succeeded", createdAt: now(), updatedAt: now(),
      targets: results.map((r) => ({ screenId: r.id, name: r.name, ip: this.find(projectId, r.id).ip, state: r.ok ? "succeeded" : "failed", progress: 100, message: r.message })),
      logs: [{ time: now(), level: "INFO", message: "交互原型：仅改变浏览器模拟状态，没有执行 SQL。" }, ...results.map((r) => ({ time: now(), level: r.ok ? "INFO" as const : "WARN" as const, message: `${r.name} · ${r.message}` }))]
    };
    this.finish(task); state.tasks.unshift(task); this.changed(projectId, task);
    return results;
  }

  async setScenario(projectId: string, scenario: ScreenScenario) { this.scenarios.set(projectId, scenario); this.changed(projectId); }
  async reset(projectId: string) {
    if (this.state(projectId).tasks.some((task) => ["running", "cancelling"].includes(task.state))) throw new Error("请先等待或取消正在执行的演示任务");
    const snapshot = createScreenSnapshot();
    this.deviceVersions.set(projectId, Object.fromEntries(snapshot.screens.map((screen) => [screen.id, screen.observedAppVersion ?? screen.appVersion])));
    this.ntpConfigs.delete(projectId); this.ntpPatches.delete(projectId);
    this.scenarios.delete(projectId); this.snapshots.set(projectId, snapshot); this.changed(projectId);
  }
  subscribe(listener: (projectId: string, task?: ScreenTask) => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  dispose() { for (const timer of this.timers.values()) clearInterval(timer); this.timers.clear(); }
}
