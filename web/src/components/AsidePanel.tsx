// 右栏：术语卡片（可展开）+ 知识库材料包（钉住笔记，可选自动命中）。支持整体折叠（偏好持久化）。

import { useEffect, useRef, useState } from "react";
import type { BriefItem } from "../sections/BriefSection";
import type { TermItem } from "../sections/TermsSection";
import {
  knowledgeEmptyHint,
  pinnedDocuments,
  truncateNoteText,
  type KnowledgeDoc,
} from "../lib/knowledge";
import { toTermRows } from "../lib/terms";

export default function AsidePanel({
  collapsed,
  onToggleCollapsed,
  terms,
  briefs,
  knowledgeEnabled,
  knowledgeFolder,
  knowledgeDocs,
  pinnedPaths,
  onTogglePin,
  expandedTerms,
  onToggleTerm,
  dismissedTermKeys,
  onDeleteTerm,
  onAskTerm,
}: {
  collapsed: boolean;
  onToggleCollapsed: () => void;
  terms: TermItem[];
  briefs: BriefItem[];
  knowledgeEnabled: boolean;
  knowledgeFolder: string;
  knowledgeDocs: KnowledgeDoc[];
  pinnedPaths: string[];
  onTogglePin: (path: string) => void;
  expandedTerms: Record<string, boolean>;
  onToggleTerm: (resultId: string) => void;
  dismissedTermKeys: ReadonlySet<string>;
  onDeleteTerm: (term: string) => void;
  /** 手动查词：交给 App 调后端，结果通过 Term 事件回到列表。 */
  onAskTerm: (term: string) => Promise<void>;
}) {
  const pinnedNotes = pinnedDocuments(knowledgeDocs, pinnedPaths);
  const emptyHint = knowledgeEmptyHint({
    enabled: knowledgeEnabled,
    folder: knowledgeFolder,
    docCount: knowledgeDocs.length,
    pinnedCount: pinnedNotes.length,
  });
  const [expandedNotes, setExpandedNotes] = useState<Record<string, boolean>>({});
  const termRows = toTermRows(terms, dismissedTermKeys);
  const termListRef = useRef<HTMLDivElement>(null);
  const newestTermId = termRows.length > 0 ? termRows[termRows.length - 1].resultId : undefined;
  useEffect(() => {
    const list = termListRef.current;
    if (list && newestTermId) {
      list.scrollTo({ top: list.scrollHeight, behavior: "smooth" });
    }
  }, [newestTermId]);
  // 手动查词：结果由后端以 Term 事件回来（和自动提取同一条路），
  // 所以这里只管输入与"查询中"状态，不自己往列表里塞卡片。
  const [asking, setAsking] = useState(false);
  const [askDraft, setAskDraft] = useState("");
  const [askBusy, setAskBusy] = useState(false);
  const [askError, setAskError] = useState("");

  async function submitAsk() {
    const term = askDraft.trim();
    if (!term || askBusy) return;
    setAskBusy(true);
    setAskError("");
    try {
      await onAskTerm(term);
      setAsking(false);
      setAskDraft("");
    } catch (e) {
      setAskError(`${e}`.replace(/^Error: /, ""));
    } finally {
      setAskBusy(false);
    }
  }

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
          « 专业术语 / 知识库
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
          <b style={{ fontSize: 13 }}>专业术语</b>
          <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{termRows.length}</span>
        </div>
        <div ref={termListRef} style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: 12, display: "flex", flexDirection: "column", gap: 8 }}>
          {termRows.length === 0 && (
            <div style={{ color: "var(--muted)", fontSize: 13 }}>
              出现行业术语或缩写时会在这里解释；常识词不收录
            </div>
          )}
          {termRows.map((t) => {
            const expanded = !!expandedTerms[t.resultId];
            const { term, gloss } = t;
            return (
              <div
                key={t.resultId}
                onClick={() => onToggleTerm(t.resultId)}
                style={{ padding: "9px 11px", borderRadius: 10, cursor: "pointer", background: "var(--surface-2)", border: "1px solid var(--border)" }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: 8, minWidth: 0 }}>
                  <b style={{ fontSize: 13, fontFamily: "monospace", color: "var(--term)", whiteSpace: "nowrap" }}>{term}</b>
                  {/* 展开后解释在下面完整显示，这里再截一遍就是同一句话读两遍 */}
                  {!expanded && (
                    <span style={{ fontSize: 11, color: "var(--text-2)", minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {gloss}
                    </span>
                  )}
                  <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)" }}>{expanded ? "▾" : "▸"}</span>
                  {t.isFinal && (
                    <button
                      type="button"
                      title={`在本次会话中删除并屏蔽“${term}”`}
                      aria-label={`删除术语 ${term}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        onDeleteTerm(term);
                      }}
                      style={{ border: "none", background: "transparent", color: "var(--muted)", cursor: "pointer", padding: "1px 3px", lineHeight: 1 }}
                    >
                      ×
                    </button>
                  )}
                </div>
                {expanded && (
                  <div style={{ marginTop: 8, paddingTop: 8, borderTop: "1px solid var(--border)", fontSize: 13, lineHeight: 1.6, color: "var(--text)", wordBreak: "break-word" }}>
                    {gloss || t.raw}
                  </div>
                )}
              </div>
            );
          })}

          {/* 空条目：点击后手动问一个词（会议里听到不懂的，直接查） */}
          {asking ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void submitAsk();
              }}
              style={{ display: "flex", gap: 6, alignItems: "center" }}
            >
              <input
                autoFocus
                value={askDraft}
                maxLength={40}
                disabled={askBusy}
                onChange={(e) => setAskDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setAsking(false);
                    setAskError("");
                  }
                }}
                placeholder="输入要查的词，回车提交"
                style={{
                  flex: 1,
                  minWidth: 0,
                  padding: "7px 9px",
                  fontSize: 13,
                  borderRadius: 8,
                  border: "1px solid var(--term)",
                  background: "var(--surface-2)",
                  color: "var(--text)",
                }}
              />
              <button type="submit" disabled={askBusy || !askDraft.trim()} style={{ fontSize: 11, flexShrink: 0 }}>
                {askBusy ? "查询中…" : "查询"}
              </button>
            </form>
          ) : (
            <div
              onClick={() => {
                setAsking(true);
                setAskDraft("");
                setAskError("");
              }}
              title="手动查询一个术语"
              style={{
                padding: "9px 11px",
                borderRadius: 10,
                cursor: "text",
                background: "transparent",
                border: "1px dashed var(--border)",
                color: "var(--muted)",
                fontSize: 12,
              }}
            >
              ＋ 点这里手动查一个词…
            </div>
          )}
          {askError && <div style={{ fontSize: 11, color: "var(--danger)", wordBreak: "break-word" }}>{askError}</div>}
        </div>
      </section>

      {/* 知识库：材料包优先，自动命中卡片仅在 brief_retriever 打开时出现 */}
      <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "11px var(--pad)", borderBottom: "1px solid var(--border)" }}>
          <span style={{ width: 6, height: 6, borderRadius: 2, background: "var(--brief)" }} />
          <b style={{ fontSize: 13 }}>知识库</b>
          <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", fontFamily: "monospace" }}>{pinnedNotes.length}</span>
        </div>
        <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: 12, display: "flex", flexDirection: "column", gap: 9 }}>
          {emptyHint && <div style={{ color: "var(--muted)", fontSize: 13 }}>{emptyHint}</div>}
          {pinnedNotes.map((doc) => {
            const expanded = !!expandedNotes[doc.path];
            const body = expanded ? doc.text.trim() : truncateNoteText(doc.text);
            return (
              <div
                key={doc.path}
                onClick={() => setExpandedNotes((prev) => ({ ...prev, [doc.path]: !prev[doc.path] }))}
                style={{ fontSize: 12, lineHeight: 1.6, color: "var(--text-2)", wordBreak: "break-word", padding: "8px 10px", borderRadius: 8, background: "var(--surface-2)", cursor: "pointer" }}
              >
                <div style={{ display: "flex", alignItems: "baseline", gap: 6, marginBottom: 4 }}>
                  <b style={{ fontSize: 12, color: "var(--text)" }}>{doc.title || doc.path}</b>
                  <span style={{ marginLeft: "auto", fontSize: 10, color: "var(--muted)", flexShrink: 0 }}>{expanded ? "▾" : "▸"}</span>
                </div>
                <div style={{ fontSize: 11, color: "var(--muted)", marginBottom: 4, fontFamily: "monospace" }}>{doc.path}</div>
                {body}
              </div>
            );
          })}
          {knowledgeEnabled && knowledgeDocs.length > 0 && (
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              <div style={{ fontSize: 11, color: "var(--muted)" }}>钉住笔记</div>
              {knowledgeDocs.map((doc) => {
                const pinned = pinnedPaths.includes(doc.path);
                return (
                  <label
                    key={doc.path}
                    style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, color: "var(--text)", cursor: "pointer" }}
                  >
                    <input
                      type="checkbox"
                      checked={pinned}
                      onChange={() => onTogglePin(doc.path)}
                    />
                    <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {doc.title || doc.path}
                    </span>
                  </label>
                );
              })}
            </div>
          )}
          {briefs.length > 0 && (
            <>
              <div style={{ fontSize: 11, color: "var(--muted)", marginTop: 4 }}>自动命中</div>
              {briefs.map((b, i) => (
                <div key={i} style={{ fontSize: 12, lineHeight: 1.6, color: "var(--text-2)", wordBreak: "break-word", padding: "8px 10px", borderRadius: 8, background: "var(--surface-2)" }}>
                  {b.text}
                </div>
              ))}
            </>
          )}
        </div>
      </section>
    </aside>
  );
}
