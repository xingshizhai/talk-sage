import { describe, expect, it } from "vitest";
import type { AppConfig } from "./api";
import { knowledgeBaseSettings } from "./knowledge";

function cfg(kb?: AppConfig["knowledge_base"]): AppConfig {
  return { knowledge_base: kb } as AppConfig;
}

describe("knowledgeBaseSettings", () => {
  it("reads enabled folder from config", () => {
    expect(
      knowledgeBaseSettings(
        cfg({ enabled: true, folder: "D:\\Obsidian" }),
      ),
    ).toEqual({ enabled: true, folder: "D:\\Obsidian" });
  });

  it("defaults when config is missing", () => {
    expect(knowledgeBaseSettings(null)).toEqual({ enabled: false, folder: "" });
    expect(knowledgeBaseSettings(cfg(undefined))).toEqual({ enabled: false, folder: "" });
  });
});
