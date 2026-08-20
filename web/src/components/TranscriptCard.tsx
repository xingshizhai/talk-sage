// 实时转写卡片：3 视图模式（时间线 / 专注 / 密集）+ 自动滚动 + 智能句读分句。

import { useEffect, useRef } from "react";
import { punctuateAndSplit } from "../lib/transcript";

export type TranscriptMode = "timeline" | "focus" | "dense";

export interface TimelineLine {
  key: number;
  time: string;
  speaker: string;
  speakerColor: string;
  engine: string;
  text: string;
  isPartial: boolean;
  translation?: string;
}

export default function TranscriptCard({
  mode,
  setMode,
  meta,
  lines,
}: {
  mode: TranscriptMode;
  setMode: (m: TranscriptMode) => void;
  meta: string;
  lines: TimelineLine[];
}) {
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [lines, mode]);

  const modes: { key: TranscriptMode; label: string }[] = [
    { key: "timeline", label: "时间线" },
    { key: "focus", label: "专注" },
    { key: "dense", label: "密集" },
  ];

  // 专注模式：最后一行高亮（当前句）
  const focusIndex = lines.length > 0 ? lines.length - 1 : -1;

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
      <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "11px var(--pad)", borderBottom: "1px solid var(--border)" }}>
        <b style={{ fontSize: 13 }}>实时转写</b>
        <span style={{ fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{meta}</span>
        <div style={{ flex: 1 }} />
        {modes.map((m) => (
          <button
            key={m.key}
            onClick={() => setMode(m.key)}
            style={{
              padding: "4px 10px",
              borderRadius: 7,
              border: "1px solid var(--border)",
              cursor: "pointer",
              font: "inherit",
              fontSize: 11,
              fontWeight: 600,
              background: mode === m.key ? "var(--me)" : "var(--surface-2)",
              color: mode === m.key ? "#fff" : "var(--text-2)",
            }}
          >
            {m.label}
          </button>
        ))}
      </div>

      <div ref={scrollRef} style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "var(--pad)", display: "flex", flexDirection: "column", gap: 10 }}>
        {lines.length === 0 && (
          <div style={{ color: "var(--muted)", fontSize: 13 }}>开始监听后，转写将实时显示在这里…</div>
        )}
        {mode === "dense" ? (
          lines.map((l) => {
            const sentences = punctuateAndSplit(l.text);
            return (
              <div key={l.key} style={{ fontSize: 12, color: "var(--text-2)", wordBreak: "break-word" }}>
                <span style={{ color: l.speakerColor, fontWeight: 600 }}>[{l.speaker}]</span>{" "}
                {sentences.map((s, j) => (
                  <span key={j} style={{ color: l.isPartial ? "var(--muted)" : "var(--text)", marginRight: 6 }}>
                    {s}
                    {j < sentences.length - 1 && <span style={{ color: "var(--muted)" }}>｜</span>}
                  </span>
                ))}
                {l.isPartial && <span style={{ color: "var(--muted)" }}> ▍</span>}
              </div>
            );
          })
        ) : (
          lines.map((l, i) => {
            const sentences = punctuateAndSplit(l.text);
            return (
            <div
              key={l.key}
              style={{
                display: "grid",
                gridTemplateColumns: mode === "timeline" ? "52px 1fr" : "1fr",
                gap: 10,
                opacity: mode === "focus" && i !== focusIndex ? 0.45 : 1,
                fontSize: mode === "focus" && i === focusIndex ? 14 : 13,
                transition: "opacity 0.2s, font-size 0.2s",
              }}
            >
              {mode === "timeline" && (
                <span style={{ fontSize: 10, color: "var(--muted)", fontFamily: "monospace", paddingTop: 2 }}>{l.time}</span>
              )}
              <div style={{ wordBreak: "break-word" }}>
                <span style={{ fontSize: 10, fontWeight: 700, color: l.speakerColor, marginRight: 6 }}>
                  {l.speaker}
                </span>
                <span style={{ fontSize: 10, color: "var(--muted)", fontFamily: "monospace", marginRight: 6 }}>
                  {l.engine}
                </span>
                {sentences.map((s, j) => (
                  <div key={j} style={{ color: l.isPartial ? "var(--text-2)" : "var(--text)", lineHeight: 1.7 }}>
                    {s}
                    {l.isPartial && j === sentences.length - 1 && <span style={{ color: "var(--muted)" }}> ▍</span>}
                  </div>
                ))}
                {l.translation && (
                  <div style={{ marginTop: 3, fontSize: 12, color: "var(--muted)", borderLeft: "2px solid var(--term)", paddingLeft: 8 }}>
                    {l.translation}
                  </div>
                )}
              </div>
            </div>
            );
          })
        )}
      </div>
    </section>
  );
}
