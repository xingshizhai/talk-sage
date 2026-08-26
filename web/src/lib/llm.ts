import type { AppConfig } from "./api";

/** 读某个 LLM provider 已保存的 API Key；没有该 provider 或未配置时返回空串。 */
export function llmProviderApiKey(config: AppConfig | null | undefined, provider: string): string {
  if (!provider) {
    return "";
  }
  return config?.llm?.providers?.[provider]?.api_key ?? "";
}
