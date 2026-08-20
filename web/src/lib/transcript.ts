// 转写行聚合逻辑（纯 TypeScript，无 React 依赖，便于单元测试）。
//
// 规则：
// - partial 事件：若无未完成行 → 新增一行（标记 ▍）；若已有未完成行 → 原地更新文本
// - final 事件：把未完成行固化为最终文本；若无未完成行 → 直接新增一行

export interface TranscriptLine {
  key: number;
  speakerLabel: string;
  text: string;
  isPartial: boolean;
  tsMs: number;
}

/** segment 事件的最小形状（与 Rust DomainEvent::Segment 对应）。 */
export interface SegmentEvent {
  speaker_label: string;
  text: string;
  is_partial: boolean;
  ts_ms?: number;
}

/** 转写行聚合器：增量处理 segment 事件，输出稳定的行列表。 */
export class TranscriptAccumulator {
  private lines: TranscriptLine[] = [];
  private lastPartialKey: number | null = null;
  private nextKey = 0;

  /** 当前行列表（只读视图）。 */
  getLines(): readonly TranscriptLine[] {
    return this.lines;
  }

  /** 处理一条 segment 事件。 */
  push(seg: SegmentEvent): void {
    const tsMs = seg.ts_ms ?? Date.now();
    if (seg.is_partial) {
      if (this.lastPartialKey !== null) {
        // 更新未完成行
        this.lines = this.lines.map((l) =>
          l.key === this.lastPartialKey ? { ...l, text: seg.text, isPartial: true, tsMs } : l,
        );
      } else {
        // 新起一行
        const key = this.nextKey++;
        this.lastPartialKey = key;
        this.lines = [...this.lines, { key, speakerLabel: seg.speaker_label, text: seg.text, isPartial: true, tsMs }];
      }
    } else {
      if (this.lastPartialKey !== null) {
        // 固化未完成行
        const key = this.lastPartialKey;
        this.lastPartialKey = null;
        this.lines = this.lines.map((l) => (l.key === key ? { ...l, text: seg.text, isPartial: false, tsMs } : l));
      } else {
        // 直接新增一行
        const key = this.nextKey++;
        this.lines = [...this.lines, { key, speakerLabel: seg.speaker_label, text: seg.text, isPartial: false, tsMs }];
      }
    }
  }
}

// ── 分句（ASR 流式输出通常无标点，按句末标点 / 弱切分 / 长度软断提高可读性）──

/** 句末标点：切新句。 */
const SENTENCE_END = /[。！？；…\n]+/;
/** 弱边界（保持句中，但可断行）：中文逗号/顿号/分号、英文逗号/空格。 */
const WEAK_BOUNDARY = /[，、,;；]/;
/** 单句软断最大字符数（无任何边界时的兜底）。 */
const SOFT_LIMIT = 28;

/**
 * 把一段转写文本拆成句子列表：
 * 1. 按句末标点（。！？；… 换行）切分，保留边界字符；
 * 2. 单句仍超长时，优先在弱边界（，、,;）处断；
 * 3. 仍超长则在最近空白处软断（中文按字符数兜底）。
 * 结果过滤空白句。
 */
export function splitSentences(text: string): string[] {
  const raw = String(text ?? "");
  if (!raw.trim()) return [];
  const parts: string[] = [];
  let buf = "";
  for (const ch of raw) {
    buf += ch;
    if (SENTENCE_END.test(ch)) {
      parts.push(buf);
      buf = "";
    }
  }
  if (buf.trim()) parts.push(buf);
  if (parts.length === 0 && raw.trim()) parts.push(raw);

  const out: string[] = [];
  for (const p of parts) {
    if (p.length <= SOFT_LIMIT) {
      const t = p.trim();
      if (t) out.push(t);
      continue;
    }
    // 长句：优先弱边界断行
    let seg = "";
    let start = 0;
    for (let i = 0; i < p.length; i++) {
      const ch = p[i];
      const isBoundary = WEAK_BOUNDARY.test(ch) || ch === " ";
      if (isBoundary && i - start >= 10) {
        seg = p.slice(start, i + 1).trim();
        if (seg) out.push(seg);
        start = i + 1;
      } else if (i - start >= SOFT_LIMIT) {
        // 软断：在最后 10 字符内找空白/弱边界，否则字符硬断
        const window = p.slice(start, i);
        const lastSpace = Math.max(window.lastIndexOf(" "), window.lastIndexOf("，"), window.lastIndexOf("、"));
        const cut = lastSpace > 0 ? start + lastSpace + 1 : i;
        seg = p.slice(start, cut).trim();
        if (seg) out.push(seg);
        start = cut;
      }
    }
    const tail = p.slice(start).trim();
    if (tail) out.push(tail);
  }
  return out;
}
