// 要点聚合卡片：分类徽章 + 文本 + 手动整理时间戳记录。

import { useState } from "react";
import type { KeyPoint } from "../lib/highlights";

const KIND_COLOR: Record<string, { fg: string; bg: string }> = {
  问句: { fg: "var(--client)", bg: "var(--client-soft)" },
  要求: { fg: "var(--me)", bg: "var(--me-soft)" },
  决策: { fg: "var(--live)", bg: "var(--live-soft)" },
  行动: { fg: "var(--danger)", bg: "var(--danger-soft)" },
  技术: { fg: "var(--term)", bg: "var(--term-soft)" },
  其他: { fg: "var(--muted)", bg: "var(--surface-2)" },
};

type FlushRecord = { time: string; pointsBefore: number; msg: string; done?: boolean };

export default function KeyPointsCard({
  points,
  flushRecords = [],
  pluginLabel,
  listening,
  onFlush,
}: {
  points: readonly KeyPoint[];
  flushRecords?: readonly FlushRecord[];
  pluginLabel?: string;
  listening?: boolean;
  onFlush?: () => Promise<void>;
}) {
  const [flushing, setFlushing] = useState(false);

  const handleFlush = async () => {
    if (!onFlush || flushing) return;
    setFlushing(true);
    try { await onFlush(); } finally {
      setTimeout(() => setFlushing(false), 1500);
    }
  };

  // 把 points 和 flushRecords 交织：flush 记录插在它触发时已有的要点数量之后
  type Row =
    | { kind: "point"; point: KeyPoint; idx: number }
    | { kind: "flush"; record: FlushRecord; key: string };

  const rows: Row[] = [];
  let flushIdx = 0;
  for (let i = 0; i <= points.length; i++) {
    // 插入所有 pointsBefore === i 的 flush 记录
    while (flushIdx < flushRecords.length && flushRecords[flushIdx].pointsBefore === i) {
      rows.push({ kind: "flush", record: flushRecords[flushIdx], key: `flush-${flushIdx}` });
      flushIdx++;
    }
    if (i < points.length) {
      rows.push({ kind: "point", point: points[i], idx: i });
    }
  }
  // 剩余的 flush 记录追加到末尾
  while (flushIdx < flushRecords.length) {
    rows.push({ kind: "flush", record: flushRecords[flushIdx], key: `flush-${flushIdx}` });
    flushIdx++;
  }

  return (
    <section
      style={{
        background: "var(--card-bg)",
        border: "var(--card-border)",
        borderRadius: "var(--card-radius)",
        boxShadow: "var(--card-shadow)",
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
        flex: 1,
        minHeight: 0,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "11px var(--pad)", borderBottom: "1px solid var(--border)" }}>
        <span style={{ width: 6, height: 6, borderRadius: 2, background: "var(--live)" }} />
        <b style={{ fontSize: 13 }}>要点聚合</b>
        {pluginLabel && (
          <span style={{ fontSize: 10, color: "var(--muted)", padding: "1px 6px", borderRadius: 4, background: "var(--surface-2)", border: "1px solid var(--border)" }}>
            {pluginLabel}
          </span>
        )}
        <span style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 8 }}>
          {listening && onFlush && (
            <button
              onClick={handleFlush}
              disabled={flushing}
              title="立即处理当前积累的转写段，提前生成要点"
              style={{
                fontSize: 10,
                padding: "2px 8px",
                borderRadius: 4,
                border: "1px solid var(--border)",
                background: flushing ? "var(--surface-2)" : "var(--card-bg)",
                color: flushing ? "var(--muted)" : "var(--live)",
                cursor: flushing ? "default" : "pointer",
                fontWeight: 600,
                transition: "all 0.15s",
              }}
            >
              {flushing ? "整理中…" : "⚡ 立即整理"}
            </button>
          )}
          <span style={{ fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{points.length}</span>
        </span>
      </div>
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "var(--pad)", display: "flex", flexDirection: "column", gap: 9 }}>
        {rows.length === 0 && (
          <div style={{ color: "var(--muted)", fontSize: 13 }}>会中要点由插件抽取；关闭插件或听写场景下这里为空…</div>
        )}
        {rows.map((row) => {
          if (row.kind === "flush") {
            // 结论在后端跑完才回来；没到之前显示"整理中…"，别让用户以为点了没反应
            const { msg, done } = row.record;
            const added = /^新增/.test(msg);
            const failed = done === true && !added && !/^未发现|^没有新要点|^这 \d+ 段|^最近没有/.test(msg);
            const color = !done ? "var(--muted)" : added ? "var(--live)" : failed ? "var(--danger)" : "var(--brief)";
            return (
              <div
                key={row.key}
                style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--muted)", fontSize: 10, userSelect: "none" }}
              >
                <span style={{ flex: 1, height: 1, background: "var(--border)" }} />
                <span title={msg} style={{ color, whiteSpace: "nowrap" }}>
                  ⚡ {row.record.time} 整理 · {msg}
                </span>
                <span style={{ flex: 1, height: 1, background: "var(--border)" }} />
              </div>
            );
          }
          const c = KIND_COLOR[row.point.kind] ?? KIND_COLOR["其他"];
          return (
            <div key={row.point.resultId || row.idx} style={{ display: "flex", gap: 9, alignItems: "flex-start" }}>
              <span style={{ flexShrink: 0, fontSize: 10, fontWeight: 700, padding: "2px 7px", borderRadius: 5, background: c.bg, color: c.fg }}>
                {row.point.kind}
              </span>
              <span style={{ fontSize: 13, lineHeight: 1.6, color: "var(--text)", flex: 1 }}>{row.point.text}</span>
              <span
                title={row.point.manual ? "手动点击「立即整理」触发" : "自动批量聚合触发"}
                style={{ flexShrink: 0, fontSize: 9, color: "var(--muted)", alignSelf: "center", opacity: 0.6 }}
              >
                {row.point.manual ? "手动" : "自动"}
              </span>
            </div>
          );
        })}
      </div>
    </section>
  );
}
