// 实时翻译分区：中英互译结果列表。

export interface TranslationItem {
  resultId: string;
  direction: "zh_en" | "en_zh";
  content: string;
}

export default function TranslationSection({ items }: { items: TranslationItem[] }) {
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
      {items.length === 0 && <div style={{ color: "#64748b" }}>实时翻译将显示在这里…</div>}
      {items.map((t) => (
        <div key={t.resultId} style={{ marginBottom: 6, wordBreak: "break-word" }}>
          <span
            style={{
              display: "inline-block",
              fontSize: 10,
              fontWeight: 600,
              color: "#a78bfa",
              background: "rgba(167,139,250,0.12)",
              padding: "1px 6px",
              borderRadius: 3,
              marginRight: 6,
              fontFamily: "monospace",
            }}
          >
            {t.direction === "en_zh" ? "EN→中" : "中→EN"}
          </span>
          <span style={{ color: "#e2e8f0" }}>{t.content}</span>
        </div>
      ))}
    </div>
  );
}
