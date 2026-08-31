export type DataDirectorySwitchMode = "empty" | "migrate" | "use_existing";

export interface DataDirectorySwitchRequest {
  targetDirectory: string;
  mode: Exclude<DataDirectorySwitchMode, "use_existing">;
}

export interface DataDirectoryStatus {
  activeDirectory: string;
  defaultDirectory: string;
  pendingDirectory?: string;
  previousDirectory?: string;
  pendingMode?: DataDirectorySwitchMode;
  restartRequired: boolean;
  firstSetup: boolean;
  lastSwitchError?: string;
}
