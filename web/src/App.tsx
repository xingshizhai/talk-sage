import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { getApi } from "./lib/transport";
import type { AppConfig, DomainEvent } from "./lib/api";
import { TranscriptAccumulator, type TranscriptLine } from "./lib/transcript";
import TranscriptSection from "./sections/TranscriptSection";
import TermsSection, { type TermItem } from "./sections/TermsSection";
import TranslationSection, { type TranslationItem } from "./sections/TranslationSection";
import BriefSection, { type BriefItem } from "./sections/BriefSection";
import HistorySection from "./sections/HistorySection";
import SettingsSection from "./sections/SettingsSection";
import DebugWindow from "./sections/DebugWindow";
import type { NotesTemplate, SegmentHit, SessionDetail, SessionRecord } from "./lib/api";

const api = getApi();

const PANEL_STYLE: CSSProperties = {
  border: "1px solid rgba(255,255,255,0.1)",
  borderRadius: 10,
  background: "rgba(255,255,255,0.02)",
  padding: 10,
  marginBottom: 12,
};

export default function App() {
  const [version, setVersion] = useState<string>("—");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [listening, setListening] = useState(false);
  const [status, setStatus] = useState<string>("待机");
  const [lines, setLines] = useState<TranscriptLine[]>([]);
  const [terms, setTerms] = useState<TermItem[]>([]);
  const [translations, setTranslations] = useState<TranslationItem[]>([]);
  const [briefs, setBriefs] = useState<BriefItem[]>([]);
  const [rawEvents, setRawEvents] = useState<DomainEvent[]>([]);
  const [showHistory, setShowHistory] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [showDebug, setShowDebug] = useState(false);
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [searchResults, setSearchResults] = useState<SegmentHit[] | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [templates, setTemplates] = useState<NotesTemplate[]>([]);
  const [notesBusy, setNotesBusy] = useState(false);
  const accumulatorRef = useRef(new TranscriptAccumulator());

  useEffect(() => {
    api.getVersion().then(setVersion).catch(console.error);
    api.getConfig().then(setConfig).catch(console.error);
    const off = api.onEvent((ev: DomainEvent) => {
      if (ev.type === "status") {
        setStatus(ev.message);
        if (ev.stage === "recording") setListening(true);
        if (ev.stage === "idle" || ev.stage === "asr_ready") setListening(false);
      }
      if (ev.type === "segment") {
        const acc = accumulatorRef.current;
        acc.push(ev);
        setLines([...acc.getLines()]);
      }
      if (ev.type === "term") {
        setTerms((prev) => {
          const idx = prev.findIndex((t) => t.resultId === ev.result_id);
          if (idx >= 0) {
            const next = [...prev];
            next[idx] = { resultId: ev.result_id, content: ev.content, isFinal: ev.status === "final" };
            return next;
          }
          return [...prev, { resultId: ev.result_id, content: ev.content, isFinal: ev.status === "final" }];
        });
      }
      if (ev.type === "translation") {
        setTranslations((prev) => [
          ...prev,
          { resultId: ev.result_id, direction: ev.direction, content: ev.content },
        ]);
      }
      if (ev.type === "brief") {
        setBriefs((prev) => [...prev, { source: ev.source, text: ev.text }]);
      }
      // 调试窗口：保留最近 200 条事件
      setRawEvents((prev) => [...prev.slice(-199), ev]);
    });
    return off;
  }, []);

  const refreshHistory = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
    } catch (e) {
      console.error("历史加载失败:", e);
    }
  }, []);

  const handleHistorySearch = useCallback(async (q: string) => {
    setDetail(null);
    if (!q.trim()) {
      setSearchResults(null);
      return;
    }
    try {
      setSearchResults(await api.searchSessions(q));
    } catch (e) {
      console.error("搜索失败:", e);
    }
  }, []);

  const handleHistorySelect = useCallback(
    async (id: number) => {
      if (id < 0) {
        setDetail(null);
        return;
      }
      try {
        setDetail(await api.getSession(id));
        if (templates.length === 0) {
          setTemplates(await api.listNotesTemplates());
        }
      } catch (e) {
        console.error("会话详情失败:", e);
      }
    },
    [templates.length],
  );

  const handleGenerateNotes = useCallback(
    async (templateId: string) => {
      if (!detail) return;
      setNotesBusy(true);
      try {
        const notes = await api.generateNotes(detail.id, templateId);
        setDetail({ ...detail, notes });
      } catch (e) {
        console.error("纪要生成失败:", e);
        alert(`纪要生成失败: ${e}`);
      } finally {
        setNotesBusy(false);
      }
    },
    [detail],
  );

  const handleListen = useCallback(async () => {
    try {
      if (listening) {
        await api.stopListen();
        setListening(false);
        setStatus("已停止");
      } else {
        setStatus("启动中…");
        await api.startListen();
      }
    } catch (e) {
      setStatus(`错误: ${e}`);
    }
  }, [listening]);

  const handleToggleHistory = useCallback(() => {
    setShowHistory((v) => !v);
    if (!showHistory) refreshHistory();
  }, [showHistory, refreshHistory]);

  return (
    <div style={{ height: "100vh", display: "flex", flexDirection: "column", fontFamily: "system-ui, sans-serif", overflow: "hidden" }}>
      {/* 顶栏 */}
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "10px 14px",
          borderBottom: "1px solid rgba(255,255,255,0.1)",
          background: "rgba(255,255,255,0.02)",
        }}
      >
        <b style={{ fontSize: 15 }}>TalkSage</b>
        <span style={{ fontSize: 11, color: "#64748b" }}>
          v{version} · {api.transport}
        </span>
        <span
          style={{
            fontSize: 11,
            padding: "2px 8px",
            borderRadius: 10,
            background: listening ? "rgba(52,211,153,0.15)" : "rgba(100,116,139,0.15)",
            color: listening ? "#34d399" : "#94a3b8",
          }}
        >
          {listening ? "● 监听中" : status}
        </span>
        <div style={{ flex: 1 }} />
        <button
          onClick={handleListen}
          style={{
            padding: "7px 18px",
            borderRadius: 8,
            border: "none",
            fontWeight: 600,
            cursor: "pointer",
            background: listening ? "#ef4444" : "#10b981",
            color: "#fff",
          }}
        >
          {listening ? "⏹ 停止监听" : "▶ 开始监听"}
        </button>
        <button
          onClick={handleToggleHistory}
          style={{ padding: "7px 12px", borderRadius: 8, cursor: "pointer", background: showHistory ? "#2563eb" : "#1e293b", color: "#e2e8f0", border: "none" }}
        >
          历史
        </button>
        <button
          onClick={() => setShowSettings((v) => !v)}
          style={{ padding: "7px 12px", borderRadius: 8, cursor: "pointer", background: showSettings ? "#2563eb" : "#1e293b", color: "#e2e8f0", border: "none" }}
        >
          设置
        </button>
        <button
          onClick={() => setShowDebug(true)}
          style={{ padding: "7px 12px", borderRadius: 8, cursor: "pointer", background: "#1e293b", color: "#e2e8f0", border: "none" }}
        >
          调试
        </button>
      </header>

      {/* 主体：左右双栏 */}
      <div style={{ flex: 1, display: "flex", overflow: "hidden" }}>
        {/* 左栏：实时转写 + 实时翻译 */}
        <div style={{ flex: 1, display: "flex", flexDirection: "column", padding: 12, minWidth: 0 }}>
          <div style={{ flex: 1, display: "flex", flexDirection: "column", minHeight: 0, ...PANEL_STYLE, marginBottom: 10 }}>
            <h2 style={{ fontSize: 13, margin: "0 0 8px", color: "#94a3b8" }}>实时转写</h2>
            <div style={{ flex: 1, minHeight: 0 }}>
              <TranscriptSection lines={lines} />
            </div>
          </div>
          <div style={{ ...PANEL_STYLE, marginBottom: 0 }}>
            <h2 style={{ fontSize: 13, margin: "0 0 8px", color: "#94a3b8" }}>实时翻译</h2>
            <TranslationSection items={translations} />
          </div>
        </div>

        {/* 右栏：术语 / 简报 / 历史 / 设置 */}
        <div style={{ width: 360, borderLeft: "1px solid rgba(255,255,255,0.1)", padding: 12, overflowY: "auto" }}>
          <div style={{ ...PANEL_STYLE }}>
            <h2 style={{ fontSize: 13, margin: "0 0 8px", color: "#94a3b8" }}>术语</h2>
            <TermsSection items={terms} />
          </div>
          <div style={{ ...PANEL_STYLE }}>
            <h2 style={{ fontSize: 13, margin: "0 0 8px", color: "#94a3b8" }}>简报</h2>
            <BriefSection items={briefs} />
          </div>

          {showHistory && (
            <div style={PANEL_STYLE}>
              <h2 style={{ fontSize: 13, margin: "0 0 8px", color: "#94a3b8" }}>历史会话</h2>
              <HistorySection
                sessions={sessions}
                searchResults={searchResults}
                detail={detail}
                templates={templates}
                onSearch={handleHistorySearch}
                onSelect={handleHistorySelect}
                onRefresh={refreshHistory}
                onGenerateNotes={handleGenerateNotes}
                notesBusy={notesBusy}
              />
            </div>
          )}

          {showSettings && (
            <div style={PANEL_STYLE}>
              <h2 style={{ fontSize: 13, margin: "0 0 8px", color: "#94a3b8" }}>设置</h2>
              <SettingsSection config={config} onSave={api.saveConfig} />
            </div>
          )}
        </div>
      </div>

      {/* 调试窗口（模态） */}
      {showDebug && (
        <DebugWindow events={rawEvents} readLogs={api.readLogs} onClose={() => setShowDebug(false)} />
      )}
    </div>
  );
}
