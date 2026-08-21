import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getApi } from "./lib/transport";
import type { AppConfig, ConversationMetrics, DomainEvent, NotesTemplate, NudgeEvent, SegmentHit, SessionDetail, SessionRecord, TrioSummary } from "./lib/api";
import { TranscriptAccumulator } from "./lib/transcript";
import { KeyPointAggregator, type KeyPoint } from "./lib/highlights";
import { cssVars, type Theme } from "./lib/theme";
import { loadTheme, saveTheme, loadTranscriptMode, saveTranscriptMode, loadAsideCollapsed, saveAsideCollapsed } from "./lib/prefs";
import SideNav, { type HealthRow, type NavItem } from "./components/SideNav";
import TranscriptCard, { type TimelineLine, type TranscriptMode } from "./components/TranscriptCard";
import KeyPointsCard from "./components/KeyPointsCard";
import AsidePanel from "./components/AsidePanel";
import HistorySection from "./sections/HistorySection";
import SettingsSection from "./sections/SettingsSection";
import DebugWindow from "./sections/DebugWindow";
import type { TermItem } from "./sections/TermsSection";
import type { BriefItem } from "./sections/BriefSection";

const api = getApi();

function fmtTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

const SPEAKER_STYLE: Record<string, { color: string; engine: string }> = {
  我: { color: "var(--me)", engine: "paraformer-zh" },
  客户: { color: "var(--client)", engine: "zipformer-en" },
};

// 动态说话人（客户1/客户2…）循环配色
const CLIENT_COLORS = ["var(--client)", "var(--term)", "var(--brief)", "var(--live)", "var(--danger)", "var(--me)"];

function speakerStyle(label: string): { color: string; engine: string } {
  if (label === "我") return SPEAKER_STYLE["我"];
  const m = /^客户(\d+)$/.exec(label);
  if (m) {
    const n = Number(m[1]);
    return { color: CLIENT_COLORS[(n - 1) % CLIENT_COLORS.length], engine: "zipformer-en" };
  }
  return SPEAKER_STYLE[label] ?? { color: "var(--muted)", engine: "?" };
}

export default function App() {
  // 主题 / 转写模式从持久化偏好恢复
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const [version, setVersion] = useState<string>("—");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [listening, setListening] = useState(false);
  const [status, setStatus] = useState<string>("待机");
  // 启动默认进入「实时转写」页（不跨启动恢复导航页）
  const [navPage, setNavPage] = useState<string>("transcript");
  const [micRms, setMicRms] = useState(0); // 麦克风电平（Level 事件）
  const [asideCollapsed, setAsideCollapsed] = useState<boolean>(() => loadAsideCollapsed());
  const [mode, setMode] = useState<TranscriptMode>(() => loadTranscriptMode());
  const [lines, setLines] = useState<TimelineLine[]>([]);
  const [points, setPoints] = useState<readonly KeyPoint[]>([]);
  const [terms, setTerms] = useState<TermItem[]>([]);
  const [expandedTerms, setExpandedTerms] = useState<Record<string, boolean>>({});
  const [briefs, setBriefs] = useState<BriefItem[]>([]);
  const [rawEvents, setRawEvents] = useState<DomainEvent[]>([]);
  const [sessions, setSessions] = useState<SessionRecord[]>([]);
  const [searchResults, setSearchResults] = useState<SegmentHit[] | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [templates, setTemplates] = useState<NotesTemplate[]>([]);
  const [notesBusy, setNotesBusy] = useState(false);
  const [trioBusy, setTrioBusy] = useState(false);
  // 会话指标（会中实时；借鉴 Call.md）
  const [metrics, setMetrics] = useState<ConversationMetrics | null>(null);
  // 会中提示（规则 + 冷却；借鉴 Call.md nudge-engine）
  const [nudges, setNudges] = useState<NudgeEvent[]>([]);
  const [showDebug, setShowDebug] = useState(false);
  const [noiseLevel, setNoiseLevel] = useState(0); // 0..100（UI 百分比）
  const prevNoiseRef = useRef(-1);
  const accumulatorRef = useRef(new TranscriptAccumulator());
  const pointsRef = useRef(new KeyPointAggregator());
  const lastTranslationRef = useRef<Record<string, string>>({});

  useEffect(() => {
    api.getVersion().then(setVersion).catch(console.error);
    api.getConfig().then(setConfig).catch(console.error);
    // Windows 桌面：最小化 → 隐藏到系统托盘（托盘点击恢复；macOS 遵循系统惯例最小化到 Dock，不做此处理）
    const isTauri = !!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    const isWindows = /Windows/i.test(navigator.userAgent);
    let minimizeListener: (() => void) | undefined;
    if (isTauri && isWindows) {
      const onVisibility = () => {
        if (document.hidden) {
          getCurrentWindow()
            .isMinimized()
            .then((m) => {
              if (m) api.minimizeToTray().catch((e) => console.error("最小化到托盘失败:", e));
            })
            .catch(() => {});
        }
      };
      document.addEventListener("visibilitychange", onVisibility);
      minimizeListener = () => document.removeEventListener("visibilitychange", onVisibility);
    }
    const off = api.onEvent((ev: DomainEvent) => {
      if (ev.type === "status") {
        setStatus(ev.message);
        if (ev.stage === "recording") setListening(true);
        if (ev.stage === "idle" || ev.stage === "asr_ready") setListening(false);
      }
      if (ev.type === "snapshot") {
        const acc = accumulatorRef.current;
        acc.applySnapshot(
          ev.committed.map((s) => ({
            speaker_label: s.speaker_label,
            text: s.text,
            is_partial: false,
            ts_ms: s.ts_ms,
          })),
          ev.hypothesis.map((h) => ({
            speaker_label: h.speaker_label,
            text: h.text,
            is_partial: true,
            ts_ms: h.ts_ms,
          })),
        );
        setLines(
          acc.getLines().map((l) => {
            const st = speakerStyle(l.speakerLabel);
            return {
              key: l.key,
              time: fmtTime(l.tsMs),
              speaker: l.speakerLabel,
              speakerColor: st.color,
              engine: st.engine,
              text: l.text,
              isPartial: l.isPartial,
              translation: lastTranslationRef.current[l.speakerLabel],
            };
          }),
        );
        if (ev.stage === "recording") setListening(true);
      }
      if (ev.type === "level") {
        setMicRms(ev.mic_rms);
      }
      if (ev.type === "segment") {
        const acc = accumulatorRef.current;
        acc.push(ev);
        setLines(
          acc.getLines().map((l) => {
            const st = speakerStyle(l.speakerLabel);
            return {
              key: l.key,
              time: fmtTime(l.tsMs),
              speaker: l.speakerLabel,
              speakerColor: st.color,
              engine: st.engine,
              text: l.text,
              isPartial: l.isPartial,
              translation: lastTranslationRef.current[l.speakerLabel],
            };
          }),
        );
        if (!ev.is_partial) {
          if (pointsRef.current.push(ev.text, ev.ts_ms ?? Date.now())) {
            setPoints([...pointsRef.current.getItems()]);
          }
        }
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
        lastTranslationRef.current[ev.direction === "en_zh" ? "客户" : "我"] = ev.content;
      }
      if (ev.type === "brief") {
        setBriefs((prev) => [...prev.slice(-19), { source: ev.source, text: ev.text }]);
      }
      if (ev.type === "metrics") {
        setMetrics(ev.metrics);
      }
      if (ev.type === "nudge") {
        setNudges((prev) => [...prev.slice(-3), ev.nudge]);
      }
      setRawEvents((prev) => [...prev.slice(-199), ev]);
    });
    return () => {
      minimizeListener?.();
      off();
    };
  }, []);

  // 运行状态行
  const healthRows: HealthRow[] = [
    { dot: listening ? "var(--live)" : "var(--muted)", label: "监听", value: listening ? "活跃" : "待机" },
    { dot: "var(--client)", label: "客户流(VAD)", value: "双流" },
    { dot: "var(--me)", label: "用户流", value: "paraformer" },
    { dot: "var(--live)", label: "ASR", value: status },
  ];

  const navItems: NavItem[] = [
    { key: "transcript", label: "实时转写", dot: "var(--live)", badge: String(lines.length), active: navPage === "transcript" },
    { key: "history", label: "历史会话", dot: "var(--term)", badge: String(sessions.length), active: navPage === "history" },
    { key: "settings", label: "设置", dot: "var(--brief)", badge: "", active: navPage === "settings" },
  ];

  const refreshHistory = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
    } catch (e) {
      console.error("历史加载失败:", e);
    }
  }, []);

  // 噪音电平阈值：监听中防抖同步到后端（0..100 → 0..0.1 RMS 门限），无需停止监听
  useEffect(() => {
    if (!listening) return;
    const t = setTimeout(() => {
      if (prevNoiseRef.current === noiseLevel) return;
      prevNoiseRef.current = noiseLevel;
      api
        .setNoiseLevel((noiseLevel / 100) * 0.1)
        .catch((e) => console.error("设置噪音电平阈值失败:", e));
    }, 150);
    return () => clearTimeout(t);
  }, [noiseLevel, listening]);

  // 停止监听后重置噪音电平阈值（新会话默认关闭）
  useEffect(() => {
    if (!listening) {
      setNoiseLevel(0);
      prevNoiseRef.current = -1;
    }
  }, [listening]);

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

  const handleDeleteSession = useCallback(
    async (id: number) => {
      try {
        await api.deleteSession(id);
        setDetail(null);
        refreshHistory();
      } catch (e) {
        console.error("删除会话失败:", e);
        alert(`删除失败: ${e}`);
      }
    },
    [refreshHistory],
  );

  /** 批量删除（历史列表多选）。 */
  const handleDeleteSessions = useCallback(
    async (ids: number[]) => {
      if (ids.length === 0) return;
      try {
        await Promise.all(ids.map((id) => api.deleteSession(id)));
        setDetail(null);
        refreshHistory();
      } catch (e) {
        console.error("批量删除失败:", e);
        alert(`部分删除失败: ${e}`);
      }
    },
    [refreshHistory],
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

  /** 三段式智能纪要（概述 / 归属要点 / 行动项；借鉴 Call.md）。 */
  const handleGenerateTrio = useCallback(
    async (meetingName: string, meetingDescription: string) => {
      if (!detail) return;
      setTrioBusy(true);
      try {
        const trio: TrioSummary = await api.generateTrioNotes(detail.id, meetingName || undefined, meetingDescription || undefined);
        setDetail({ ...detail, trio: JSON.stringify(trio) });
      } catch (e) {
        console.error("智能纪要生成失败:", e);
        alert(`智能纪要生成失败: ${e}`);
      } finally {
        setTrioBusy(false);
      }
    },
    [detail],
  );

  /** 导出会话为 Markdown 单文件（转写 + 纪要 + 指标；借鉴 Call.md markdown-export）。 */
  const handleExportMarkdown = useCallback(
    async (id: number): Promise<string> => {
      try {
        const { path, content } = await api.exportSessionMarkdown(id);
        // 浏览器/WebView 侧再触发一次下载（桌面端 path 同时落盘）
        const blob = new Blob([content], { type: "text/markdown;charset=utf-8" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = `session-${id}.md`;
        a.click();
        URL.revokeObjectURL(url);
        return path;
      } catch (e) {
        console.error("导出失败:", e);
        alert(`导出失败: ${e}`);
        return "";
      }
    },
    [],
  );

  const handleListen = useCallback(async () => {
    try {
      if (listening) {
        await api.stopListen();
        setListening(false);
        setStatus("已停止");
      } else {
        setStatus("启动中…");
        // 开始监听 → 自动跳转到实时转写页
        setNavPage("transcript");
        await api.startListen();
      }
    } catch (e) {
      setStatus(`错误: ${e}`);
    }
  }, [listening]);

  const handleNavigate = useCallback(
    (key: string) => {
      setNavPage(key);
      if (key === "history") refreshHistory();
    },
    [refreshHistory],
  );

  const pageStyle: CSSProperties = {
    background: "var(--bg)",
    color: "var(--text)",
    height: "100vh",
    display: "flex",
    fontFamily: "system-ui, sans-serif",
    overflow: "hidden",
    ...(cssVars(theme) as CSSProperties),
  };

  return (
    <div style={pageStyle}>
      <SideNav
        theme={theme}
        onToggleTheme={() =>
          setTheme((t) => {
            const next = t === "dark" ? "light" : "dark";
            saveTheme(next);
            return next;
          })
        }
        navItems={navItems}
        healthRows={healthRows}
        listening={listening}
        onToggleListen={handleListen}
        onOpenDebug={() => setShowDebug(true)}
        onNavigate={handleNavigate}
        noiseLevel={noiseLevel}
        onNoiseLevel={setNoiseLevel}
        micRms={micRms}
        version={version}
        transport={api.transport}
      />

      {/* 主区：不滚动，滚动交给内部子区域（转写/要点/历史/设置各自 flex+overflow） */}
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", padding: 14, gap: 12, overflow: "hidden", position: "relative" }}>
        {/* 页头 */}
        <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
          <h1 style={{ fontSize: 18, margin: 0 }}>会议辅助</h1>
          <span style={{ fontSize: 11, color: "var(--muted)" }}>
            v{version} · {api.transport}
          </span>
          <span
            style={{
              marginLeft: "auto",
              fontSize: 11,
              padding: "3px 10px",
              borderRadius: 10,
              background: listening ? "var(--live-soft)" : "var(--surface-2)",
              color: listening ? "var(--live)" : "var(--muted)",
            }}
          >
            {listening ? "● VAD 双流活跃" : status}
          </span>
        </div>

        {navPage === "transcript" && (
          <>
            {/* 会中提示（借鉴 Call.md nudge-engine）：浮动 toast，可手动关闭 */}
            {nudges.length > 0 && (
              <div style={{ position: "absolute", top: 8, right: 8, zIndex: 60, display: "flex", flexDirection: "column", gap: 6, maxWidth: 340 }}>
                {nudges.map((n) => (
                  <div
                    key={n.id}
                    style={{
                      background: "var(--card-bg)",
                      border: "1px solid var(--border)",
                      borderRadius: 8,
                      padding: "8px 10px",
                      fontSize: 12,
                      boxShadow: "0 2px 10px rgba(0,0,0,.18)",
                      display: "flex",
                      gap: 8,
                      alignItems: "flex-start",
                    }}
                  >
                    <span style={{ color: n.severity === "high" ? "var(--danger)" : n.severity === "medium" ? "var(--term)" : "var(--brief)", fontSize: 14 }}>💡</span>
                    <div>
                      <div style={{ lineHeight: 1.5 }}>{n.message}</div>
                      <button
                        onClick={() => setNudges((prev) => prev.filter((x) => x.id !== n.id))}
                        style={{ fontSize: 10, marginTop: 4, padding: "1px 8px", cursor: "pointer" }}
                      >
                        知道了
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}

            {/* 会话指标条（借鉴 Call.md conversation-metrics）：发言占比 / 语速 / 提问 / 独白 / 打断 / 健康分 */}
            {metrics && metrics.segment_count_me + metrics.segment_count_them > 0 && (
              <div
                style={{
                  display: "flex",
                  gap: 14,
                  flexWrap: "wrap",
                  fontSize: 11,
                  background: "var(--card-bg)",
                  border: "var(--card-border)",
                  borderRadius: 8,
                  padding: "6px 12px",
                  alignItems: "center",
                }}
              >
                <b style={{ fontSize: 11 }}>会话指标</b>
                <span>
                  发言 <b style={{ color: "var(--me)" }}>{Math.round(metrics.talk_ratio_me * 100)}%</b> /{" "}
                  <b style={{ color: "var(--client)" }}>{Math.round(metrics.talk_ratio_them * 100)}%</b>
                </span>
                <span>
                  语速 <b>{Math.round(metrics.pace_wpm)}</b> WPM
                </span>
                <span>
                  提问 <b>{metrics.questions_me}</b>
                </span>
                <span>
                  独白 {metrics.monologue_detected ? "⚠ 是" : "否"}
                </span>
                <span>
                  打断 <b>{metrics.interruption_count}</b>
                </span>
                <span
                  style={{
                    color: metrics.health_score >= 70 ? "var(--live)" : metrics.health_score >= 50 ? "var(--term)" : "var(--brief)",
                    fontWeight: 700,
                  }}
                >
                  健康分 {metrics.health_score}
                </span>
              </div>
            )}

            <TranscriptCard
              mode={mode}
              setMode={(m) => {
                setMode(m);
                saveTranscriptMode(m);
              }}
              meta={`${lines.length} 段 · ${mode === "timeline" ? "时间线" : mode === "focus" ? "专注" : "密集"}`}
              lines={lines}
            />
            <KeyPointsCard points={points} />
          </>
        )}

        {navPage === "history" && (
          <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
            <div style={{ padding: "11px var(--pad)", borderBottom: "1px solid var(--border)" }}>
              <b style={{ fontSize: 13 }}>历史会话</b>
            </div>
            <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "var(--pad)" }}>
              <HistorySection
                sessions={sessions}
                searchResults={searchResults}
                detail={detail}
                templates={templates}
                onSearch={handleHistorySearch}
                onSelect={handleHistorySelect}
                onRefresh={refreshHistory}
                onGenerateNotes={handleGenerateNotes}
                onGenerateTrio={handleGenerateTrio}
                onExportMarkdown={handleExportMarkdown}
                onGenerateHighlights={async (id) => api.generateHighlights(id)}
                onDeleteSession={handleDeleteSession}
                onDeleteSessions={handleDeleteSessions}
                notesBusy={notesBusy}
                trioBusy={trioBusy}
              />
            </div>
          </section>
        )}

        {navPage === "settings" && (
          <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
            <div style={{ padding: "11px var(--pad)", borderBottom: "1px solid var(--border)" }}>
              <b style={{ fontSize: 13 }}>设置</b>
            </div>
            <div style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: "var(--pad)" }}>
              <SettingsSection config={config} onSave={api.saveConfig} />
            </div>
          </section>
        )}
      </div>

      {/* 右栏：术语/简报属于实时转写上下文，仅转写页显示（历史/设置页右侧为设置内容） */}
      {navPage === "transcript" && (
        <AsidePanel
          collapsed={asideCollapsed}
          onToggleCollapsed={() =>
            setAsideCollapsed((c) => {
              const next = !c;
              saveAsideCollapsed(next);
              return next;
            })
          }
          terms={terms}
          briefs={briefs}
          expandedTerms={expandedTerms}
          onToggleTerm={(id) => setExpandedTerms((prev) => ({ ...prev, [id]: !prev[id] }))}
        />
      )}

      {showDebug && <DebugWindow events={rawEvents} readLogs={api.readLogs} onClose={() => setShowDebug(false)} />}
    </div>
  );
}
