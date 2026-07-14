import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";
import { FormLabel } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { isValidUserAgentHeader } from "@/lib/userAgent";

/**
 * 自定义 User-Agent 预设列表
 *
 * 取值来自 PR #3671 对 Kimi Coding Plan（api.kimi.com/coding）UA 白名单的 curl 实测：
 * `claude-cli/*`、`claude-code/*`、`Kilo-Code/*` 可通过；`codex-cli`、`kimi-cli` 会被 403。
 * 白名单只校验 UA 名称前缀、不看版本号，因此用静态值即可，版本不会因 Claude Code 升级而失效。
 *
 * 第一条是官方 Claude Code CLI 实际发送的完整格式（参见 `stream_check.rs` 里检测用的
 * `claude-cli/2.1.2 (external, cli)`），最贴近真实客户端、最稳过严格的 UA 校验；其余为简短变体。
 */
const USER_AGENT_PRESETS: readonly string[] = [
  "claude-cli/2.1.161 (external, cli)",
  "claude-cli/2.1.161",
  "claude-code/1.0.0",
  "claude-code/0.1.0",
  "Kilo-Code/1.0",
];

interface CustomUserAgentFieldProps {
  /** 输入框的 id（用于 label htmlFor）；两个表单需传入各自唯一值。 */
  id: string;
  value: string;
  onChange: (value: string) => void;
}

/**
 * 供应商级自定义 User-Agent 字段（Claude / Codex 表单共用）。
 *
 * 含标签 + 输入框 + 右侧预设下拉菜单 + 实时合法性提示。校验口径与后端
 * `parse_custom_user_agent` 一致（见 `@/lib/userAgent`），非法时给非阻断红字提示
 * （运行时仍会静默忽略）。
 */
export function CustomUserAgentField({
  id,
  value,
  onChange,
}: CustomUserAgentFieldProps) {
  const { t } = useTranslation();
  const valid = isValidUserAgentHeader(value);

  return (
    <div className="space-y-2">
      <FormLabel htmlFor={id}>
        {t("providerForm.customUserAgent", {
          defaultValue: "自定义 User-Agent",
        })}
      </FormLabel>
      <div className="flex items-center gap-2">
        <Input
          id={id}
          type="text"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="Mozilla/5.0 ..."
          autoComplete="off"
          className="flex-1"
        />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" className="shrink-0 gap-1">
              {t("providerForm.customUserAgentPresets", {
                defaultValue: "预设",
              })}
              <ChevronDown className="h-3.5 w-3.5 opacity-60" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            className="max-h-64 overflow-y-auto z-[200]"
          >
            {USER_AGENT_PRESETS.map((preset) => (
              <DropdownMenuItem
                key={preset}
                onSelect={() => onChange(preset)}
                className="font-mono text-xs"
              >
                {preset}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      {valid ? (
        <p className="text-xs text-muted-foreground">
          {t("providerForm.customUserAgentHint", {
            defaultValue:
              "仅在开启本地路由/代理接管后生效，会替换转发到供应商 API 请求中的 User-Agent。",
          })}
        </p>
      ) : (
        <p className="text-xs text-destructive">
          {t("providerForm.customUserAgentInvalid", {
            defaultValue:
              "User-Agent 不能包含控制字符（如换行符），否则将被忽略。",
          })}
        </p>
      )}
    </div>
  );
}
