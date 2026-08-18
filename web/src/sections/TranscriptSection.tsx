// 实时转写分区：纯展示组件（行聚合在 App 事件处理器中完成）。

export interface TranscriptLine {
  key: number;
  speakerLabel: string;
  text: string;
  isPartial: boolean;
}

const SPEAKER_STYLE: Record<string, { color: string; bg: string }> = {
  我: { color: "#818cf8", bg: "rgba(99,102,241,0.14)" },
  客户: { color: "#2dd4bf", bg: "rgba(45,212,191,0.12)" },
};

export default function TranscriptSection({ lines }: { lines: TranscriptLine[] }) {
  return (
    <div
      style={{
        border: "1px solid rgba(255,255,255,0.08)",
        borderRadius: 8,
        background: "rgba(255,255,255,0.02)",
        padding: 8,
        maxHeight: 260,
        overflowY: "auto",
        fontSize: 12,
        lineHeight: 1.6,
      }}
    >
      {lines.length === 0 && (
        <div style={{ color: "#64748b" }}>开始监听后，转写将实时显示在这里…</div>
      )}
      {lines.map((l) => {
        const st = SPEAKER_STYLE[l.speakerLabel] ?? {
          color: "#94a3b8",
          bg: "rgba(148,163,184,0.1)",
        };
        return (
          <div key={l.key} style={{ marginBottom: 4, wordBreak: "break-word" }}>
            <span
              style={{
                display: "inline-block",
                fontSize: 10,
                fontWeight: 600,
                color: st.color,
                background: st.bg,
                padding: "1px 6px",
                borderRadius: 3,
                marginRight: 6,
                fontFamily: "monospace",
              }}
            >
              {l.speakerLabel}
            </span>
            <span style={{ color: l.isPartial ? "#cbd5e1" : "#e2e8f0" }}>
              {l.text}
              {l.isPartial && <span style={{ color: "#64748b" }}> ▍</span>}
            </span>
          </div>
        );
      })}
    </div>
  );
}
