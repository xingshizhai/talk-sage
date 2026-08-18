// 术语解释分区：骨架卡片 → 最终内容原地更新。

export interface TermItem {
  resultId: string;
  content: string;
  isFinal: boolean;
}

export default function TermsSection({ items }: { items: TermItem[] }) {
  return (
    <div
      style={{
        border: "1px solid rgba(255,255,255,0.08)",
        borderRadius: 8,
        background: "rgba(255,255,255,0.02)",
        padding: 8,
        maxHeight: 200,
        overflowY: "auto",
        fontSize: 12,
        lineHeight: 1.6,
      }}
    >
      {items.length === 0 && <div style={{ color: "#64748b" }}>识别到英文缩写后，将在此解释…</div>}
      {items.map((t) => (
        <div key={t.resultId} style={{ marginBottom: 6, wordBreak: "break-word" }}>
          <span
            style={{
              display: "inline-block",
              fontSize: 10,
              fontWeight: 600,
              color: "#2dd4bf",
              background: "rgba(45,212,191,0.12)",
              padding: "1px 6px",
              borderRadius: 3,
              marginRight: 6,
              fontFamily: "monospace",
            }}
          >
            TERM
          </span>
          <span style={{ color: t.isFinal ? "#e2e8f0" : "#94a3b8" }}>{t.content}</span>
        </div>
      ))}
    </div>
  );
}
