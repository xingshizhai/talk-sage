// 要点展示：分类标签映射。抽取已迁到 Rust 插件 `key_point_extractor`。

export type KeyPointKind = "问句" | "要求" | "决策" | "行动" | "技术" | "其他";

export interface KeyPoint {
  resultId: string;
  kind: KeyPointKind;
  text: string;
  tsMs: number;
  manual: boolean;
}

const CATEGORY_LABEL: Record<string, KeyPointKind> = {
  question: "问句",
  requirement: "要求",
  decision: "决策",
  action: "行动",
  technical: "技术",
  other: "其他",
};

/** 后端 `KeyPointCategory`（snake_case）→ 中文标签。 */
export function categoryLabel(category: string): KeyPointKind {
  return CATEGORY_LABEL[category] ?? "其他";
}

/** 把插件/会话里的要点记录转成卡片用的结构。 */
export function toKeyPoint(input: { result_id: string; category: string; content: string; ts_ms?: number; manual?: boolean }): KeyPoint {
  return {
    resultId: input.result_id,
    kind: categoryLabel(input.category),
    text: input.content,
    tsMs: input.ts_ms ?? 0,
    manual: input.manual ?? false,
  };
}

/** 文本噪音评分：0（正常语言）~ 1（纯噪音/乱码）。与 Rust 侧 text_noise_score 同算法。 */
export function textNoiseScore(text: string): number {
  if (!text) return 1;
  const chars = Array.from(text);
  const total = chars.length;
  if (total < 2) return 1;
  const FILLERS = new Set("嗯啊哦唉哎呢呀吧啦嘛么呃嘿哈".split(""));
  let filler = 0;
  let runChars = 0;
  let runLen = 0;
  let meaningful = 0;
  let prev: string | null = null;
  for (const c of chars) {
    if (FILLERS.has(c)) filler++;
    const isCn = c >= "\u4e00" && c <= "\u9fff";
    const isEn = /[a-zA-Z0-9]/.test(c);
    if (isCn || isEn) meaningful++;
    if (prev === c) runLen++;
    else runLen = 1;
    if (runLen >= 2) runChars++;
    prev = c;
  }
  const runRatio = runChars / total;
  const fillerRatio = filler / total;
  const meaningfulRatio = meaningful / total;
  // bigram 多样性：重复短语（"那个个那个个"）的相邻二元组高度重复 → 噪音
  const bigramSet = new Set<string>();
  for (let i = 0; i < chars.length - 1; i++) {
    bigramSet.add(chars[i] + chars[i + 1]);
  }
  const bigramTotal = chars.length - 1;
  const bigramUnique = bigramTotal > 0 ? bigramSet.size / bigramTotal : 1;
  return Math.max(
    0,
    Math.min(1, (1 - bigramUnique) * 0.5 + runRatio * 0.4 + fillerRatio * 0.2 + (1 - meaningfulRatio) * 0.1),
  );
}
