import { useTranslation } from "react-i18next";
import {
  useState,
  useRef,
  useCallback,
  useMemo,
  type ReactNode,
} from "react";
import { FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { toast } from "sonner";
import {
  Download,
  Plus,
  Trash2,
  ChevronDown,
  ChevronRight,
  Loader2,
} from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ApiKeySection } from "./shared";
import {
  fetchModelsForConfig,
  showFetchModelsError,
  type FetchedModel,
} from "@/lib/api/model-fetch";
import type { ProviderCategory } from "@/types";
import { Checkbox } from "@/components/ui/checkbox";
import type { PiAgentApiMode, PiAgentModel } from "./hooks/usePiAgentFormState";

export interface PiAgentFormFieldsProps {
  baseUrl: string;
  onBaseUrlChange: (value: string) => void;
  apiKey: string;
  onApiKeyChange: (value: string) => void;
  category?: ProviderCategory;
  shouldShowApiKeyLink: boolean;
  websiteUrl: string;
  isPartner?: boolean;
  partnerPromotionKey?: string;
  api: PiAgentApiMode;
  onApiChange: (api: PiAgentApiMode) => void;
  models: PiAgentModel[];
  onModelsChange: (models: PiAgentModel[]) => void;
}

type BaseUrlErrorCode = "empty" | "invalid" | "scheme";

const BASE_URL_ERROR_I18N_KEY: Record<BaseUrlErrorCode, string> = {
  empty: "piAgent.form.baseUrlRequired",
  scheme: "piAgent.form.baseUrlScheme",
  invalid: "piAgent.form.baseUrlInvalid",
};

const TEMPLATE_TOKEN_RE = /\$\{[^}]+\}/g;

function validateBaseUrl(raw: string): BaseUrlErrorCode | null {
  const trimmed = raw.trim();
  if (!trimmed) return "empty";
  const candidate = trimmed.replace(TEMPLATE_TOKEN_RE, "placeholder");
  let u: URL;
  try {
    u = new URL(candidate);
  } catch {
    return "invalid";
  }
  if (!u.protocol.startsWith("http")) return "scheme";
  if (!u.hostname) return "invalid";
  return null;
}

interface AdvancedSectionProps {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  labelKey: string;
  children: ReactNode;
}

function AdvancedSection({
  open,
  onOpenChange,
  labelKey,
  children,
}: AdvancedSectionProps) {
  const { t } = useTranslation();
  return (
    <Collapsible open={open} onOpenChange={onOpenChange}>
      <CollapsibleTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-7 gap-1 text-xs text-muted-foreground hover:text-foreground"
        >
          {open ? (
            <ChevronDown className="h-3.5 w-3.5" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5" />
          )}
          {t(labelKey)}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="space-y-3 pt-2">
        {children}
      </CollapsibleContent>
    </Collapsible>
  );
}

interface PiAgentCostEditorProps {
  cost: PiAgentModel["cost"];
  onChange: (cost: PiAgentModel["cost"]) => void;
}

function PiAgentCostEditor({ cost, onChange }: PiAgentCostEditorProps) {
  const { t } = useTranslation();

  const costFields = [
    { key: "input", label: "piAgent.form.costInput" },
    { key: "output", label: "piAgent.form.costOutput" },
    { key: "cacheRead", label: "piAgent.form.costCacheRead" },
    { key: "cacheWrite", label: "piAgent.form.costCacheWrite" },
  ] as const;

  return (
    <div className="grid grid-cols-2 gap-2">
      {costFields.map(({ key, label }) => (
        <div key={key} className="space-y-1">
          <label className="text-xs text-muted-foreground">
            {t(label)}
          </label>
          <Input
            type="number"
            step="0.01"
            min="0"
            value={cost?.[key as keyof typeof cost] ?? ""}
            onChange={(e) => {
              const v = e.target.value;
              const n = v === "" ? undefined : parseFloat(v);
              onChange({
                ...cost,
                [key]: n !== undefined && Number.isFinite(n) ? n : undefined,
              });
            }}
            placeholder="0"
          />
        </div>
      ))}
    </div>
  );
}

export function PiAgentFormFields({
  baseUrl,
  onBaseUrlChange,
  apiKey,
  onApiKeyChange,
  category,
  shouldShowApiKeyLink,
  websiteUrl,
  isPartner,
  partnerPromotionKey,
  api,
  onApiChange,
  models,
  onModelsChange,
}: PiAgentFormFieldsProps) {
  const { t } = useTranslation();
  const [expandedModels, setExpandedModels] = useState<Record<number, boolean>>(
    {},
  );
  const [fetchedModels, setFetchedModels] = useState<FetchedModel[]>([]);
  const [isFetchingModels, setIsFetchingModels] = useState(false);
  const [baseUrlTouched, setBaseUrlTouched] = useState(false);

  const baseUrlErrorCode = useMemo(() => validateBaseUrl(baseUrl), [baseUrl]);
  const showBaseUrlError = baseUrlTouched && baseUrlErrorCode !== null;
  const baseUrlErrorMessage = baseUrlErrorCode
    ? t(BASE_URL_ERROR_I18N_KEY[baseUrlErrorCode])
    : "";

  const modelKeysRef = useRef<string[]>([]);
  while (modelKeysRef.current.length < models.length) {
    modelKeysRef.current.push(crypto.randomUUID());
  }
  if (modelKeysRef.current.length > models.length) {
    modelKeysRef.current.length = models.length;
  }
  const modelKeys = modelKeysRef.current;

  const groupedFetchedModels = useMemo(
    () =>
      Object.entries(
        fetchedModels.reduce(
          (acc, m) => {
            const v = m.ownedBy || "Other";
            if (!acc[v]) acc[v] = [];
            acc[v].push(m);
            return acc;
          },
          {} as Record<string, FetchedModel[]>,
        ),
      ).sort(([a], [b]) => a.localeCompare(b)),
    [fetchedModels],
  );

  const toggleModelAdvanced = (index: number) => {
    setExpandedModels((prev) => ({ ...prev, [index]: !prev[index] }));
  };

  const handleAddModel = () => {
    modelKeysRef.current.push(crypto.randomUUID());
    onModelsChange([
      ...models,
      {
        id: "",
        name: "",
        contextWindow: undefined,
        maxTokens: undefined,
        reasoning: false,
        cost: undefined,
      },
    ]);
  };

  const handleFetchModels = useCallback(() => {
    if (!baseUrl || !apiKey) {
      showFetchModelsError(null, t, {
        hasApiKey: !!apiKey,
        hasBaseUrl: !!baseUrl,
      });
      return;
    }
    setIsFetchingModels(true);
    fetchModelsForConfig(baseUrl, apiKey)
      .then((fetched) => {
        setFetchedModels(fetched);
        if (fetched.length === 0) {
          toast.info(t("providerForm.fetchModelsEmpty"));
        } else {
          toast.success(
            t("providerForm.fetchModelsSuccess", { count: fetched.length }),
          );
        }
      })
      .catch((err) => {
        console.warn("[ModelFetch] Failed:", err);
        showFetchModelsError(err, t);
      })
      .finally(() => setIsFetchingModels(false));
  }, [baseUrl, apiKey, t]);

  const handleRemoveModel = (index: number) => {
    modelKeysRef.current.splice(index, 1);
    const next = [...models];
    next.splice(index, 1);
    onModelsChange(next);
    setExpandedModels((prev) => {
      const updated = { ...prev };
      delete updated[index];
      return updated;
    });
  };

  const handleModelChange = (
    index: number,
    field: keyof PiAgentModel,
    value: unknown,
  ) => {
    const next = [...models];
    next[index] = { ...next[index], [field]: value };
    onModelsChange(next);
  };

  const piAgentApiOptions = useMemo(
    () => [
      { value: "openai-completions", label: "piAgent.form.apiModeOpenAI" },
      { value: "anthropic-messages", label: "piAgent.form.apiModeAnthropic" },
    ],
    [],
  );

  return (
    <>
      <div className="space-y-2">
        <FormLabel htmlFor="pi-agent-api">
          {t("piAgent.form.api", { defaultValue: "API 类型" })}
        </FormLabel>
        <Select
          value={api}
          onValueChange={(v) => onApiChange(v as PiAgentApiMode)}
        >
          <SelectTrigger id="pi-agent-api">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {piAgentApiOptions.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {t(opt.label)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          {t("piAgent.form.apiHint", {
            defaultValue: "供应商 API 协议。请根据端点选择正确的协议。",
          })}
        </p>
      </div>

      <div className="space-y-2">
        <FormLabel htmlFor="pi-agent-baseurl">
          {t("piAgent.form.baseUrl", { defaultValue: "API 端点" })}
        </FormLabel>
        <Input
          id="pi-agent-baseurl"
          value={baseUrl}
          onChange={(e) => onBaseUrlChange(e.target.value)}
          onBlur={() => setBaseUrlTouched(true)}
          placeholder="https://api.example.com/v1"
          aria-invalid={showBaseUrlError}
          className={
            showBaseUrlError
              ? "border-destructive focus-visible:ring-destructive"
              : undefined
          }
        />
        {showBaseUrlError ? (
          <p className="text-xs text-destructive">{baseUrlErrorMessage}</p>
        ) : (
          <p className="text-xs text-muted-foreground">
            {t("piAgent.form.baseUrlHint", {
              defaultValue: "供应商的 API 端点地址。",
            })}
          </p>
        )}
      </div>

      <ApiKeySection
        value={apiKey}
        onChange={onApiKeyChange}
        category={category === "official" ? undefined : category}
        shouldShowLink={shouldShowApiKeyLink}
        websiteUrl={websiteUrl}
        isPartner={isPartner}
        partnerPromotionKey={partnerPromotionKey}
      />

      <div className="space-y-3">
        <div className="flex items-center justify-between">
          <FormLabel>
            {t("piAgent.form.models", { defaultValue: "模型列表" })}
          </FormLabel>
          <div className="flex gap-1">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleFetchModels}
              disabled={isFetchingModels}
              className="h-7 gap-1"
            >
              {isFetchingModels ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Download className="h-3.5 w-3.5" />
              )}
              {t("providerForm.fetchModels")}
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleAddModel}
              className="h-7 gap-1"
            >
              <Plus className="h-3.5 w-3.5" />
              {t("piAgent.form.addModel", { defaultValue: "添加模型" })}
            </Button>
          </div>
        </div>

        {models.length === 0 ? (
          <p className="text-sm text-muted-foreground py-2">
            {t("piAgent.form.noModels", {
              defaultValue: "暂无模型配置。",
            })}
          </p>
        ) : (
          <div className="space-y-4">
            {models.map((model, index) => (
              <div
                key={modelKeys[index]}
                className="p-3 border border-border/50 rounded-lg space-y-3"
              >
                <div className="flex items-center">
                  <span
                    className={`text-[10px] font-medium px-1.5 py-0.5 rounded ${
                      index === 0
                        ? "bg-blue-500/15 text-blue-600 dark:text-blue-400"
                        : "bg-muted text-muted-foreground"
                    }`}
                  >
                    {index === 0
                      ? t("piAgent.form.primaryModel", {
                          defaultValue: "默认模型",
                        })
                      : t("piAgent.form.fallbackModel", {
                          defaultValue: "备选模型",
                        })}
                  </span>
                </div>

                <div className="flex items-center gap-2">
                  <div className="flex-1 space-y-1">
                    <label className="text-xs text-muted-foreground">
                      {t("piAgent.form.modelId", { defaultValue: "模型 ID" })}
                    </label>
                    <div className="flex gap-1">
                      <Input
                        value={model.id}
                        onChange={(e) =>
                          handleModelChange(index, "id", e.target.value)
                        }
                        placeholder={t("piAgent.form.modelIdPlaceholder", {
                          defaultValue: "anthropic/claude-opus-4-8",
                        })}
                        className="flex-1"
                      />
                      {fetchedModels.length > 0 && (
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              variant="outline"
                              size="icon"
                              className="shrink-0"
                            >
                              <ChevronDown className="h-4 w-4" />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent
                            align="end"
                            className="max-h-64 overflow-y-auto z-[200]"
                          >
                            {groupedFetchedModels.map(
                              ([vendor, vModels], vi) => (
                                <div key={vendor}>
                                  {vi > 0 && <DropdownMenuSeparator />}
                                  <DropdownMenuLabel>
                                    {vendor}
                                  </DropdownMenuLabel>
                                  {vModels.map((m) => (
                                    <DropdownMenuItem
                                      key={m.id}
                                      onSelect={() =>
                                        handleModelChange(index, "id", m.id)
                                      }
                                    >
                                      {m.id}
                                    </DropdownMenuItem>
                                  ))}
                                </div>
                              ),
                            )}
                          </DropdownMenuContent>
                        </DropdownMenu>
                      )}
                    </div>
                  </div>
                  <div className="flex-1 space-y-1">
                    <label className="text-xs text-muted-foreground">
                      {t("piAgent.form.modelName", {
                        defaultValue: "显示名称",
                      })}
                    </label>
                    <Input
                      value={model.name ?? ""}
                      onChange={(e) =>
                        handleModelChange(index, "name", e.target.value)
                      }
                      placeholder={t("piAgent.form.modelNamePlaceholder", {
                        defaultValue: "Claude Opus 4.8",
                      })}
                    />
                  </div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => handleRemoveModel(index)}
                    className="h-9 w-9 mt-5 text-muted-foreground hover:text-destructive"
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>

                <AdvancedSection
                  open={expandedModels[index] ?? false}
                  onOpenChange={() => toggleModelAdvanced(index)}
                  labelKey="piAgent.form.advancedOptions"
                >
                  <div className="grid grid-cols-2 gap-3">
                    <div className="space-y-1">
                      <label className="text-xs text-muted-foreground">
                        {t("piAgent.form.contextWindow", {
                          defaultValue: "上下文长度",
                        })}
                      </label>
                      <Input
                        type="number"
                        min="0"
                        value={model.contextWindow ?? ""}
                        onChange={(e) => {
                          const v = e.target.value;
                          handleModelChange(
                            index,
                            "contextWindow",
                            v ? Number.parseInt(v, 10) || 0 : undefined,
                          );
                        }}
                        placeholder="200000"
                      />
                    </div>
                    <div className="space-y-1">
                      <label className="text-xs text-muted-foreground">
                        {t("piAgent.form.maxTokens", {
                          defaultValue: "最大输出 Token",
                        })}
                      </label>
                      <Input
                        type="number"
                        min="0"
                        value={model.maxTokens ?? ""}
                        onChange={(e) => {
                          const v = e.target.value;
                          handleModelChange(
                            index,
                            "maxTokens",
                            v ? Number.parseInt(v, 10) || 0 : undefined,
                          );
                        }}
                        placeholder="16384"
                      />
                    </div>
                  </div>

                  <div className="flex items-center gap-2 pt-1">
                    <Checkbox
                      id={`reasoning-${modelKeys[index]}`}
                      checked={model.reasoning ?? false}
                      onCheckedChange={(checked) =>
                        handleModelChange(index, "reasoning", checked === true)
                      }
                    />
                    <label
                      htmlFor={`reasoning-${modelKeys[index]}`}
                      className="text-xs text-muted-foreground cursor-pointer"
                    >
                      {t("piAgent.form.reasoning", {
                        defaultValue: "支持推理（Reasoning）",
                      })}
                    </label>
                  </div>

                  <div className="space-y-1">
                    <label className="text-xs text-muted-foreground">
                      {t("piAgent.form.cost", {
                        defaultValue: "费用（每百万 Token）",
                      })}
                    </label>
                    <PiAgentCostEditor
                      cost={model.cost}
                      onChange={(cost) =>
                        handleModelChange(index, "cost", cost)
                      }
                    />
                  </div>
                </AdvancedSection>
              </div>
            ))}
          </div>
        )}

        <p className="text-xs text-muted-foreground">
          {t("piAgent.form.modelsHint", {
            defaultValue: "第一个模型会作为默认模型。",
          })}
        </p>
      </div>
    </>
  );
}
