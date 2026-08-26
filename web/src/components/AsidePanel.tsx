// 右栏：术语卡片（可展开）+ 简报（知识库命中）。支持整体折叠（偏好持久化）。

import type { BriefItem } from "../sections/BriefSection";
import type { TermItem } from "../sections/TermsSection";

export default function AsidePanel({
  collapsed,
  onToggleCollapsed,
  terms,
  briefs,
  expandedTerms,
  onToggleTerm,
}: {
  collapsed: boolean;
  onToggleCollapsed: () => void;
  terms: TermItem[];
  briefs: BriefItem[];
  expandedTerms: Record<string, boolean>;
  onToggleTerm: (resultId: string) => void;
}) {
  // 折叠态：仅保留一条窄竖栏，点击展开
  if (collapsed) {
    return (
      <aside
        style={{
          width: 32,
          borderLeft: "1px solid var(--border)",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          paddingTop: 8,
          flexShrink: 0,
        }}
      >
        <button
          title="展开右栏"
          onClick={onToggleCollapsed}
          style={{
            border: "1px solid var(--border)",
            background: "var(--surface-2)",
            color: "var(--text-2)",
            borderRadius: 7,
            cursor: "pointer",
            font: "inherit",
            fontSize: 13,
            lineHeight: 1,
            padding: "6px 2px",
            writingMode: "vertical-rl",
          }}
        >
          « 术语 / 简报
        </button>
      </aside>
    );
  }

  return (
    <aside
      style={{
        width: 330,
        flexShrink: 0,
        borderLeft: "1px solid var(--border)",
        display: "flex",
        flexDirection: "column",
        gap: 12,
        padding: 12,
        boxSizing: "border-box",
      }}
    >
      {/* 收起按钮 */}
      <div style={{ display: "flex", justifyContent: "flex-end" }}>
        <button
          title="收起右栏"
          onClick={onToggleCollapsed}
          style={{
            border: "1px solid var(--border)",
            background: "var(--surface-2)",
            color: "var(--text-2)",
            borderRadius: 7,
            cursor: "pointer",
            font: "inherit",
            fontSize: 12,
            padding: "3px 9px",
          }}
        >
          » 收起
        </button>
      </div>

      {/* 术语 */}
      <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "11px var(--pad)", borderBottom: "1px solid var(--border)" }}>
          <span style={{ width: 6, height: 6, borderRadius: 2, background: "var(--term)" }} />
          <b style={{ fontSize: 13 }}>术语</b>
          <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{terms.length}</span>
        </div>
        <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: 12, display: "flex", flexDirection: "column", gap: 8 }}>
          {terms.length === 0 && <div style={{ color: "var(--muted)", fontSize: 13 }}>识别到英文缩写后将在此解释…</div>}
          {terms.map((t) => {
            const expanded = !!expandedTerms[t.resultId];
            // 拆分术语与解释（"NPI = 中文全称（英文全称）..."）
            const eq = t.content.indexOf(" = ");
            const term = eq > 0 ? t.content.slice(0, eq).trim() : t.content;
            const gloss = eq > 0 ? t.content.slice(eq + 3).trim() : "";
            return (
              <div
                key={t.resultId}
                onClick={() => onToggleTerm(t.resultId)}
                style={{ padding: "9px 11px", borderRadius: 10, cursor: "pointer", background: "var(--surface-2)", border: "1px solid var(--border)" }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: 8, minWidth: 0 }}>
                  <b style={{ fontSize: 13, fontFamily: "monospace", color: "var(--term)", whiteSpace: "nowrap" }}>{term}</b>
                  <span style={{ fontSize: 11, color: "var(--text-2)", minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {gloss}
                  </span>
                  <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)" }}>{expanded ? "▾" : "▸"}</span>
                </div>
                {expanded && (
                  <div style={{ marginTop: 8, paddingTop: 8, borderTop: "1px solid var(--border)", fontSize: 13, lineHeight: 1.6, color: "var(--text)", wordBreak: "break-word" }}>
                    {t.content}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </section>

      {/* 简报 */}
      <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "11px var(--pad)", borderBottom: "1px solid var(--border)" }}>
          <span style={{ width: 6, height: 6, borderRadius: 2, background: "var(--brief)" }} />
          <b style={{ fontSize: 13 }}>知识库命中</b>
        </div>
        <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: 12, display: "flex", flexDirection: "column", gap: 9 }}>
          {briefs.length === 0 && <div style={{ color: "var(--muted)", fontSize: 13 }}>发言命中知识库后显示…</div>}
          {briefs.map((b, i) => (
            <div key={i} style={{ fontSize: 12, lineHeight: 1.6, color: "var(--text-2)", wordBreak: "break-word", padding: "8px 10px", borderRadius: 8, background: "var(--surface-2)" }}>
              {b.text}
            </div>
          ))}
        </div>
      </section>
    </aside>
  );
}
