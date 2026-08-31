export interface ReleaseProfileValues {
  envTemplate: string;
  composeTemplate: string;
  platformHost: string;
  platformApiPort: number;
  platformMqttHost: string;
  platformMqttPort: number;
  sshPort: number;
  sshTimeoutSeconds: number;
  aioDataRoot: string;
  aioDeployRoot: string;
}

export interface ReleaseProfileCredentials {
  platformAuthKey: string;
  platformMqttUser: string;
  platformMqttPassword: string;
  aioMqttUser: string;
  aioMqttPassword: string;
  sshUser: string;
  sshPassword?: string;
  sshPrivateKey?: string;
}

export interface ReleaseProfileDraft {
  values: ReleaseProfileValues;
  credentials: ReleaseProfileCredentials;
  expectedVersion?: number;
}

export interface ReleaseProfileView {
  profileKey: string;
  values: ReleaseProfileValues;
  credentials: ReleaseProfileCredentials;
  version: number;
  updatedBy: string;
  updatedAt: string;
}

export interface ReleaseProfileValidation {
  valid: boolean;
  recognizedPlaceholderCount: number;
  warnings: string[];
}

export interface ReleaseMasterKeyTransferRequest {
  filePath: string;
  passphrase: string;
}

export interface ReleaseMasterKeyOperationResult {
  keyVersion: number;
  message: string;
}

export const emptyReleaseProfileDraft = (): ReleaseProfileDraft => ({
  values: {
    envTemplate: "",
    composeTemplate: "services:\n",
    platformHost: "",
    platformApiPort: 8055,
    platformMqttHost: "",
    platformMqttPort: 1883,
    sshPort: 22,
    sshTimeoutSeconds: 15,
    aioDataRoot: "/opt/data",
    aioDeployRoot: "/opt/data/inxaiot"
  },
  credentials: {
    platformAuthKey: "",
    platformMqttUser: "",
    platformMqttPassword: "",
    aioMqttUser: "",
    aioMqttPassword: "",
    sshUser: "root",
    sshPassword: "",
    sshPrivateKey: ""
  }
});
