import type { AppConfig } from "./api";

export type KnowledgeDoc = {
  path: string;
  title: string;
  text: string;
};

/** 读已保存的知识库开关与文件夹；未配置时关闭、路径为空。 */
export function knowledgeBaseSettings(
  config: AppConfig | null | undefined,
): { enabled: boolean; folder: string } {
  return {
    enabled: config?.knowledge_base?.enabled ?? false,
    folder: config?.knowledge_base?.folder ?? "",
  };
}

/** 源已启用且填了 vault 路径时，转写页才去拉文档列表。 */
export function knowledgeSourceReady(
  config: AppConfig | null | undefined,
): boolean {
  const kb = knowledgeBaseSettings(config);
  return kb.enabled && kb.folder.trim().length > 0;
}

export function togglePinnedPath(pinned: readonly string[], path: string): string[] {
  return pinned.includes(path) ? pinned.filter((p) => p !== path) : [...pinned, path];
}

/** 按钉住顺序取出文档；已删路径跳过。 */
export function pinnedDocuments(
  docs: readonly KnowledgeDoc[],
  pinned: readonly string[],
): KnowledgeDoc[] {
  const byPath = new Map(docs.map((d) => [d.path, d]));
  return pinned.flatMap((path) => {
    const doc = byPath.get(path);
    return doc ? [doc] : [];
  });
}

export function truncateNoteText(text: string, max = 480): string {
  const t = text.trim();
  if (t.length <= max) return t;
  return `${t.slice(0, max)}…`;
}

export function knowledgeEmptyHint(opts: {
  enabled: boolean;
  folder: string;
  docCount: number;
  pinnedCount: number;
}): string {
  if (!opts.enabled || !opts.folder.trim()) return "在设置中启用 Obsidian 知识源";
  if (opts.docCount === 0) return "仓库里还没有可钉的笔记";
  if (opts.pinnedCount === 0) return "从知识库钉住笔记，会中可对照";
  return "";
}
