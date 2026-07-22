import { invoke } from "./adapter";

export interface FeatureCapabilities {
  directoryPicker: boolean;
  openExternal: boolean;
  endpointTest: boolean;
  workspace: boolean;
  subscriptionQuota: boolean;
  tray: boolean;
  terminalLaunch: boolean;
  configDirOverride: boolean;
  fileDialogs: boolean;
  sessionManager: boolean;
  usageDashboard: boolean;
  environmentManagement: boolean;
  appUpdate: boolean;
  portableMode: boolean;
  claudePluginIntegration: boolean;
}

export interface AppCapabilities {
  providers: boolean;
  prompts: boolean;
  mcp: boolean;
  skills: boolean;
  usage: boolean;
  sessions: boolean;
  localRouting: boolean;
  additiveProviderMode: boolean;
  hostManaged: boolean;
}

export interface RuntimeCapabilities {
  runtime: "web" | "desktop";
  host: "server" | "local";
  apps: string[];
  features: FeatureCapabilities;
  appFeatures: Record<string, AppCapabilities>;
}

export const capabilitiesApi = {
  async get(): Promise<RuntimeCapabilities> {
    return await invoke("get_capabilities");
  },
};
