export interface ReleaseProfileValues {
  envTemplate: string;
  composeTemplate: string;
  hostInfoTemplate: string;
  platformHost: string;
  platformApiPort: number;
  platformMqttHost: string;
  platformMqttPort: number;
  sshPort: number;
  sshTimeoutSeconds: number;
  aioDataRoot: string;
  aioDeployRoot: string;
}

export interface ReleaseComposeService {
  name: string;
  configuredImage: string;
  imageEnvironmentVariable: string;
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

export type ReleaseAgentScriptSource = "built_in" | "project";

export interface ReleaseAgentScriptView {
  fileName: string;
  version: string;
  protocolVersion: string;
  sha256: string;
  source: ReleaseAgentScriptSource;
}

export interface ReleaseAgentScriptReplaceRequest {
  filePath: string;
  expectedVersion: number;
}

export type ReleaseProfileField =
  | `values.${keyof ReleaseProfileValues}`
  | `credentials.${keyof ReleaseProfileCredentials}`;

export type ReleaseProfileFieldErrors = Partial<Record<ReleaseProfileField, string>>;

export interface ReleaseProfileView {
  profileKey: string;
  values: ReleaseProfileValues;
  credentials: ReleaseProfileCredentials;
  credentialsResetRequired: boolean;
  agentScript: ReleaseAgentScriptView;
  composeServices: ReleaseComposeService[];
  version: number;
  updatedBy: string;
  updatedAt: string;
}

export interface ReleaseProfileValidation {
  valid: boolean;
  recognizedPlaceholderCount: number;
  composeServices: ReleaseComposeService[];
  publishedPorts: number[];
  warnings: string[];
}

export const emptyReleaseProfileDraft = (): ReleaseProfileDraft => ({
  values: {
    envTemplate: "",
    composeTemplate: "services:\n",
    hostInfoTemplate: [
      "{",
      '  "mac": "{{ node.mac }}",',
      '  "ip": "{{ node.ip }}",',
      '  "hostname": "{{ node.name }}",',
      '  "authKey": "{{ authKey }}"',
      "}"
    ].join("\n"),
    platformHost: "",
    platformApiPort: 8055,
    platformMqttHost: "",
    platformMqttPort: 1883,
    sshPort: 22,
    sshTimeoutSeconds: 15,
    aioDataRoot: "/opt/data",
    aioDeployRoot: "/opt/data/deploy"
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
