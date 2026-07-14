import { useState, useCallback, useMemo } from "react";
import type { AppId } from "@/lib/api";
import { useProvidersQuery } from "@/lib/query/queries";

export type PiAgentApiMode = string;

export interface PiAgentModel {
  id: string;
  name?: string;
  contextWindow?: number;
  maxTokens?: number;
  reasoning?: boolean;
  cost?: {
    input?: number;
    output?: number;
    cacheRead?: number;
    cacheWrite?: number;
  };
  [key: string]: unknown;
}

export interface PiAgentProviderSettingsConfig {
  name?: string;
  baseUrl?: string;
  apiKey?: string;
  api?: PiAgentApiMode;
  models?: PiAgentModel[];
  headers?: Record<string, string>;
  [key: string]: unknown;
}

export const PI_AGENT_DEFAULT_API_MODE: PiAgentApiMode = "openai-completions";

interface UsePiAgentFormStateParams {
  initialData?: {
    settingsConfig?: Record<string, unknown>;
  };
  appId: AppId;
  providerId?: string;
  onSettingsConfigChange: (config: string) => void;
  getSettingsConfig: () => string;
}

const PI_AGENT_DEFAULT_CONFIG_OBJ = {
  name: "",
  baseUrl: "",
  apiKey: "",
} as const;

export const PI_AGENT_DEFAULT_CONFIG = JSON.stringify(
  PI_AGENT_DEFAULT_CONFIG_OBJ,
  null,
  2,
);

export interface PiAgentFormState {
  piAgentProviderKey: string;
  setPiAgentProviderKey: (key: string) => void;
  piAgentBaseUrl: string;
  piAgentApiKey: string;
  piAgentApi: PiAgentApiMode;
  piAgentModels: PiAgentModel[];
  existingPiAgentKeys: string[];
  handlePiAgentBaseUrlChange: (baseUrl: string) => void;
  handlePiAgentApiKeyChange: (apiKey: string) => void;
  handlePiAgentApiChange: (api: PiAgentApiMode) => void;
  handlePiAgentModelsChange: (models: PiAgentModel[]) => void;
  resetPiAgentState: (config?: Partial<PiAgentProviderSettingsConfig>) => void;
}

function parsePiAgentField<T>(
  initialData: UsePiAgentFormStateParams["initialData"],
  field: string,
  fallback: T,
): T {
  try {
    if (initialData?.settingsConfig) {
      return (initialData.settingsConfig[field] as T) || fallback;
    }
    return (
      ((PI_AGENT_DEFAULT_CONFIG_OBJ as Record<string, unknown>)[field] as T) ||
      fallback
    );
  } catch {
    return fallback;
  }
}

export function usePiAgentFormState({
  initialData,
  appId,
  providerId,
  onSettingsConfigChange,
  getSettingsConfig,
}: UsePiAgentFormStateParams): PiAgentFormState {
  const { data: piAgentProvidersData } = useProvidersQuery("pi-agent");
  const existingPiAgentKeys = useMemo(() => {
    if (!piAgentProvidersData?.providers) return [];
    return Object.keys(piAgentProvidersData.providers).filter(
      (k) => k !== providerId,
    );
  }, [piAgentProvidersData?.providers, providerId]);

  const [piAgentProviderKey, setPiAgentProviderKey] = useState<string>(() => {
    if (appId !== "pi-agent") return "";
    return providerId || "";
  });

  const [piAgentBaseUrl, setPiAgentBaseUrl] = useState<string>(() => {
    if (appId !== "pi-agent") return "";
    return parsePiAgentField(initialData, "baseUrl", "");
  });

  const [piAgentApiKey, setPiAgentApiKey] = useState<string>(() => {
    if (appId !== "pi-agent") return "";
    return parsePiAgentField(initialData, "apiKey", "");
  });

  const [piAgentApi, setPiAgentApi] = useState<PiAgentApiMode>(() => {
    if (appId !== "pi-agent") return PI_AGENT_DEFAULT_API_MODE;
    const stored = parsePiAgentField<PiAgentApiMode | "">(
      initialData,
      "api",
      "",
    );
    return stored || PI_AGENT_DEFAULT_API_MODE;
  });

  const [piAgentModels, setPiAgentModels] = useState<PiAgentModel[]>(() => {
    if (appId !== "pi-agent") return [];
    return parsePiAgentField<PiAgentModel[]>(initialData, "models", []);
  });

  const updatePiAgentConfig = useCallback(
    (updater: (config: Record<string, unknown>) => void) => {
      try {
        const config = JSON.parse(
          getSettingsConfig() || PI_AGENT_DEFAULT_CONFIG,
        );
        updater(config);
        onSettingsConfigChange(JSON.stringify(config, null, 2));
      } catch {
        // ignore
      }
    },
    [getSettingsConfig, onSettingsConfigChange],
  );

  const handlePiAgentBaseUrlChange = useCallback(
    (baseUrl: string) => {
      setPiAgentBaseUrl(baseUrl);
      updatePiAgentConfig((config) => {
        config.baseUrl = baseUrl.trim().replace(/\/+$/, "");
      });
    },
    [updatePiAgentConfig],
  );

  const handlePiAgentApiKeyChange = useCallback(
    (apiKey: string) => {
      setPiAgentApiKey(apiKey);
      updatePiAgentConfig((config) => {
        config.apiKey = apiKey;
      });
    },
    [updatePiAgentConfig],
  );

  const handlePiAgentApiChange = useCallback(
    (api: PiAgentApiMode) => {
      setPiAgentApi(api);
      updatePiAgentConfig((config) => {
        config.api = api;
      });
    },
    [updatePiAgentConfig],
  );

  const handlePiAgentModelsChange = useCallback(
    (models: PiAgentModel[]) => {
      setPiAgentModels(models);
      updatePiAgentConfig((config) => {
        if (models.length === 0) {
          delete config.models;
        } else {
          config.models = models;
        }
      });
    },
    [updatePiAgentConfig],
  );

  const resetPiAgentState = useCallback(
    (config?: Partial<PiAgentProviderSettingsConfig>) => {
      setPiAgentProviderKey("");
      setPiAgentBaseUrl(config?.baseUrl || "");
      setPiAgentApiKey(config?.apiKey || "");
      setPiAgentApi(config?.api ?? PI_AGENT_DEFAULT_API_MODE);
      setPiAgentModels(config?.models ?? []);
    },
    [],
  );

  return {
    piAgentProviderKey,
    setPiAgentProviderKey,
    piAgentBaseUrl,
    piAgentApiKey,
    piAgentApi,
    piAgentModels,
    existingPiAgentKeys,
    handlePiAgentBaseUrlChange,
    handlePiAgentApiKeyChange,
    handlePiAgentApiChange,
    handlePiAgentModelsChange,
    resetPiAgentState,
  };
}
