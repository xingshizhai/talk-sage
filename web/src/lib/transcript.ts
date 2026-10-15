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
  voiceId?: string;
  speakerRole?: "owner" | "client" | "other" | "unknown";
}

/** segment 事件的最小形状（与 Rust DomainEvent::Segment 对应）。 */
export interface SegmentEvent {
  speaker_id?: number;
  speaker_label: string;
  text: string;
  is_partial: boolean;
  ts_ms?: number;
  /**
   * 会话内单调递增的转写修订号（Rust `TranscriptState::apply` 盖的章）。
   * 只在快照重建后用于丢弃"快照已经包含、但服务端又补发了一遍"的事件。
   */
  revision?: number;
  speaker_attribution?: {
    source?: string;
    role: "owner" | "client" | "other" | "unknown";
    voice?: { id: string; confidence?: number };
  };
}

/**
 * 会议文字通道的来源标记（`AudioSource::TypedText`）。
 *
 * 打字段不经过 Rust 的 `TranscriptState`（它只包装 Segment 事件，见
 * `talk_sage` 的 `service.rs::inject_text`），因此 `revision` 恒为 0，
 * 也永远不会出现在快照里 —— 所以它必须豁免 revision 对齐，否则快照一到，
 * 后续所有打字段都会被当成"已过期"丢掉。
 */
const TYPED_TEXT_SOURCE = "typed_text";

/** 转写行聚合器：增量处理 segment 事件，输出稳定的行列表。 */
export class TranscriptAccumulator {
  private lines: TranscriptLine[] = [];
  /** 每个说话人当前未完成（partial）行的 key——双流时"我"与"客户"的增量互不覆盖。 */
  private partialKeyBySpeaker = new Map<string, number>();
  private nextKey = 0;
  /**
   * 最近一次快照的 revision；-1 = 没有快照基线，不做任何过滤。
   *
   * 服务端为避免"订阅与取快照之间"丢事件，改为**先订阅、后取快照**，代价是
   * 那一段窗口里的事件会被补发一次。这里的 revision 门槛就是用来挡掉这些
   * 已经被快照包含的事件，否则重连后会出现重复的转写行。
   */
  private snapshotRevision = -1;

  /** 当前行列表（只读视图）。 */
  getLines(): readonly TranscriptLine[] {
    return this.lines;
  }

  /** 清空（订阅快照重建前 / 新会话开始）。会一并清掉快照基线。 */
  reset(): void {
    this.lines = [];
    this.partialKeyBySpeaker.clear();
    this.nextKey = 0;
    this.snapshotRevision = -1;
  }

  /**
   * 用服务端快照重建 committed + 各说话人 hypothesis。
   *
   * `revision` 是快照对应的转写修订号；传入后，后续 `revision` 不大于它的
   * segment 事件会被丢弃（它们已经在 committed 里了）。
   */
  applySnapshot(committed: SegmentEvent[], hypothesis: SegmentEvent[], revision?: number): void {
    this.reset();
    // 先以"无基线"写入快照内容本身，再启用 revision 门槛 —— 快照里的段
    // 携带的 revision 恰好 <= 门槛，先设门槛会把它们自己判成"已覆盖"而丢光。
    for (const s of committed) {
      this.push({ ...s, is_partial: false });
    }
    for (const h of hypothesis) {
      this.push({ ...h, is_partial: true });
    }
    this.snapshotRevision =
      typeof revision === "number" && Number.isFinite(revision) && revision >= 0 ? revision : -1;
  }

  /**
   * 该事件是否已被当前快照覆盖（服务端补发的历史事件）。
   *
   * 打字段豁免：它不进 `TranscriptState`、`revision` 恒 0、也永不出现在快照里。
   * 缺 `revision` 的事件同样放行（兼容旧服务端），宁可重复也不静默丢内容。
   */
  private coveredBySnapshot(seg: SegmentEvent): boolean {
    if (this.snapshotRevision < 0) return false;
    if (seg.speaker_attribution?.source === TYPED_TEXT_SOURCE) return false;
    if (seg.revision === undefined) return false;
    return seg.revision <= this.snapshotRevision;
  }

  /** 处理一条 segment 事件。 */
  push(seg: SegmentEvent): void {
    if (this.coveredBySnapshot(seg)) return;
    const tsMs = seg.ts_ms ?? Date.now();
    const voiceId = seg.speaker_attribution?.voice?.id;
    const speakerRole = seg.speaker_attribution?.role;
    // 声纹只允许修改显示标签；稳定的通道/角色 ID 用于把 partial 与 final 对齐。
    const speakerKey = seg.speaker_id === undefined ? seg.speaker_label : `id:${seg.speaker_id}`;
    if (seg.is_partial) {
      const key = this.partialKeyBySpeaker.get(speakerKey);
      if (key !== undefined) {
        // 更新该说话人的未完成行
        this.lines = this.lines.map((l) =>
          l.key === key ? { ...l, text: seg.text, isPartial: true, tsMs } : l,
        );
      } else {
        // 该说话人新起一行（其他说话人的未完成行不受影响）
        const k = this.nextKey++;
        this.partialKeyBySpeaker.set(speakerKey, k);
        this.lines = [...this.lines, { key: k, speakerLabel: seg.speaker_label, text: seg.text, isPartial: true, tsMs }];
      }
    } else {
      const key = this.partialKeyBySpeaker.get(speakerKey);
      if (key !== undefined) {
        // 固化该说话人的未完成行
        this.partialKeyBySpeaker.delete(speakerKey);
        this.lines = this.lines.map((l) =>
          l.key === key ? { ...l, speakerLabel: seg.speaker_label, text: seg.text, isPartial: false, tsMs, voiceId, speakerRole } : l,
        );
      } else {
        // 直接新增一行
        const k = this.nextKey++;
        this.lines = [...this.lines, { key: k, speakerLabel: seg.speaker_label, text: seg.text, isPartial: false, tsMs, voiceId, speakerRole }];
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

// ── 中文智能句读（流式 ASR 无标点，按语言线索补 。，？，再分句）──

/** 疑问尾词 → 句末加 ？ */
const QUESTION_TAILS = ["吗", "呢", "吧"];
/** 句末语助词（后接新主语/连词时前一句以句号收尾）。 */
const STRONG_TAILS = ["啊", "呀", "哦", "了", "的", "吧", "么"];
/** 连词/转折/因果/递进：出现时在前句末断句（逗号或句号）。 */
const CONJUNCTIONS = ["但是", "不过", "可是", "然而", "所以", "因此", "因为", "然后", "接着", "另外", "而且", "并且", "同时", "还有", "就是", "反正", "毕竟", "虽然", "既然", "如果", "只要"];
/** 主语/时间/称呼：句长足够时在此处开启新句。 */
const SUBJECT_STARTS = ["我们", "你们", "他们", "客户", "老板", "经理", "张总", "李总", "王总", "刘总", "陈总", "今天", "明天", "后天", "下周", "本周", "上周", "月底", "这个", "那个", "这边", "那边"];
/** 无任何线索时的最大单句长度（超过则在最近语助处断，否则逗号）。 */
const NO_CLUE_LIMIT = 22;

/** 是否以语助词结尾（用于决定断句用句号还是逗号）。 */
function endsWithTail(s: string): boolean {
  return STRONG_TAILS.some((t) => s.endsWith(t));
}

/**
 * 给无标点（或极少标点）的转写文本插入中文标点：？/。，。
 * - 句尾 吗/呢/吧 → 问号
 * - 连词（然后/但是/所以…）出现 → 前句断（句号或逗号）
 * - 主语/时间/称呼词（我们/客户/今天…）在句长足够时 → 新句
 * - 无线索超长 → 最近语助处句号，否则逗号
 * 已存在的标点保留。英文按空格自然分隔不受影响。
 */
export function smartPunctuate(text: string): string {
  const raw = String(text ?? "");
  if (!raw.trim()) return raw;
  // 已有句末标点 → 直接返回（避免重复处理）
  if (SENTENCE_END.test(raw.trim().slice(-1))) return raw;

  let out = "";
  let pending = "";

  const flush = (punct: "" | "。" | "？" | "，") => {
    const t = pending.trim();
    if (t) {
      out += t;
      if (punct) out += punct;
    }
    pending = "";
  };

  let i = 0;
  while (i < raw.length) {
    const ch = raw[i];
    // 已存在的标点：直接收尾当前句
    if (SENTENCE_END.test(ch) || WEAK_BOUNDARY.test(ch)) {
      flush(ch === "。" || ch === "！" || ch === "？" || ch === "…" || ch === "；" || ch === "\n" ? "" : "");
      out += ch;
      i++;
      continue;
    }
    // 连词：前句断句
    const conj = CONJUNCTIONS.find((c) => raw.startsWith(c, i));
    if (conj) {
      if (pending.trim()) {
        out += pending.trim();
        out += endsWithTail(pending) ? "。" : "，";
        pending = "";
      }
      out += conj;
      i += conj.length;
      continue;
    }
    // 主语/时间/称呼：句长足够时新起句
    const subj = SUBJECT_STARTS.find((c) => raw.startsWith(c, i));
    if (subj && Array.from(pending).length >= 10) {
      const t = pending.trim();
      if (t) {
        out += t;
        out += endsWithTail(t) ? "。" : "，";
        pending = "";
      }
      out += subj;
      i += subj.length;
      continue;
    }
    pending += ch;
    i++;
  }

  // 收尾
  const tail = pending.trim();
  if (tail) {
    // 疑问尾 → ？；语助尾且较长 → 。；其余超长 → ，
    if (QUESTION_TAILS.some((t) => tail.endsWith(t))) {
      out += tail + "？";
    } else if (Array.from(tail).length >= NO_CLUE_LIMIT) {
      out += tail + (endsWithTail(tail) ? "。" : "，");
    } else {
      out += tail;
    }
  }
  return out;
}

/** 展示用：智能句读 + 分句（实时转写/历史详情统一入口）。 */
export function punctuateAndSplit(text: string): string[] {
  return splitSentences(smartPunctuate(text));
}
