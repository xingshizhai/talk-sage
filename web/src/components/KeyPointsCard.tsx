// 要点聚合卡片：分类徽章 + 文本。

import type { KeyPoint } from "../lib/highlights";

const KIND_COLOR: Record<string, { fg: string; bg: string }> = {
  问句: { fg: "var(--client)", bg: "var(--client-soft)" },
  要求: { fg: "var(--me)", bg: "var(--me-soft)" },
  决策: { fg: "var(--live)", bg: "var(--live-soft)" },
  行动: { fg: "var(--danger)", bg: "var(--danger-soft)" },
  技术: { fg: "var(--term)", bg: "var(--term-soft)" },
  其他: { fg: "var(--muted)", bg: "var(--surface-2)" },
};

export default function KeyPointsCard({ points }: { points: readonly KeyPoint[] }) {
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
        <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{points.length}</span>
      </div>
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "var(--pad)", display: "flex", flexDirection: "column", gap: 9 }}>
        {points.length === 0 && (
          <div style={{ color: "var(--muted)", fontSize: 13 }}>会中要点由插件抽取；关闭插件或听写场景下这里为空…</div>
        )}
        {points.map((p, i) => {
          const c = KIND_COLOR[p.kind] ?? KIND_COLOR["其他"];
          return (
            <div key={p.resultId || i} style={{ display: "flex", gap: 9, alignItems: "flex-start" }}>
              <span style={{ flexShrink: 0, fontSize: 10, fontWeight: 700, padding: "2px 7px", borderRadius: 5, background: c.bg, color: c.fg }}>
                {p.kind}
              </span>
              <span style={{ fontSize: 13, lineHeight: 1.6, color: "var(--text)" }}>{p.text}</span>
            </div>
          );
        })}
      </div>
    </section>
  );
}
