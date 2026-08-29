import { describe, expect, it } from "vitest";
import type { AppConfig } from "./api";
import {
  knowledgeBaseSettings,
  knowledgeEmptyHint,
  knowledgeSourceReady,
  pinnedDocuments,
  togglePinnedPath,
  truncateNoteText,
  type KnowledgeDoc,
} from "./knowledge";

function cfg(kb?: AppConfig["knowledge_base"]): AppConfig {
  return { knowledge_base: kb } as AppConfig;
}

const DOCS: KnowledgeDoc[] = [
  { path: "wiki/a.md", title: "交期", text: "样品交期两周" },
  { path: "wiki/b.md", title: "价格", text: "MOQ 500" },
];

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

describe("knowledgeSourceReady", () => {
  it("requires both switch and folder", () => {
    expect(knowledgeSourceReady(cfg({ enabled: true, folder: "" }))).toBe(false);
    expect(knowledgeSourceReady(cfg({ enabled: false, folder: "D:\\x" }))).toBe(false);
    expect(knowledgeSourceReady(cfg({ enabled: true, folder: "D:\\x" }))).toBe(true);
  });
});

describe("togglePinnedPath", () => {
  it("adds then removes the same path", () => {
    expect(togglePinnedPath([], "wiki/a.md")).toEqual(["wiki/a.md"]);
    expect(togglePinnedPath(["wiki/a.md"], "wiki/a.md")).toEqual([]);
    expect(togglePinnedPath(["wiki/a.md"], "wiki/b.md")).toEqual(["wiki/a.md", "wiki/b.md"]);
  });
});

describe("pinnedDocuments", () => {
  it("keeps pin order and skips missing paths", () => {
    expect(pinnedDocuments(DOCS, ["wiki/b.md", "gone.md", "wiki/a.md"])).toEqual([
      DOCS[1],
      DOCS[0],
    ]);
  });
});

describe("truncateNoteText", () => {
  it("leaves short notes unchanged", () => {
    expect(truncateNoteText("短")).toBe("短");
  });

  it("ellipsis after max chars", () => {
    expect(truncateNoteText("abcdef", 4)).toBe("abcd…");
  });
});

describe("knowledgeEmptyHint", () => {
  it("guides setup, empty vault, then pin action", () => {
    expect(knowledgeEmptyHint({ enabled: false, folder: "", docCount: 0, pinnedCount: 0 }))
      .toBe("在设置中启用 Obsidian 知识源");
    expect(knowledgeEmptyHint({ enabled: true, folder: "D:\\v", docCount: 0, pinnedCount: 0 }))
      .toBe("仓库里还没有可钉的笔记");
    expect(knowledgeEmptyHint({ enabled: true, folder: "D:\\v", docCount: 2, pinnedCount: 0 }))
      .toBe("从知识库钉住笔记，会中可对照");
    expect(knowledgeEmptyHint({ enabled: true, folder: "D:\\v", docCount: 2, pinnedCount: 1 }))
      .toBe("");
  });
});
