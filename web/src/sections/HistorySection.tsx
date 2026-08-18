// 历史面板：会话列表 + 全文搜索 + 详情查看 + 纪要生成。

import { useState } from "react";
import type { NotesTemplate, SegmentHit, SessionDetail, SessionRecord } from "../lib/api";

function formatTime(sec: number): string {
  const d = new Date(sec * 1000);
  return d.toLocaleString("zh-CN", { hour12: false });
}

export default function HistorySection({
  sessions,
  searchResults,
  detail,
  templates,
  onSearch,
  onSelect,
  onRefresh,
  onGenerateNotes,
  notesBusy,
}: {
  sessions: SessionRecord[];
  searchResults: SegmentHit[] | null;
  detail: SessionDetail | null;
  templates: NotesTemplate[];
  onSearch: (q: string) => void;
  onSelect: (id: number) => void;
  onRefresh: () => void;
  onGenerateNotes: (templateId: string) => void;
  notesBusy: boolean;
}) {
  const [query, setQuery] = useState("");
  const [templateId, setTemplateId] = useState(templates[0]?.id ?? "standard_meeting");

  return (
    <div
      style={{
        border: "1px solid rgba(255,255,255,0.08)",
        borderRadius: 8,
        padding: 8,
        maxHeight: 420,
        overflowY: "auto",
        fontSize: 12,
      }}
    >
      <div style={{ display: "flex", gap: 6, marginBottom: 8 }}>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && onSearch(query)}
          placeholder="搜索转写内容…"
          style={{
            flex: 1,
            padding: "4px 8px",
            fontSize: 12,
            borderRadius: 4,
            border: "1px solid #334155",
            background: "#0f172a",
            color: "#e2e8f0",
          }}
        />
        <button onClick={() => onSearch(query)} style={{ fontSize: 12 }}>
          搜索
        </button>
        <button onClick={onRefresh} style={{ fontSize: 12 }}>
          刷新
        </button>
      </div>

      {detail ? (
        <div>
          <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 6 }}>
            <b>会话 #{detail.id}</b>
            <button onClick={() => onSelect(-1)} style={{ fontSize: 11 }}>
              ← 返回
            </button>
          </div>
          <div style={{ color: "#64748b" }}>
            {formatTime(detail.started_at)}（{detail.segments.length} 段）
          </div>

          {/* 纪要生成 */}
          <div style={{ display: "flex", gap: 6, alignItems: "center", margin: "8px 0" }}>
            <select
              value={templateId}
              onChange={(e) => setTemplateId(e.target.value)}
              style={{
                fontSize: 12,
                padding: "3px 6px",
                borderRadius: 4,
                background: "#0f172a",
                color: "#e2e8f0",
                border: "1px solid #334155",
              }}
            >
              {templates.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
            <button onClick={() => onGenerateNotes(templateId)} disabled={notesBusy} style={{ fontSize: 12 }}>
              {notesBusy ? "生成中…" : "生成纪要"}
            </button>
          </div>
          {detail.notes && (
            <pre
              style={{
                whiteSpace: "pre-wrap",
                background: "rgba(255,255,255,0.04)",
                borderRadius: 6,
                padding: 8,
                fontSize: 11,
                margin: "4px 0",
                color: "#e2e8f0",
              }}
            >
              {detail.notes}
            </pre>
          )}

          <div style={{ marginTop: 6 }}>
            {detail.segments.map((s, i) => (
              <div key={i} style={{ marginBottom: 4, wordBreak: "break-word" }}>
                <b style={{ color: s.speaker_id === 1 ? "#2dd4bf" : "#818cf8" }}>[{s.speaker_label}]</b> {s.text}
              </div>
            ))}
          </div>
          {detail.terms.length > 0 && (
            <div style={{ marginTop: 6, color: "#94a3b8" }}>
              <b>术语：</b>
              {detail.terms.join("；")}
            </div>
          )}
        </div>
      ) : searchResults ? (
        <div>
          {searchResults.length === 0 && <div style={{ color: "#64748b" }}>无匹配</div>}
          {searchResults.map((h, i) => (
            <div
              key={i}
              style={{ marginBottom: 4, wordBreak: "break-word", cursor: "pointer" }}
              onClick={() => onSelect(h.session_id)}
            >
              <span style={{ color: "#64748b" }}>#{h.session_id}</span>{" "}
              <b style={{ color: h.speaker_label === "客户" ? "#2dd4bf" : "#818cf8" }}>{h.speaker_label}</b> {h.text}
            </div>
          ))}
        </div>
      ) : (
        <div>
          {sessions.length === 0 && <div style={{ color: "#64748b" }}>暂无历史会话</div>}
          {sessions.map((s) => (
            <div
              key={s.id}
              style={{
                marginBottom: 6,
                padding: "4px 6px",
                borderRadius: 4,
                cursor: "pointer",
                background: "rgba(255,255,255,0.03)",
              }}
              onClick={() => onSelect(s.id)}
            >
              <div>
                #{s.id} · {formatTime(s.started_at)}
              </div>
              <div style={{ color: "#64748b" }}>
                {s.segment_count} 段 · {s.term_count} 术语
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
