import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { getApi } from "./lib/transport";
import type { AppConfig, DomainEvent, NotesTemplate, SegmentHit, SessionDetail, SessionRecord } from "./lib/api";
import { TranscriptAccumulator } from "./lib/transcript";
import { KeyPointAggregator, type KeyPoint } from "./lib/highlights";
import { cssVars, type Theme } from "./lib/theme";
import { loadTheme, saveTheme, loadTranscriptMode, saveTranscriptMode, loadNavPage, saveNavPage, loadAsideCollapsed, saveAsideCollapsed } from "./lib/prefs";
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

export default function App() {
  // 主题 / 转写模式从持久化偏好恢复
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const [version, setVersion] = useState<string>("—");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [listening, setListening] = useState(false);
  const [status, setStatus] = useState<string>("待机");
  const [navPage, setNavPage] = useState<string>(() => loadNavPage());
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
  const [showDebug, setShowDebug] = useState(false);
  const [noiseLevel, setNoiseLevel] = useState(0); // 0..100（UI 百分比）
  const prevNoiseRef = useRef(-1);
  const accumulatorRef = useRef(new TranscriptAccumulator());
  const pointsRef = useRef(new KeyPointAggregator());
  const lastTranslationRef = useRef<Record<string, string>>({});

  useEffect(() => {
    api.getVersion().then(setVersion).catch(console.error);
    api.getConfig().then(setConfig).catch(console.error);
    // 启动时若恢复的是历史页，先刷新会话列表
    if (navPage === "history") refreshHistory();
    const off = api.onEvent((ev: DomainEvent) => {
      if (ev.type === "status") {
        setStatus(ev.message);
        if (ev.stage === "recording") setListening(true);
        if (ev.stage === "idle" || ev.stage === "asr_ready") setListening(false);
      }
      if (ev.type === "segment") {
        const acc = accumulatorRef.current;
        acc.push(ev);
        setLines(
          acc.getLines().map((l) => {
            const st = SPEAKER_STYLE[l.speakerLabel] ?? { color: "var(--muted)", engine: "?" };
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
      setRawEvents((prev) => [...prev.slice(-199), ev]);
    });
    return off;
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

  // 噪音电平：监听中防抖同步到后端（0..100 → 0..0.1 RMS 门限），无需停止监听
  useEffect(() => {
    if (!listening) return;
    const t = setTimeout(() => {
      if (prevNoiseRef.current === noiseLevel) return;
      prevNoiseRef.current = noiseLevel;
      api
        .setNoiseLevel((noiseLevel / 100) * 0.1)
        .catch((e) => console.error("设置噪音电平失败:", e));
    }, 150);
    return () => clearTimeout(t);
  }, [noiseLevel, listening]);

  // 停止监听后重置噪音电平（新会话默认关闭）
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
        // 开始监听 → 自动跳转到实时转写页
        setNavPage("transcript");
        saveNavPage("transcript");
        await api.startListen();
      }
    } catch (e) {
      setStatus(`错误: ${e}`);
    }
  }, [listening]);

  const handleNavigate = useCallback(
    (key: string) => {
      setNavPage(key);
      saveNavPage(key);
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
      />

      {/* 主区：不滚动，滚动交给内部子区域（转写/要点/历史/设置各自 flex+overflow） */}
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", padding: 14, gap: 12, overflow: "hidden" }}>
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
                notesBusy={notesBusy}
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
