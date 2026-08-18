// 简报分区：知识库命中片段。

export interface BriefItem {
  source: string;
  text: string;
}

export default function BriefSection({ items }: { items: BriefItem[] }) {
  return (
    <div
      style={{
        border: "1px solid rgba(255,255,255,0.08)",
        borderRadius: 8,
        background: "rgba(255,255,255,0.02)",
        padding: 8,
        maxHeight: 180,
        overflowY: "auto",
        fontSize: 12,
        lineHeight: 1.6,
      }}
    >
      {items.length === 0 && <div style={{ color: "#64748b" }}>客户发言命中知识库后，简报显示在这里…</div>}
      {items.map((b, i) => (
        <div key={i} style={{ marginBottom: 6, wordBreak: "break-word" }}>
          <span
            style={{
              display: "inline-block",
              fontSize: 10,
              fontWeight: 600,
              color: "#fbbf24",
              background: "rgba(251,191,36,0.12)",
              padding: "1px 6px",
              borderRadius: 3,
              marginRight: 6,
              fontFamily: "monospace",
            }}
          >
            简报
          </span>
          <span style={{ color: "#cbd5e1" }}>{b.text}</span>
        </div>
      ))}
    </div>
  );
}
