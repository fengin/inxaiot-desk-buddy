import type { LocalScreenInput, ScreenMergeCandidate, ScreenMergeDecision, ScreenMergeResult, ScreenOperationInput, ScreenPreflightItem, ScreenSnapshot, ScreenStatusChange, ScreenStatusResult, ScreenTask } from "@/shared/model/screen";
import type { ScreenPlatformFields, ScreenRegistrationPreview, ScreenRegistrationSubmission } from "@/shared/model/screenRegistration";
import type { ScreenVersionPreview } from "@/shared/model/screenMaintenance";

/** 屏业务调用约定，正式桌面与开发原型分别装配。 */
export interface ScreenAdapter {
  readonly mode: "prototype" | "real";
  loadAppConfigDraft?(projectId: string): Promise<import("@/shared/model/screenAppConfig").ScreenAppConfigDraft | null>;
  saveAppConfigDraft?(projectId: string, draft: import("@/shared/model/screenAppConfig").ScreenAppConfigDraft | null): Promise<void>;
  readAppConfig?(projectId: string, screenIds: string[]): Promise<import("@/shared/model/screenAppConfig").ScreenAppConfigRead[]>;
  preflightAppConfig?(projectId: string, input: ScreenOperationInput, patches: Record<string, import("@/shared/model/screenAppConfig").ScreenAppConfigPatch>): Promise<ScreenPreflightItem[]>;
  selectApk?(): Promise<import("@/shared/model/screen").ScreenApkSelection | null>;
  readDiagnostics?(projectId:string,taskId:string):Promise<string>;
  exportDiagnostics?(projectId:string,taskId:string):Promise<string|null>;
  selectBusinessProject?(projectId: string, businessProjectId: string): Promise<void>;
  load(projectId: string, options?: { refreshPlatform?: boolean }): Promise<ScreenSnapshot>;
  saveLocal(projectId: string, input: LocalScreenInput, id?: string, expectedRevision?: number): Promise<void>;
  importLocal(projectId: string, inputs: LocalScreenInput[]): Promise<void>;
  removeLocal(projectId: string, id: string): Promise<void>;
  savePlatformDraft(projectId: string, screenId: string, values: ScreenPlatformFields, expectedRevision?: number, expectedDraftRevision?: number): Promise<void>;
  discardPlatformDraft(projectId: string, screenId: string): Promise<void>;
  previewPlatformRegistration(projectId: string, screenIds: string[]): Promise<ScreenRegistrationPreview>;
  submitPlatformRegistration(projectId: string, input: ScreenRegistrationSubmission): Promise<string>;
  previewVersionSync(projectId: string, screenIds: string[]): Promise<ScreenVersionPreview>;
  submitVersionSync(projectId: string, previewId: string, screenIds: string[]): Promise<string>;
  merge(projectId: string, candidate: ScreenMergeCandidate, decision: ScreenMergeDecision): Promise<ScreenMergeResult | undefined>;
  preflight(projectId: string, input: ScreenOperationInput): Promise<ScreenPreflightItem[]>;
  execute(projectId: string, input: ScreenOperationInput): Promise<string>;
  cancel(projectId: string, taskId: string): Promise<void>;
  verify(projectId: string, taskId: string): Promise<void>;
  coverStatus(projectId: string, changes: ScreenStatusChange[]): Promise<ScreenStatusResult[]>;
  subscribe(listener: (projectId: string, task?: ScreenTask) => void): () => void;
}

let adapter: ScreenAdapter | undefined;
export function configureScreenAdapter(next: ScreenAdapter) { adapter = next; }
export function useScreenAdapter(): ScreenAdapter {
  if (!adapter) throw new Error("智能屏业务尚未完成初始化");
  return adapter;
}
