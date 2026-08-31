export interface ReleaseImage {
  service: string;
  image: string;
  archive: string;
}

export interface ReleaseManifest {
  schemaVersion: number;
  version: string;
  composeFile: string;
  images: ReleaseImage[];
  templates: { env: string; hostInfo: string };
  runtime: { os: string; arch: string; docker: string; compose: string };
}

export interface ImageArchiveInfo {
  path: string;
  size: number;
  repoTags: string[];
}

export interface ReleaseValidation {
  valid: boolean;
  packageDir: string;
  manifest?: ReleaseManifest;
  fingerprint?: string;
  images: ImageArchiveInfo[];
  errors: string[];
  warnings: string[];
}

export interface ServiceImageInspection {
  archive: ImageArchiveInfo;
  expectedImage?: string;
  expectedMatches: boolean;
}

export interface DeploymentPlanInput {
  mode: "first_deploy" | "full_upgrade" | "service_upgrade";
  targetMacs: string[];
  artifactPath: string;
  artifactName: string;
  artifactVersion: string;
  serviceName?: string;
  imageName?: string;
  images?: Record<string, string>;
  batchSize: number;
  concurrency: number;
}

export interface DeploymentExecutionSummary {
  targets: Array<{
    mac: string;
    state: "succeeded" | "failed" | "cancelled" | "panicked";
    error?: string;
  }>;
  successCount: number;
  failureCount: number;
  cancelledCount: number;
}
