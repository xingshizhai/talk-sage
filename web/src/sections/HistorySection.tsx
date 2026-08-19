// 历史面板：会话列表 + 全文搜索 + 详情查看（含质量/统计）+ 纪要生成。

import { useState } from "react";
import type { NotesTemplate, SegmentHit, SessionDetail, SessionRecord } from "../lib/api";

function formatTime(sec: number): string {
  const d = new Date(sec * 1000);
  return d.toLocaleString("zh-CN", { hour12: false });
}

/** 质量徽章样式。 */
const QUALITY_STYLE: Record<string, { label: string; color: string; bg: string }> = {
  clean: { label: "正常", color: "var(--live)", bg: "var(--live-soft)" },
  noise: { label: "噪音", color: "var(--brief)", bg: "var(--brief-soft)" },
  silent: { label: "静音", color: "var(--muted)", bg: "var(--surface-2)" },
  low: { label: "待复核", color: "var(--term)", bg: "var(--term-soft)" },
};

function QualityBadge({ quality }: { quality?: string }) {
  if (!quality) return null;
  const s = QUALITY_STYLE[quality] ?? { label: quality, color: "var(--muted)", bg: "var(--surface-2)" };
  return (
    <span
      style={{
        fontSize: 10,
        fontWeight: 700,
        padding: "1px 7px",
        borderRadius: 8,
        color: s.color,
        background: s.bg,
        marginLeft: 6,
      }}
    >
      {s.label}
    </span>
  );
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
        border: "1px solid var(--border)",
        borderRadius: 8,
        padding: 8,
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
            border: "1px solid var(--border)",
            background: "var(--surface-2)",
            color: "var(--text)",
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
          <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 6, alignItems: "center" }}>
            <b>
              会话 #{detail.id} <QualityBadge quality={detail.meta?.quality} />
            </b>
            <button onClick={() => onSelect(-1)} style={{ fontSize: 11 }}>
              ← 返回
            </button>
          </div>
          <div style={{ color: "var(--muted)" }}>
            {formatTime(detail.started_at)}（{detail.segments.length} 段）
          </div>

          {/* 会话统计（质量评估信息） */}
          {detail.meta && (
            <div
              style={{
                marginTop: 6,
                padding: "7px 9px",
                borderRadius: 6,
                background: "var(--surface-2)",
                border: "1px solid var(--border)",
                fontSize: 11,
                lineHeight: 1.7,
                color: "var(--text-2)",
                fontFamily: "monospace",
              }}
            >
              <div>
                质量: <b style={{ color: detail.meta.skipped_analysis ? "var(--brief)" : "var(--live)" }}>{detail.meta.quality}</b>
                {detail.meta.skipped_analysis && (
                  <span style={{ color: "var(--brief)" }}>（已跳过要点聚合等分析）</span>
                )}
              </div>
              <div>
                时长 {Math.round(detail.meta.duration_ms / 1000)}s · 语音 {Math.round(detail.meta.speech_ratio * 100)}% · 文本噪音{" "}
                {detail.meta.text_noise.toFixed(2)}
              </div>
              <div>平均能量 {detail.meta.avg_rms.toFixed(4)} · 峰值 {detail.meta.max_rms.toFixed(4)}</div>
              {detail.meta.streams.map((s) => (
                <div key={s.speaker_label}>
                  [{s.speaker_label}] {Math.round(s.total_ms / 1000)}s / 语音 {Math.round(s.speech_ms / 1000)}s / {s.final_segments} 段
                  {s.recording ? ` / ${s.recording.split(/[\\/]/).pop()}` : ""}
                </div>
              ))}
            </div>
          )}

          {/* 纪要生成 */}
          <div style={{ display: "flex", gap: 6, alignItems: "center", margin: "8px 0" }}>
            <select
              value={templateId}
              onChange={(e) => setTemplateId(e.target.value)}
              style={{
                fontSize: 12,
                padding: "3px 6px",
                borderRadius: 4,
                background: "var(--surface-2)",
                color: "var(--text)",
                border: "1px solid var(--border)",
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
                background: "var(--surface-2)",
                borderRadius: 6,
                padding: 8,
                fontSize: 11,
                margin: "4px 0",
                color: "var(--text)",
              }}
            >
              {detail.notes}
            </pre>
          )}

          <div style={{ marginTop: 6 }}>
            {detail.segments.map((s, i) => (
              <div key={i} style={{ marginBottom: 4, wordBreak: "break-word" }}>
                <b style={{ color: s.speaker_id === 1 ? "var(--client)" : "var(--me)" }}>[{s.speaker_label}]</b> {s.text}
              </div>
            ))}
          </div>
          {detail.terms.length > 0 && (
            <div style={{ marginTop: 6, color: "var(--text-2)" }}>
              <b>术语：</b>
              {detail.terms.join("；")}
            </div>
          )}
        </div>
      ) : searchResults ? (
        <div>
          {searchResults.length === 0 && <div style={{ color: "var(--muted)" }}>无匹配</div>}
          {searchResults.map((h, i) => (
            <div
              key={i}
              style={{ marginBottom: 4, wordBreak: "break-word", cursor: "pointer" }}
              onClick={() => onSelect(h.session_id)}
            >
              <span style={{ color: "var(--muted)" }}>#{h.session_id}</span>{" "}
              <b style={{ color: h.speaker_label === "客户" ? "var(--client)" : "var(--me)" }}>{h.speaker_label}</b> {h.text}
            </div>
          ))}
        </div>
      ) : (
        <div>
          {sessions.length === 0 && <div style={{ color: "var(--muted)" }}>暂无历史会话</div>}
          {sessions.map((s) => (
            <div
              key={s.id}
              style={{
                marginBottom: 6,
                padding: "4px 6px",
                borderRadius: 4,
                cursor: "pointer",
                background: "var(--surface-2)",
              }}
              onClick={() => onSelect(s.id)}
            >
              <div>
                #{s.id} · {formatTime(s.started_at)} <QualityBadge quality={s.quality} />
              </div>
              <div style={{ color: "var(--muted)" }}>
                {s.segment_count} 段 · {s.term_count} 术语
                {s.duration_ms ? ` · ${Math.round(s.duration_ms / 1000)}s` : ""}
                {s.speech_ratio !== undefined ? ` · 语音 ${Math.round(s.speech_ratio * 100)}%` : ""}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
