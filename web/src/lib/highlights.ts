// 要点聚合：本地规则从转写提取关键信息（问句/要求/决策/行动/技术/其他）。

export type KeyPointKind = "问句" | "要求" | "决策" | "行动" | "技术" | "其他";

export interface KeyPoint {
  kind: KeyPointKind;
  text: string;
  tsMs: number;
}

// 注意：JS 正则 \b 对中文无效（中文非 \w），中文关键词用裸词匹配，英文用 \b。
const QUESTION_RE = /[?？]|\b(what|how|why|when|where|who|which|should|could|would|can|do|does|is|are)\b|吗|呢|怎么|什么|多少|能不能|要不要|是否/i;
const DECISION_RE = /\b(agreed|go with|decided|proceed|settled|confirmed)\b|确认|决定|就定|采用|拍板|定了|达成|结论/i;
const REQUIREMENT_RE = /\b(need|require|must|should|wants?)\b|要求|需要|必须|希望|期望|交期|价格|报价|样品|MOQ|NPI|预算|指标/i;
const TECH_RE = /方案|架构|接口|协议|版本|兼容|性能|延迟|并发|部署|迁移|API|SDK|规范|数据库|服务器|前端|后端/i;
// 行动：具体下一步（负责人/动作 + 时间或数字）
const ACTION_RE = /\b(send|submit|deliver|schedule|arrange|follow ?up|call|email|write|prepare|review)\b|提交|发送|发给|安排|跟进|汇总|整理|确认|通知|约|联系|更新|上线|交付|截止|之前|之后|明天|下周|本周|月底|月初|下午|上午|\d{1,4}\s*(台|套|个|万|元|%|批|台|件)/i;
// 数字+单位 / 金额（"50万"、"300台"、"15%"、"Q3"、"6月"）
const NUMERIC_RE = /\d{1,4}\s*(台|套|个|件|万|亿|元|块|%|批|台|日|号|月|周|点|人)|[0-9]{2,4}|[Qq][1-4]|20\d{2}/;

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

/** 句末切分（复用转写分句的轻量版：仅按句末标点与弱边界拆，供逐句判定）。 */
function splitForKp(text: string): string[] {
  return text
    .split(/[。！？；…\n]+/)
    .flatMap((s) => s.split(/[，、,;]/))
    .map((s) => s.trim())
    .filter((s) => s.length >= 2);
}

/** 判定单句要点类别（含数字/时间/行动启发式）。 */
function kindOf(sentence: string): KeyPointKind | null {
  if (QUESTION_RE.test(sentence)) return "问句";
  if (DECISION_RE.test(sentence)) return "决策";
  if (ACTION_RE.test(sentence) || NUMERIC_RE.test(sentence)) return "行动";
  if (REQUIREMENT_RE.test(sentence)) return "要求";
  if (TECH_RE.test(sentence)) return "技术";
  // 含明确动作动词 + 宾语（"我们下周一交付"）→ 行动
  if (/[我你他她]们?/.test(sentence) && /(交付|提交|发送|安排|跟进|确认|做|完成|给)/.test(sentence)) return "行动";
  return null;
}

/** 从一条 final 段提取要点（按句判定，最多 3 条/段，去重）。 */
export function extractKeyPoint(text: string, tsMs: number): KeyPoint[] {
  const t = text.trim();
  if (t.length < 4) return [];
  if (textNoiseScore(t) > NOISE_THRESHOLD) return []; // 噪音/乱码段跳过
  const sentences = splitForKp(t);
  const out: KeyPoint[] = [];
  const seen = new Set<string>();
  for (const s of sentences) {
    const kind = kindOf(s);
    if (!kind) continue;
    // 问句/技术等类别需要足够信息量；行动类含数字/时间可放宽
    const minLen = kind === "行动" ? 6 : 8;
    if (s.length < minLen) continue;
    const key = s;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({ kind, text: s.length > 120 ? `${s.slice(0, 120)}…` : s, tsMs });
    if (out.length >= 3) break;
  }
  return out;
}

/** 增量聚合器：维护要点列表（去重，按时间）。 */
export class KeyPointAggregator {
  private items: KeyPoint[] = [];

  getItems(): readonly KeyPoint[] {
    return this.items;
  }

  push(text: string, tsMs: number): boolean {
    const kps = extractKeyPoint(text, tsMs);
    if (kps.length === 0) return false;
    let added = false;
    for (const kp of kps) {
      // 去重：与最近 5 条相似则跳过
      const recent = this.items.slice(-5);
      if (recent.some((r) => r.text === kp.text)) continue;
      this.items.push(kp);
      added = true;
    }
    if (this.items.length > 80) {
      this.items = this.items.slice(-80);
    }
    return added;
  }
}
