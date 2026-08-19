// 要点聚合：本地规则从转写提取关键信息（问句/要求/决策/其他）。

export type KeyPointKind = "问句" | "要求" | "决策" | "技术" | "其他";

export interface KeyPoint {
  kind: KeyPointKind;
  text: string;
  tsMs: number;
}

// 注意：JS 正则 \b 对中文无效（中文非 \w），中文关键词用裸词匹配，英文用 \b。
const QUESTION_RE = /[?？]|\b(what|how|why|when|where|who|which|should|could|would|can|do|does|is|are)\b|吗|呢|怎么|什么|多少/i;
const DECISION_RE = /\b(agreed|go with|decided|proceed|settled)\b|确认|决定|就定|采用|拍板/i;
const REQUIREMENT_RE = /\b(need|require|must|should|wants?)\b|要求|需要|必须|希望|期望|交期|价格|报价|样品|MOQ|NPI/i;
const TECH_RE = /方案|架构|接口|协议|版本|兼容|性能|延迟|并发|部署|迁移|API|SDK|规范/i;

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

/** 噪音段不参与要点聚合（阈值与质量评估一致，略保守）。 */
const NOISE_THRESHOLD = 0.5;

/** 从一条 final 段提取要点（最多 1 条/段，按优先级）。 */
export function extractKeyPoint(text: string, tsMs: number): KeyPoint | null {
  const t = text.trim();
  if (t.length < 4) return null;
  if (textNoiseScore(t) > NOISE_THRESHOLD) return null; // 噪音/乱码段跳过
  let kind: KeyPointKind = "其他";
  if (QUESTION_RE.test(t)) kind = "问句";
  else if (DECISION_RE.test(t)) kind = "决策";
  else if (REQUIREMENT_RE.test(t)) kind = "要求";
  else if (TECH_RE.test(t)) kind = "技术";
  if (kind === "其他" && t.length < 12) return null; // 太短且无特征 → 忽略
  return { kind, text: t.length > 120 ? `${t.slice(0, 120)}…` : t, tsMs };
}

/** 增量聚合器：维护要点列表（去重，按时间）。 */
export class KeyPointAggregator {
  private items: KeyPoint[] = [];

  getItems(): readonly KeyPoint[] {
    return this.items;
  }

  push(text: string, tsMs: number): boolean {
    const kp = extractKeyPoint(text, tsMs);
    if (!kp) return false;
    // 去重：与最近一条相似则跳过
    const last = this.items[this.items.length - 1];
    if (last && last.text === kp.text) return false;
    this.items.push(kp);
    if (this.items.length > 60) {
      this.items = this.items.slice(-60);
    }
    return true;
  }
}
