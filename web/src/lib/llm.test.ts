import { describe, expect, it } from "vitest";
import type { AppConfig } from "./api";
import { llmProviderApiKey } from "./llm";

function cfg(providers: AppConfig["llm"]["providers"], def = "deepseek"): AppConfig {
  return { llm: { default: def, providers } } as AppConfig;
}

describe("llmProviderApiKey", () => {
  it("returns the saved key for the selected provider", () => {
    expect(
      llmProviderApiKey(
        cfg({
          deepseek: { base_url: "https://api.deepseek.com/v1", model: "deepseek-chat", api_key: "sk-from-file" },
        }),
        "deepseek",
      ),
    ).toBe("sk-from-file");
  });

  it("returns empty when config or provider is missing", () => {
    expect(llmProviderApiKey(null, "deepseek")).toBe("");
    expect(llmProviderApiKey(cfg({}), "deepseek")).toBe("");
    expect(
      llmProviderApiKey(
        cfg({
          deepseek: { base_url: null, model: "deepseek-chat", api_key: "sk-x" },
        }),
        "kimi",
      ),
    ).toBe("");
  });
});
