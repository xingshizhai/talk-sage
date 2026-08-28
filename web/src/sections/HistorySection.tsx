// 历史面板：会话列表 + 全文搜索 + 详情查看（含质量/统计/录音回放）+ 纪要生成 + 删除。

import { useCallback, useEffect, useRef, useState } from "react";
import type { NotesTemplate, SegmentHit, SessionDetail, SessionRecord, TrioSummary } from "../lib/api";
import { recordingUrl } from "../lib/transport";
import { punctuateAndSplit } from "../lib/transcript";
import { categoryLabel } from "../lib/highlights";

function formatTime(sec: number): string {
  const d = new Date(sec * 1000);
  return d.toLocaleString("zh-CN", { hour12: false });
}

/** 场景模式中文标签（与 App.tsx SCENE_LABELS 对应）。 */
function sceneLabel(mode: string): string {
  const map: Record<string, string> = {
    dictation: "单人听写",
    conversation: "一对一会话",
    translation: "双语对话",
    meeting: "多人会议",
    lecture: "演讲/课堂",
    custom: "自定义",
  };
  return map[mode] ?? mode;
}

/** 说话人识别模式中文标签。 */
function speakerLabel(mode: string): string {
  const map: Record<string, string> = { off: "关闭", channel: "按通道", voiceprint: "声纹" };
  return map[mode] ?? mode;
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
  onGenerateTrio,
  onExportMarkdown,
  onExportText,
  onExportAudio,
  onGenerateHighlights,
  onRenameSession,
  onDeleteSession,
  onDeleteSessions,
  notesBusy,
  trioBusy,
}: {
  sessions: SessionRecord[];
  searchResults: SegmentHit[] | null;
  detail: SessionDetail | null;
  templates: NotesTemplate[];
  onSearch: (q: string) => void;
  onSelect: (id: number) => void;
  onRefresh: () => void;
  onGenerateNotes: (templateId: string) => void;
  onGenerateTrio: (meetingName: string, meetingDescription: string) => void;
  onExportMarkdown: (id: number) => Promise<string>;
  onExportText: (id: number) => Promise<string>;
  onExportAudio: (id: number) => Promise<string>;
  onGenerateHighlights: (id: number) => Promise<string[]>;
  /** 重命名会话；传空串 = 清除自定义名，回到 "#id · 时间"。 */
  onRenameSession: (id: number, title: string) => void;
  onDeleteSession: (id: number) => void;
  onDeleteSessions: (ids: number[]) => void;
  notesBusy: boolean;
  trioBusy: boolean;
}) {
  const [query, setQuery] = useState("");
  const [templateId, setTemplateId] = useState(templates[0]?.id ?? "standard_meeting");
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  // 列表多选（批量删除）
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [confirmBatch, setConfirmBatch] = useState(false);
  // 回放光标：{说话人, 音频秒}，用于同步高亮该说话人的当前段
  const [playCursor, setPlayCursor] = useState<{ speaker: string; time: number } | null>(null);
  // 三段式智能纪要：会议名称/说明（可选）
  const [meetingName, setMeetingName] = useState("");
  const [meetingDescription, setMeetingDescription] = useState("");
  // 导出状态消息（已保存路径 / 下载提示）
  const [exportMsg, setExportMsg] = useState("");
  const [exporting, setExporting] = useState<"markdown" | "text" | "audio" | null>(null);
  // 重命名：正在编辑的会话 id + 输入框草稿（列表与详情页共用一套状态，同时只会开一个）
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameDraft, setRenameDraft] = useState("");

  /** 导出会话内容：markdown / text / audio。桌面端返回落盘路径，浏览器模式触发下载。 */
  const runExport = useCallback(
    async (kind: "markdown" | "text" | "audio") => {
      if (!detail || exporting) return;
      setExporting(kind);
      setExportMsg("");
      try {
        if (kind === "markdown") {
          const path = await onExportMarkdown(detail.id);
          setExportMsg(path ? `已导出：${path}` : "已开始下载（浏览器模式）");
        } else if (kind === "text") {
          const path = await onExportText(detail.id);
          setExportMsg(path ? `已导出：${path}` : "已开始下载（浏览器模式）");
        } else {
          const path = await onExportAudio(detail.id);
          setExportMsg(path ? `已导出录音：${path}` : "已开始下载（浏览器模式）");
        }
      } catch (e) {
        setExportMsg(`导出失败：${e}`);
      } finally {
        setExporting(null);
      }
    },
    [detail, exporting, onExportMarkdown, onExportText, onExportAudio],
  );
  // LLM 整理后的核心要点（历史详情）
  const [aiHighlights, setAiHighlights] = useState<string[]>([]);
  const [hlBusy, setHlBusy] = useState(false);
  const detailIdRef = useRef(detail?.id);
  detailIdRef.current = detail?.id;

  useEffect(() => {
    setAiHighlights([]);
  }, [detail?.id]);

  /** 切换单条选择。 */
  function toggleSelect(id: number) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  /** 全选 / 清空（基于当前列表）。 */
  function toggleSelectAll() {
    const allSelected = sessions.length > 0 && sessions.every((s) => selected.has(s.id));
    setSelected(allSelected ? new Set() : new Set(sessions.map((s) => s.id)));
  }

  /** 批量删除（二次确认）。 */
  function confirmDeleteBatch() {
    if (confirmBatch) {
      setConfirmBatch(false);
      const ids = [...selected];
      setSelected(new Set());
      onDeleteSessions(ids);
    } else {
      setConfirmBatch(true);
      setTimeout(() => setConfirmBatch(false), 3000);
    }
  }

  /** 播放进度 → 更新光标。 */
  function onPlay(speaker: string, time: number) {
    setPlayCursor({ speaker, time });
  }

  /** 判断段是否处于播放高亮：段在录音中的位置 ≈ (ts_ms - duration_ms - started_at*1000)/1000。 */
  function segActive(speaker: string, tsMs: number, idx: number, durationMs: number | undefined): boolean {
    if (!playCursor || (playCursor.speaker !== "*" && playCursor.speaker !== speaker) || !detail) return false;
    const t = playCursor.time;
    const start = (tsMs - (durationMs ?? 0) - detail.started_at * 1000) / 1000;
    const rest = detail.segments
      .slice(idx + 1)
      .find((s) => playCursor.speaker === "*" || s.speaker_label === speaker);
    const nextStart = rest ? (rest.ts_ms - (rest.duration_ms ?? 0) - detail.started_at * 1000) / 1000 : Infinity;
    return t >= start && t < nextStart;
  }

  /** 进入重命名编辑态（草稿预填当前名字）。 */
  function startRename(id: number, current: string | null | undefined) {
    setRenamingId(id);
    setRenameDraft(current ?? "");
  }

  /** 提交重命名：名字没变就直接收起，不打扰后端。 */
  function commitRename(id: number, current: string | null | undefined) {
    const next = renameDraft.trim();
    setRenamingId(null);
    if (next !== (current ?? "").trim()) onRenameSession(id, next);
  }

  /** 删除二次确认：第一次点击进入确认态（3 秒后自动恢复），再次点击执行删除。 */
  function confirmDelete(id: number) {
    if (confirmDeleteId === id) {
      setConfirmDeleteId(null);
      onDeleteSession(id);
    } else {
      setConfirmDeleteId(id);
      setTimeout(() => setConfirmDeleteId((c) => (c === id ? null : c)), 3000);
    }
  }

  /** 就地重命名输入框：Enter 保存 / Esc 取消 / 失焦保存。列表与详情页共用。 */
  function renameInput(id: number, current: string | null | undefined) {
    return (
      <input
        autoFocus
        value={renameDraft}
        maxLength={80}
        placeholder="给这场会话起个名字（留空恢复默认）"
        onClick={(e) => e.stopPropagation()}
        onChange={(e) => setRenameDraft(e.target.value)}
        onBlur={() => commitRename(id, current)}
        onKeyDown={(e) => {
          // 阻止冒泡：历史页外层有全局快捷键（Space 暂停等）
          e.stopPropagation();
          if (e.key === "Enter") {
            e.preventDefault();
            commitRename(id, current);
          } else if (e.key === "Escape") {
            e.preventDefault();
            setRenamingId(null);
          }
        }}
        style={{
          flex: 1,
          minWidth: 0,
          padding: "2px 6px",
          fontSize: 12,
          borderRadius: 4,
          border: "1px solid var(--me)",
          background: "var(--surface-2)",
          color: "var(--text)",
        }}
      />
    );
  }

  const renameBtnStyle: React.CSSProperties = {
    fontSize: 10,
    padding: "1px 7px",
    borderRadius: 6,
    cursor: "pointer",
    border: "1px solid var(--border)",
    background: "var(--surface-2)",
    color: "var(--muted)",
    flexShrink: 0,
  };

  const deleteBtnStyle: React.CSSProperties = {
    fontSize: 10,
    padding: "1px 7px",
    borderRadius: 6,
    cursor: "pointer",
    border: "1px solid var(--border)",
    background: "var(--surface-2)",
  };

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
          <div style={{ display: "flex", gap: 8, marginBottom: 6, alignItems: "center" }}>
            {renamingId === detail.id ? (
              renameInput(detail.id, detail.title)
            ) : (
              <>
                <b style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  {detail.title || `会话 #${detail.id}`} <QualityBadge quality={detail.meta?.quality} />
                </b>
                <button
                  onClick={() => startRename(detail.id, detail.title)}
                  title={detail.title ? "重命名（留空恢复默认）" : "给这场会话命名"}
                  style={renameBtnStyle}
                >
                  重命名
                </button>
              </>
            )}
            <button onClick={() => onSelect(-1)} style={{ fontSize: 11, marginLeft: "auto", flexShrink: 0 }}>
              ← 返回
            </button>
          </div>
          <div style={{ color: "var(--muted)" }}>
            {detail.title ? `#${detail.id} · ` : ""}
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
              {/* 运行环境快照：当时用的模型/场景/主要参数，供事后对比分析 */}
              {(() => {
                const ri = detail.meta?.runtime_info;
                if (!ri) return null;
                return (
                  <div style={{ marginTop: 5, paddingTop: 5, borderTop: "1px dashed var(--border)", color: "var(--muted)" }}>
                    <div style={{ fontWeight: 700, color: "var(--text-2)", marginBottom: 2 }}>运行环境（本次转写配置）</div>
                    <div>
                      场景 {sceneLabel(ri.scene_mode)} · 引擎 {ri.user_engine}
                      {ri.client_enabled ? ` + ${ri.client_engine ?? "?"}` : "（单流）"} · v{ri.app_version}
                    </div>
                    <div>
                      VAD {ri.vad_preset}
                      {ri.vad_min_silence_ms ? `（段尾静音 ${ri.vad_min_silence_ms}ms）` : ""} · 降噪{" "}
                      {ri.denoise_enabled ? "开" : "关"} · 最短提交 {ri.min_segment_ms}ms · 增益 {ri.input_gain_db}dB
                    </div>
                    <div>说话人 {speakerLabel(ri.speaker_mode)} · {ri.sample_rate}Hz</div>
                  </div>
                );
              })()}
            </div>
          )}

          {/* 回放：完整会话为主，原始输入分轨保留用于诊断和模型评估。 */}
          {(() => {
            const recs = detail.meta?.streams.filter((s) => s.recording) ?? [];
            const master = detail.meta?.master_recording ?? (recs.length === 1 ? recs[0].recording : null);
            if (!master && recs.length === 0) return null;
            const masterUrl = recordingUrl(master);
            return (
              <div style={{ marginTop: 8, padding: "7px 9px", borderRadius: 6, background: "var(--surface-2)", border: "1px solid var(--border)" }}>
                <div style={{ fontSize: 11, fontWeight: 700, marginBottom: 6, color: "var(--text-2)" }}>回放（完整会话）</div>
                {masterUrl && (
                  <audio
                    controls
                    preload="metadata"
                    src={masterUrl}
                    style={{ width: "100%", height: 32 }}
                    onTimeUpdate={(e) => onPlay("*", (e.target as HTMLAudioElement).currentTime)}
                    onEnded={() => setPlayCursor(null)}
                  />
                )}
                {!masterUrl && <div style={{ fontSize: 10, color: "var(--muted)" }}>该旧会话没有完整主录音，请展开下方原始分轨播放。</div>}
                <div style={{ fontSize: 10, color: "var(--muted)", marginTop: 3 }}>播放时下方对应时间的转写会同步高亮；双流录音左声道为麦克风，右声道为系统音频。</div>
                <div style={{ marginTop: 6, display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
                  <button
                    onClick={() => void runExport("audio")}
                    disabled={exporting !== null}
                    style={{ fontSize: 11, padding: "2px 10px", cursor: exporting !== null ? "default" : "pointer" }}
                  >
                    {exporting === "audio" ? "导出中…" : "导出完整录音"}
                  </button>
                  <span style={{ fontSize: 10, color: "var(--muted)" }}>导出为 WAV 音频文件（完整会议）</span>
                </div>
                {recs.length > 0 && (
                  <details style={{ marginTop: 8 }}>
                    <summary style={{ cursor: "pointer", fontSize: 10, color: "var(--text-2)", fontWeight: 700 }}>原始输入分轨（{recs.length}）</summary>
                    <div style={{ marginTop: 6 }}>
                      {recs.map((s, i) => {
                        const url = recordingUrl(s.recording);
                        if (!url) return null;
                        return (
                          <div key={`${s.recording}-${i}`} style={{ marginBottom: 8 }}>
                            <div style={{ fontSize: 10, color: "var(--muted)", marginBottom: 2, fontFamily: "monospace" }}>
                              [{s.speaker_label}] {Math.round(s.total_ms / 1000)}s · {s.recording!.split(/[\\/]/).pop()}
                            </div>
                            <audio controls preload="metadata" src={url} style={{ width: "100%", height: 30 }} onTimeUpdate={(e) => onPlay(s.speaker_label, (e.target as HTMLAudioElement).currentTime)} onEnded={() => setPlayCursor(null)} />
                          </div>
                        );
                      })}
                    </div>
                  </details>
                )}
              </div>
            );
          })()}

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
            <button
              onClick={() => void runExport("markdown")}
              disabled={exporting !== null}
              style={{ fontSize: 12, marginLeft: "auto" }}
            >
              {exporting === "markdown" ? "导出中…" : "导出 Markdown"}
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

          {/* 三段式智能纪要（借鉴 Call.md summary-generator）：概述 / 归属要点 / 行动项 */}
          <div style={{ margin: "8px 0 4px", borderTop: "1px dashed var(--border)", paddingTop: 8 }}>
            <b style={{ fontSize: 12 }}>智能纪要</b>
            <span style={{ fontSize: 10, color: "var(--muted)", marginLeft: 6 }}>叙事概述 + 归属要点 + 行动项（并行生成）</span>
          </div>
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 4 }}>
            <input
              value={meetingName}
              onChange={(e) => setMeetingName(e.target.value)}
              placeholder="会议名称（可选）"
              style={{ fontSize: 11, padding: "3px 6px", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)", width: 150 }}
            />
            <input
              value={meetingDescription}
              onChange={(e) => setMeetingDescription(e.target.value)}
              placeholder="会议说明（可选，如：确认 NPI 交付时间）"
              style={{ fontSize: 11, padding: "3px 6px", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)", flex: 1, minWidth: 180 }}
            />
            <button onClick={() => onGenerateTrio(meetingName, meetingDescription)} disabled={trioBusy} style={{ fontSize: 12 }}>
              {trioBusy ? "生成中…" : "生成智能纪要"}
            </button>
          </div>
          {detail.trio && (() => {
            let trio: TrioSummary | null = null;
            try {
              trio = JSON.parse(detail.trio) as TrioSummary;
            } catch {
              trio = null;
            }
            if (!trio) return null;
            return (
              <div style={{ background: "var(--surface-2)", borderRadius: 6, padding: 8, fontSize: 11, margin: "4px 0", color: "var(--text)" }}>
                {trio.short_overview && <p style={{ margin: "0 0 6px", lineHeight: 1.6 }}>{trio.short_overview}</p>}
                {trio.key_points.length > 0 && (
                  <div style={{ marginBottom: 6 }}>
                    <b>关键要点</b>
                    {trio.key_points.map((kp, i) => (
                      <div key={i} style={{ margin: "4px 0 0 8px" }}>
                        <b style={{ color: "var(--term)" }}>▸ {kp.topic}</b>
                        <ul style={{ margin: "2px 0 0 16px", paddingLeft: 4 }}>
                          {kp.points.map((p, j) => (
                            <li key={j}>{p}</li>
                          ))}
                        </ul>
                      </div>
                    ))}
                  </div>
                )}
                {trio.action_items.length > 0 && (
                  <div>
                    <b>行动项</b>
                    <ul style={{ margin: "2px 0 0 16px", paddingLeft: 4 }}>
                      {trio.action_items.map((a, i) => (
                        <li key={i}>{a}</li>
                      ))}
                    </ul>
                  </div>
                )}
              </div>
            );
          })()}

          <div style={{ marginTop: 10, borderTop: "1px dashed var(--border)", paddingTop: 8, display: "flex", gap: 8, alignItems: "center" }}>
            <b style={{ fontSize: 12 }}>转写</b>
            <span style={{ fontSize: 10, color: "var(--muted)" }}>{detail.segments.length} 段</span>
            <button
              onClick={() => void runExport("text")}
              disabled={exporting !== null}
              style={{ fontSize: 11, padding: "2px 10px", marginLeft: "auto", cursor: exporting !== null ? "default" : "pointer" }}
            >
              {exporting === "text" ? "导出中…" : "导出文本"}
            </button>
          </div>
          <div style={{ marginTop: 6 }}>
            {detail.segments.map((s, i) => {
              const sentences = punctuateAndSplit(s.text);
              return (
                <div
                  key={i}
                  style={{
                    marginBottom: 4,
                    wordBreak: "break-word",
                    padding: "1px 4px",
                    borderRadius: 4,
                    background: segActive(s.speaker_label, s.ts_ms, i, s.duration_ms) ? "var(--brief-soft)" : undefined,
                  }}
                >
                  <b style={{ color: s.speaker_id === 1 ? "var(--client)" : "var(--me)" }}>[{s.speaker_label}]</b>{" "}
                  {sentences.map((sent, j) => (
                    <span key={j} style={{ color: "var(--text)" }}>
                      {sent}
                      {j < sentences.length - 1 && <span style={{ color: "var(--muted)" }}>｜</span>}
                    </span>
                  ))}
                </div>
              );
            })}
          </div>
          {detail.terms.length > 0 && (
            <div style={{ marginTop: 6, color: "var(--text-2)" }}>
              <b>专业术语：</b>
              {detail.terms.join("；")}
            </div>
          )}

          <div style={{ marginTop: 8 }}>
            <b style={{ fontSize: 12 }}>会中要点</b>
            {(detail.key_points ?? []).length === 0 ? (
              <div style={{ color: "var(--muted)", fontSize: 12, marginTop: 4 }}>无（插件未启用、听写场景，或旧会话未落库）</div>
            ) : (
              <div style={{ marginTop: 4, display: "flex", flexDirection: "column", gap: 4 }}>
                {(detail.key_points ?? []).map((kp) => (
                  <div key={kp.result_id} style={{ display: "flex", gap: 8, alignItems: "flex-start", fontSize: 12, lineHeight: 1.6, color: "var(--text)" }}>
                    <span style={{ flexShrink: 0, fontSize: 10, fontWeight: 700, padding: "1px 6px", borderRadius: 5, background: "var(--surface-2)", color: "var(--text-2)" }}>
                      {categoryLabel(kp.category)}
                    </span>
                    <span>{kp.content}</span>
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* 会后 LLM：整理已落库要点，不再从转写重抽 */}
          <div style={{ margin: "8px 0 4px", borderTop: "1px dashed var(--border)", paddingTop: 8, display: "flex", gap: 8, alignItems: "center" }}>
            <b style={{ fontSize: 12 }}>核心要点（AI 整理）</b>
            <button
              onClick={async () => {
                const id = detail.id;
                setHlBusy(true);
                setAiHighlights([]);
                try {
                  const points = await onGenerateHighlights(id);
                  if (detailIdRef.current === id) setAiHighlights(points);
                } catch (e) {
                  alert(`要点整理失败: ${e}`);
                } finally {
                  setHlBusy(false);
                }
              }}
              disabled={hlBusy}
              style={{ fontSize: 11 }}
            >
              {hlBusy ? "整理中…" : "AI 整理"}
            </button>
            <span style={{ fontSize: 10, color: "var(--muted)" }}>基于会中要点合并去重；旧会话无要点时才根据转写整理。需要配置 LLM</span>
          </div>
          {aiHighlights.length > 0 && (
            <div style={{ background: "var(--surface-2)", borderRadius: 6, padding: 8, fontSize: 12, margin: "4px 0", color: "var(--text)" }}>
              {aiHighlights.map((h, i) => (
                <div key={i} style={{ display: "flex", gap: 8, padding: "2px 0", lineHeight: 1.6 }}>
                  <span style={{ color: "var(--live)", fontWeight: 700, flexShrink: 0 }}>{i + 1}.</span>
                  <span>{h}</span>
                </div>
              ))}
            </div>
          )}

          <div style={{ marginTop: 10, borderTop: "1px solid var(--border)", paddingTop: 8, display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
            <button
              onClick={() => confirmDelete(detail.id)}
              style={{ ...deleteBtnStyle, color: confirmDeleteId === detail.id ? "var(--danger)" : "var(--muted)" }}
            >
              {confirmDeleteId === detail.id ? "确认删除此会话？" : "删除此会话"}
            </button>
            {exportMsg && <span style={{ fontSize: 10, color: "var(--muted)", wordBreak: "break-all" }}>{exportMsg}</span>}
          </div>
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
          {/* 多选工具条 */}
          {sessions.length > 0 && (
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                marginBottom: 8,
                padding: "6px 8px",
                borderRadius: 6,
                background: selected.size > 0 ? "var(--me-soft)" : "var(--surface-2)",
                border: "1px solid var(--border)",
                fontSize: 11,
              }}
            >
              <label style={{ display: "flex", alignItems: "center", gap: 5, cursor: "pointer" }}>
                <input
                  type="checkbox"
                  checked={sessions.length > 0 && sessions.every((s) => selected.has(s.id))}
                  onChange={toggleSelectAll}
                />
                全选
              </label>
              <span style={{ color: "var(--muted)" }}>已选 {selected.size} 条</span>
              <button
                onClick={confirmDeleteBatch}
                disabled={selected.size === 0}
                style={{
                  marginLeft: "auto",
                  fontSize: 11,
                  padding: "2px 10px",
                  borderRadius: 6,
                  cursor: selected.size > 0 ? "pointer" : "not-allowed",
                  opacity: selected.size > 0 ? 1 : 0.45,
                  border: "1px solid var(--border)",
                  background: "var(--surface-2)",
                  color: confirmBatch ? "var(--danger)" : "var(--danger)",
                }}
              >
                {confirmBatch ? `确认删除 ${selected.size} 条？` : "删除选中"}
              </button>
            </div>
          )}
          {sessions.map((s) => (
            <div
              key={s.id}
              style={{
                marginBottom: 6,
                padding: "4px 6px",
                borderRadius: 4,
                cursor: "pointer",
                background: selected.has(s.id) ? "var(--me-soft)" : "var(--surface-2)",
              }}
              onClick={() => onSelect(s.id)}
            >
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <input
                  type="checkbox"
                  checked={selected.has(s.id)}
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => {
                    e.stopPropagation();
                    toggleSelect(s.id);
                  }}
                  style={{ cursor: "pointer", flexShrink: 0 }}
                />
                {renamingId === s.id ? (
                  renameInput(s.id, s.title)
                ) : (
                  <>
                    {/* 命名过的会话以名字为主，编号和时间退到第二行 */}
                    <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {s.title ? <b>{s.title}</b> : `#${s.id} · ${formatTime(s.started_at)}`} <QualityBadge quality={s.quality} />
                    </span>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        startRename(s.id, s.title);
                      }}
                      title={s.title ? "重命名（留空恢复默认）" : "给这场会话命名"}
                      style={{ ...renameBtnStyle, marginLeft: "auto" }}
                    >
                      重命名
                    </button>
                  </>
                )}
              </div>
              <div style={{ color: "var(--muted)", marginLeft: 24 }}>
                {s.title ? `#${s.id} · ${formatTime(s.started_at)} · ` : ""}
                {s.segment_count} 段 · {s.term_count} 专业术语
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
