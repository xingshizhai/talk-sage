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
