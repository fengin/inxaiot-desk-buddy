export interface ImageArchiveInfo {
  path: string;
  size: number;
  repoTags: string[];
}

export interface ServiceImageInspection {
  archive: ImageArchiveInfo;
  expectedImage?: string;
  expectedMatches: boolean;
}

export interface DeploymentPlanInput {
  mode: "first_deploy" | "full_upgrade" | "service_upgrade";
  targetMacs: string[];
  imageFiles: DeploymentImageInput[];
  artifactPath?: string;
  artifactName?: string;
  artifactVersion?: string;
  serviceName?: string;
  imageName?: string;
  serviceImageEnvironmentVariable?: string;
  images?: Record<string, string>;
  batchSize: number;
  concurrency: number;
}

export interface DeploymentImageInput {
  serviceName: string;
  filePath: string;
  imageTag: string;
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
