import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getApi } from "./lib/transport";
import type { AppConfig, AsrRuntimeStatus, ConversationMetrics, DomainEvent, NotesTemplate, NudgeEvent, SceneMode, SegmentHit, SessionDetail, SessionRecord, TrioSummary } from "./lib/api";
import { TranscriptAccumulator } from "./lib/transcript";
import { keyPointKey, toKeyPoint, type KeyPoint } from "./lib/highlights";
import { applyIncomingNudge } from "./lib/nudge";
import { termKey } from "./lib/terms";
import { cssVars, type Theme } from "./lib/theme";
import { loadTheme, saveTheme, loadTranscriptMode, saveTranscriptMode, loadAsideCollapsed, saveAsideCollapsed } from "./lib/prefs";
import SideNav, { type HealthRow, type NavItem } from "./components/SideNav";
import TranscriptCard, { fmtElapsed, type TimelineLine, type TranscriptMode } from "./components/TranscriptCard";
import KeyPointsCard from "./components/KeyPointsCard";
import AsidePanel from "./components/AsidePanel";
import HistorySection from "./sections/HistorySection";
import SettingsSection from "./sections/SettingsSection";
import ModelsSection from "./sections/ModelsSection";
import KnowledgeSection from "./sections/KnowledgeSection";
import ChatSection from "./sections/ChatSection";
import DebugWindow from "./sections/DebugWindow";
import type { TermItem } from "./sections/TermsSection";
import type { BriefItem } from "./sections/BriefSection";
import { knowledgeBaseSettings, knowledgeSourceReady, togglePinnedPath, type KnowledgeDoc } from "./lib/knowledge";

const api = getApi();

function fmtTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

const SPEAKER_STYLE: Record<string, { color: string; engine: string }> = {
  我: { color: "var(--me)", engine: "qwen3-asr" },
  客户: { color: "var(--client)", engine: "qwen3-asr" },
  对方: { color: "var(--client)", engine: "qwen3-asr" },
};

// 动态说话人（客户1/客户2…）循环配色
const CLIENT_COLORS = ["var(--client)", "var(--term)", "var(--brief)", "var(--live)", "var(--danger)", "var(--me)"];

/** 页头标题：与左侧导航项一一对应（语音转写页保留"会议辅助"这个业务名）。 */
const PAGE_TITLES: Record<string, string> = {
  transcript: "会议辅助",
  history: "历史会话",
  chat: "AI 助手",
  models: "模型管理",
  knowledge: "知识管理",
  settings: "设置",
};

const SCENE_LABELS: Record<SceneMode, string> = {
  dictation: "单人听写",
  conversation: "一对一会话",
  bilingual: "双语对话",
  live_translation: "实时翻译",
  meeting: "多人会议",
  lecture: "演讲/课堂",
  custom: "自定义",
};

function speakerStyle(label: string): { color: string; engine: string } {
  if (label === "我") return SPEAKER_STYLE["我"];
  const m = /^客户(\d+)$/.exec(label);
  if (m) {
    const n = Number(m[1]);
    return { color: CLIENT_COLORS[(n - 1) % CLIENT_COLORS.length], engine: "qwen3-asr" };
  }
  return SPEAKER_STYLE[label] ?? { color: "var(--muted)", engine: "?" };
}

export default function App() {
  // 主题 / 转写模式从持久化偏好恢复
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const [version, setVersion] = useState<string>("—");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [audioSource, setAudioSource] = useState<"mic" | "loopback">("mic");
  const [gpuStatus, setGpuStatus] = useState<AsrRuntimeStatus | null>(null);
  const [listening, setListening] = useState(false);
  const [paused, setPaused] = useState(false);
  const [status, setStatus] = useState<string>("待机");
  const [listenElapsed, setListenElapsed] = useState(0);
  // 计时核心：{ start: 当前活跃段起点（暂停时置 0）, accumMs: 暂停前已累计毫秒 }。
  // 用 ref 避免 onEvent 一次性注册闭包里的过期值；暂停期间 accumMs 冻结、计时不走。
  const timerRef = useRef({ start: 0, accumMs: 0 });
  // 启动默认进入「实时转写」页（不跨启动恢复导航页）
  const [navPage, setNavPage] = useState<string>("transcript");
  // 设置页有未保存改动（由 SettingsSection 上报）：离开前需要确认。
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [micRms, setMicRms] = useState(0); // 麦克风电平（Level 事件）
  const [asideCollapsed, setAsideCollapsed] = useState<boolean>(() => loadAsideCollapsed());
  const [mode, setMode] = useState<TranscriptMode>(() => loadTranscriptMode());
  const [lines, setLines] = useState<TimelineLine[]>([]);
  const [points, setPoints] = useState<readonly KeyPoint[]>([]);
  const [dismissedPointKeys, setDismissedPointKeys] = useState<ReadonlySet<string>>(() => new Set());
  // done = 已收到后端回执（msg 才是最终结论，此前只是"整理中…"）
  const [flushRecords, setFlushRecords] = useState<readonly { time: string; pointsBefore: number; msg: string; done?: boolean }[]>([]);
  const [terms, setTerms] = useState<TermItem[]>([]);
  const [expandedTerms, setExpandedTerms] = useState<Record<string, boolean>>({});
  // 用户在当前会话主动删除的术语；后续其他插件再返回同一词也不显示。
  const [dismissedTermKeys, setDismissedTermKeys] = useState<ReadonlySet<string>>(() => new Set());
  const [briefs, setBriefs] = useState<BriefItem[]>([]);
  const [kbDocs, setKbDocs] = useState<KnowledgeDoc[]>([]);
  const [pinnedNotePaths, setPinnedNotePaths] = useState<string[]>([]);
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
  // 关闭窗口确认（监听中点击 X → 弹窗确认；避免误关丢掉未收尾的会话）
  const [confirmClose, setConfirmClose] = useState(false);
  // 文件导入状态
  const [importing, setImporting] = useState(false);
  const [importFile, setImportFile] = useState<string | null>(null);
  const [importDone, setImportDone] = useState(false);
  const [importSessionId, setImportSessionId] = useState<number | null>(null);
  const [mediaProgress, setMediaProgress] = useState({ positionMs: 0, totalMs: 0, speed: 1 });
  const [noiseLevel, setNoiseLevel] = useState(0); // 0..100（UI 百分比）
  const prevNoiseRef = useRef(-1);
  // onCloseRequested 只注册一次，用 ref 取最新监听状态，避免闭包过期
  const listeningRef = useRef(listening);
  listeningRef.current = listening;
  const accumulatorRef = useRef(new TranscriptAccumulator());
  const lastTranslationRef = useRef<Record<string, string>>({});

  useEffect(() => {
    api.getVersion().then(setVersion).catch(console.error);
    api.getConfig().then((cfg) => { setConfig(cfg); setAudioSource(cfg.audio?.audio_source ?? "mic"); }).catch(console.error);
    api.getAsrRuntimeStatus().then(setGpuStatus).catch((error) => console.error("读取 ASR 运行状态失败:", error));
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
    // 关闭窗口守卫：监听中点击 X 不允许直接关闭，弹确认对话框。
    // 确认后先优雅停止（落库 + 录音收尾），再 destroy（destroy 不触发
    // close-requested，避免再次进入守卫死循环）。
    let closeGuard: (() => void) | undefined;
    if (isTauri) {
      getCurrentWindow()
        .onCloseRequested(async (event) => {
          if (listeningRef.current) {
            // 监听中：拦截关闭，弹确认对话框
            event.preventDefault();
            setConfirmClose(true);
            return;
          }
          // 未监听：不 preventDefault，由 Tauri wrapper 自动 destroy 关闭。
          // （wrapper：handler 返回后若未 preventDefault 则自动 destroy。
          // 不要再手动 destroy，避免双重关闭。）
        })
        .then((fn) => {
          closeGuard = fn;
        })
        .catch((err) => console.error("注册关闭守卫失败:", err));
    }
    const off = api.onEvent((ev: DomainEvent) => {
      if (ev.type === "status") {
        setStatus(ev.message);
        if (ev.stage === "recording" || ev.stage === "importing") {
          setListening((prev) => {
            if (!prev) {
              // 全新开始：清零计时
              timerRef.current = { start: Date.now(), accumMs: 0 };
              setListenElapsed(0);
            } else {
              // 从暂停恢复：start 重新指向当前时刻，保留已累计时长
              timerRef.current = { ...timerRef.current, start: Date.now() };
            }
            return true;
          });
          setPaused(false);
        }
        if (ev.stage === "paused") {
          setListening((prev) => {
            if (!prev) {
              // 异常顺序（未 recording 直接 paused）：按全新开始处理
              timerRef.current = { start: Date.now(), accumMs: 0 };
              setListenElapsed(0);
            } else {
              // 正常暂停：把当前活跃段累计进 accumMs，start 冻结（暂停期间不走表）
              const t = timerRef.current;
              if (t.start > 0) {
                timerRef.current = { start: 0, accumMs: t.accumMs + (Date.now() - t.start) };
              }
            }
            return true;
          });
          setPaused(true);
        }
        if (ev.stage === "idle") {
          setListening(false);
          setPaused(false);
          timerRef.current = { start: 0, accumMs: 0 };
        }
      }
      if (ev.type === "media_progress") {
        setMediaProgress({ positionMs: ev.position_ms, totalMs: ev.total_ms, speed: ev.speed });
        setListenElapsed(Math.floor(ev.position_ms / 1000));
      }
      if (ev.type === "media_completed") {
        setImporting(false);
        setImportDone(!ev.cancelled && !ev.error);
        setImportSessionId(ev.session_id ?? null);
        setListening(false);
        setPaused(false);
        setStatus(ev.error ? `文件转写失败: ${ev.error}` : ev.cancelled ? "文件转写已停止" : "文件转写完成");
        if (ev.error) window.alert(`文件转写失败：${ev.error}`);
      }
      if (ev.type === "snapshot") {
        const acc = accumulatorRef.current;
        acc.applySnapshot(
          ev.committed.map((s) => ({
            speaker_id: s.speaker_id,
            speaker_label: s.speaker_label,
            text: s.text,
            is_partial: false,
            ts_ms: s.ts_ms,
          })),
          ev.hypothesis.map((h) => ({
            speaker_id: h.speaker_id,
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
      }
      if (ev.type === "key_point") {
        const item = toKeyPoint(ev);
        setPoints((prev) => {
          const idx = prev.findIndex((p) => p.resultId === item.resultId);
          if (idx >= 0) {
            const next = [...prev];
            next[idx] = item;
            return next;
          }
          const next = [...prev, item];
          return next.length > 80 ? next.slice(-80) : next;
        });
      }
      if (ev.type === "term") {
        setTerms((prev) => {
          // content="" 且 final 是骨架撤销信号，移除该卡片
          if (ev.status === "final" && ev.content === "") {
            return prev.filter((t) => t.resultId !== ev.result_id);
          }
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
        // 双语预设按输入通道标记“我/对方”；保留“客户”兼容历史事件显示。
        const label = ev.direction === "en_zh" ? "对方" : "我";
        lastTranslationRef.current[label] = ev.content;
        if (label === "对方") lastTranslationRef.current["客户"] = ev.content;
      }
      if (ev.type === "brief") {
        setBriefs((prev) => [...prev.slice(-19), { source: ev.source, text: ev.text }]);
      }
      if (ev.type === "metrics") {
        setMetrics(ev.metrics);
      }
      if (ev.type === "key_point_flush") {
        // 命令在 LLM 返回前就已返回，这里才是真正的结果：补到最后一条整理记录上
        setFlushRecords((prev) =>
          prev.length === 0
            ? prev
            : prev.map((r, i) => (i === prev.length - 1 ? { ...r, msg: ev.message, done: true } : r)),
        );
        return;
      }
      if (ev.type === "nudge") {
        setNudges((prev) => applyIncomingNudge(prev, ev.nudge));
      }
      setRawEvents((prev) => [...prev.slice(-199), ev]);
    });
    return () => {
      minimizeListener?.();
      closeGuard?.();
      off();
    };
  }, []);

  // headless 的 /config 不返回 scene（浏览器模式），这里必须容缺，否则整页崩在启动阶段
  const currentSceneLabel = config?.scene?.mode ? SCENE_LABELS[config.scene.mode] : "加载中…";

  const asrBackendLabel = (() => {
    if (!config) return "加载中…";
    if (!gpuStatus) return "检测中…";
    if (gpuStatus.route_error) return "配置不可用";
    if (gpuStatus.effective_route) return gpuStatus.effective_route;
    return gpuStatus.is_accelerated ? `GPU (${gpuStatus.display_name})` : gpuStatus.display_name;
  })();

  // 运行状态行：场景置顶，监听过程中始终可见。
  const healthRows: HealthRow[] = [
    { dot: "var(--term)", label: "场景", value: currentSceneLabel },
    {
      dot: paused ? "var(--brief)" : listening ? "var(--live)" : "var(--muted)",
      label: "监听",
      value: paused
        ? "暂停"
        : listening
          ? `活跃 ${fmtElapsed(listenElapsed)}`
          : "待机",
    },
    { dot: "var(--client)", label: "客户流(VAD)", value: "双流" },
    { dot: "var(--me)", label: "用户流", value: "qwen3-asr" },
    { dot: gpuStatus?.route_error ? "var(--danger)" : gpuStatus?.is_accelerated ? "var(--live)" : "var(--brief)", label: "ASR", value: asrBackendLabel },
  ];

  const navItems: NavItem[] = [
    { key: "transcript", label: "语音转写", dot: importing ? "var(--brief)" : listening ? "var(--live)" : "var(--muted)", badge: String(lines.length), active: navPage === "transcript" },
    { key: "history", label: "历史会话", dot: "var(--term)", badge: String(sessions.length), active: navPage === "history" },
    { key: "chat", label: "AI 助手", dot: "var(--me)", badge: "", active: navPage === "chat" },
    { key: "models", label: "模型管理", dot: "var(--client)", badge: "", active: navPage === "models" },
    { key: "knowledge", label: "知识管理", dot: "var(--term)", badge: "", active: navPage === "knowledge" },
    { key: "settings", label: "设置", dot: "var(--brief)", badge: "", active: navPage === "settings" },
  ];

  const refreshHistory = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
    } catch (e) {
      console.error("历史加载失败:", e);
    }
  }, []);

  const handleSaveConfig = useCallback(async (updates: Record<string, unknown>) => {
    await api.saveConfig(updates);
    // ASR 路由状态依赖刚保存的 asr_mode/backend/云端凭据。
    // 设置页有自己的状态用于就地预览，但左侧运行状态读取的是
    // App 层 gpuStatus，因此保存后必须同时刷新两者。
    const [newCfg, runtimeStatus] = await Promise.all([
      api.getConfig(),
      api.getAsrRuntimeStatus(),
    ]);
    setConfig(newCfg);
    setGpuStatus(runtimeStatus);
    setAudioSource(newCfg.audio?.audio_source ?? "mic");
  }, []);

  useEffect(() => {
    if (!knowledgeSourceReady(config)) {
      setKbDocs([]);
      return;
    }
    let cancelled = false;
    api
      .listKnowledgeDocuments()
      .then((docs) => {
        if (cancelled) return;
        setKbDocs(docs);
        setPinnedNotePaths((prev) => prev.filter((p) => docs.some((d) => d.path === p)));
      })
      .catch((e) => {
        console.error("读取知识库文档失败:", e);
        if (!cancelled) setKbDocs([]);
      });
    return () => {
      cancelled = true;
    };
  }, [config]);

  const handleToggleAudioSource = useCallback(async () => {
    const next: "mic" | "loopback" = audioSource === "mic" ? "loopback" : "mic";
    setAudioSource(next);
    await api.saveConfig({ audio: { audio_source: next } });
  }, [audioSource]);

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

  // 监听中每秒更新计时器（暂停期间 accumMs 冻结、start=0，显示值不变）
  useEffect(() => {
    if (!listening) { setListenElapsed(0); return; }
    const id = setInterval(() => {
      const t = timerRef.current;
      const totalMs = t.accumMs + (t.start > 0 ? Date.now() - t.start : 0);
      setListenElapsed(Math.floor(totalMs / 1000));
    }, 1000);
    return () => clearInterval(id);
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

  /** 重命名会话（历史列表 / 详情页共用）。空串 = 清除自定义名。 */
  const handleRenameSession = useCallback(
    async (id: number, title: string) => {
      try {
        await api.renameSession(id, title);
        await refreshHistory();
        // 正开着这条会话时同步刷新详情头部，避免列表已改、详情还是旧名字
        setDetail((d) => (d && d.id === id ? { ...d, title: title.trim() || null } : d));
      } catch (e) {
        console.error("重命名会话失败:", e);
        alert(`重命名失败: ${e}`);
      }
    },
    [refreshHistory],
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

  /** 编辑某条转写段（历史详情纠错）。后端会顺带清除旧纪要/要点。 */
  const handleUpdateSegment = useCallback(async (sessionId: number, segmentId: number, text: string) => {
    await api.updateSegment(sessionId, segmentId, text);
  }, []);

  /** 删除某条转写段（历史详情纠错）。 */
  const handleDeleteSegment = useCallback(async (sessionId: number, segmentId: number) => {
    await api.deleteSegment(sessionId, segmentId);
  }, []);

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

  /** 导出会话为纯文本转写（无 Markdown 标记）。 */
  const handleExportText = useCallback(
    async (id: number): Promise<string> => {
      try {
        const { path, content } = await api.exportSessionText(id);
        const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
        const url = URL.createObjectURL(blob);
        const a = document.createElement("a");
        a.href = url;
        a.download = `session-${id}.txt`;
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

  /** 导出会话完整录音（桌面端复制到 exports/ 并返回路径；浏览器模式直接下载）。 */
  const handleExportAudio = useCallback(
    async (id: number): Promise<string> => {
      try {
        return await api.exportSessionAudio(id);
      } catch (e) {
        console.error("导出录音失败:", e);
        alert(`导出录音失败: ${e}`);
        return "";
      }
    },
    [],
  );

  /** 新监听会话必须从空白的实时上下文开始；暂停/继续不调用此方法。 */
  const resetLiveSession = useCallback(() => {
    accumulatorRef.current.reset();
    lastTranslationRef.current = {};
    setLines([]);
    setPoints([]);
    setDismissedPointKeys(new Set());
    setFlushRecords([]);
    setTerms([]);
    setExpandedTerms({});
    setDismissedTermKeys(new Set());
    setBriefs([]);
    setRawEvents([]);
    setMetrics(null);
    setNudges([]);
    setMicRms(0);
    setMediaProgress({ positionMs: 0, totalMs: 0, speed: 1 });
  }, []);

  // 设置页是切走即卸载：有未保存改动时先确认，否则改动静默丢失（尤其是场景模式，
  // 用户会带着旧场景开始监听还以为换了）。返回 false = 用户选择留下，中止本次动作。
  const confirmLeaveSettings = useCallback(() => {
    if (navPage !== "settings" || !settingsDirty) return true;
    return window.confirm("设置有未保存的改动，离开后将丢失。确定离开？");
  }, [navPage, settingsDirty]);

  const handleListen = useCallback(async () => {
    if (!listening && !confirmLeaveSettings()) return;
    try {
      if (listening) {
        await api.stopListen();
        setListening(false);
        setPaused(false);
        timerRef.current = { start: 0, accumMs: 0 };
        setListenElapsed(0);
        setStatus("已停止");
      } else {
        setStatus("启动中…");
        // 开始监听 → 自动跳转到实时转写页
        setNavPage("transcript");
        // 后端会创建全新的 SessionRuntime；前端也必须同步丢弃上一会话的聚合状态。
        // 必须在 startListen 前清空，避免启动后立即到达的新事件被误删。
        resetLiveSession();
        await api.startListen(pinnedNotePaths);
      }
    } catch (e) {
      const msg = String(e).replace(/^Error: /, "");
      setStatus(`错误: ${msg}`);
      if (!listening) {
        // 启动失败：原因可能很关键（麦克风被占/模型缺失），弹窗让用户看到处理建议
        window.alert(`启动失败：${msg}`);
      }
    }
  }, [confirmLeaveSettings, listening, pinnedNotePaths, resetLiveSession]);

  const handlePause = useCallback(async () => {
    if (!listening) return;
    try {
      await api.setListenPaused(!paused);
    } catch (e) {
      setStatus(`错误: ${e}`);
    }
  }, [listening, paused]);

  /** 打开文件选择器，选中后开始文件导入转写。 */
  const handleFileImport = useCallback(async () => {
    if (listening || importing) return;
    if (!confirmLeaveSettings()) return;
    const path = await api.pickAudioFile();
    if (!path) return;
    const filename = path.replace(/\\/g, "/").split("/").pop() ?? path;
    resetLiveSession();
    setImportFile(filename);
    setImportDone(false);
    setImportSessionId(null);
    setImporting(true);
    setListening(true);
    setStatus("正在加载媒体与 ASR…");
    setNavPage("transcript");
    try {
      const sid = await api.startFileImport(path);
      setImportSessionId(sid);
    } catch (e) {
      const msg = String(e);
      if (!msg.includes("已取消")) {
        window.alert(`导入失败：${msg}`);
      }
      setImportFile(null);
      setImporting(false);
      setListening(false);
    }
  }, [confirmLeaveSettings, listening, importing, resetLiveSession]);

  /** 取消正在进行的文件导入。 */
  const handleCancelImport = useCallback(async () => {
    await api.stopListen().catch((error) => setStatus(`停止失败: ${error}`));
  }, []);

  /** 重置文件导入区域，回到空白状态。 */
  const handleResetImport = useCallback(() => {
    setImportFile(null);
    setImportDone(false);
    setImportSessionId(null);
    resetLiveSession();
  }, [resetLiveSession]);

  /** 确认关闭：先优雅停止监听（会话落库、录音收尾），再退出应用。 */
  const handleConfirmClose = useCallback(async () => {
    setConfirmClose(false);
    const win = getCurrentWindow();
    try {
      if (listeningRef.current) {
        // 后端 stop_listen 已移出主线程（不再冻结窗口），但仍可能因 finalizer
        // （webhook 等）耗时数秒——显示状态避免用户以为卡死。
        setStatus("正在停止并保存…");
        // 兜底：即使后端异常挂起，也必须在超时后强制退出，不能把用户困在假死里。
        await Promise.race([
          api.stopListen().catch((e) => console.error("退出前停止监听失败:", e)),
          new Promise((resolve) => setTimeout(resolve, 15000)),
        ]);
        setListening(false);
        setPaused(false);
        timerRef.current = { start: 0, accumMs: 0 };
      }
    } finally {
      // 优先优雅销毁窗口；destroy 不触发 close-requested，不会再次进入守卫循环。
      // 若 destroy 因 ACL 或其他原因失败，回退到 Rust 侧 app.exit（最可靠）。
      try {
        await win.destroy();
      } catch (err) {
        console.error("销毁窗口失败，改用应用退出:", err);
        await api.quitApp().catch((e) => console.error("应用退出失败:", e));
      }
    }
  }, []);

  // 应用内快捷键：Cmd/Ctrl+Shift+L 开始/停止；监听期间 Space 暂停/继续。
  // 输入框、文本域和可编辑内容中不接管按键，避免影响正常输入。
  useEffect(() => {
    const onKeyDown = (ev: KeyboardEvent) => {
      const target = ev.target as HTMLElement | null;
      const editing = target?.isContentEditable || target?.tagName === "INPUT" || target?.tagName === "TEXTAREA" || target?.tagName === "SELECT";
      if (editing || ev.repeat) return;
      if (ev.code === "KeyL" && ev.shiftKey && (ev.metaKey || ev.ctrlKey)) {
        ev.preventDefault();
        void handleListen();
      } else if (ev.code === "KeyD" && ev.shiftKey && (ev.metaKey || ev.ctrlKey)) {
        ev.preventDefault();
        setShowDebug(v => !v);
      } else if (ev.code === "Space" && listening && !ev.metaKey && !ev.ctrlKey && !ev.altKey) {
        ev.preventDefault();
        void handlePause();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handleListen, handlePause, listening]);

  const handleNavigate = useCallback(
    (key: string) => {
      if (key !== "settings" && !confirmLeaveSettings()) return;
      setNavPage(key);
      if (key === "history") refreshHistory();
    },
    [confirmLeaveSettings, refreshHistory],
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
        paused={paused}
        importing={importing}
        audioSource={audioSource}
        onToggleAudioSource={handleToggleAudioSource}
        onToggleListen={handleListen}
        onTogglePause={handlePause}

        onNavigate={handleNavigate}
        noiseLevel={noiseLevel}
        onNoiseLevel={setNoiseLevel}
        micRms={micRms}
        version={version}
        transport={api.transport}
      />

      {/* 主区：不滚动，滚动交给内部子区域（转写/要点/历史/设置各自 flex+overflow） */}
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", padding: 14, gap: 12, overflow: "hidden", position: "relative" }}>
        {/* 页头：各页面共用同一层级的标题；场景按钮与计时徽章只属于语音转写
            （它们说的是"这场会"，在历史/助手/模型/知识/设置页没有意义） */}
        <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
          <h1 style={{ fontSize: 18, margin: 0 }}>{PAGE_TITLES[navPage] ?? "会议辅助"}</h1>
          {navPage === "chat" && (
            <span style={{ fontSize: 11, color: "var(--muted)" }}>
              多轮对话，回答由「设置 → LLM」配置的模型生成
            </span>
          )}
          {navPage === "transcript" && (
          <button
            type="button"
            onClick={() => setNavPage("settings")}
            title="当前场景模式；点击打开设置"
            style={{
              border: "1px solid var(--border)",
              borderRadius: 10,
              background: "var(--term-soft)",
              color: "var(--term)",
              padding: "4px 10px",
              fontSize: 12,
              fontWeight: 700,
              cursor: "pointer",
            }}
          >
            场景 · {currentSceneLabel}
          </button>
          )}
          {navPage === "transcript" && (
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
            {listening ? `● ${currentSceneLabel} · ${fmtElapsed(listenElapsed)}` : status}
          </span>
          )}
        </div>

        {navPage === "transcript" && (
          <>
            {/* 会中提示：同时只显示一条，可手动关闭 */}
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

            {/* 文件导入：进度条 / 完成操作栏 */}
            {(importing || importDone) && importFile && (
              <div style={{ display: "flex", alignItems: "center", gap: 10, padding: "7px 14px", background: "var(--surface-2)", borderRadius: 9, border: "1px solid var(--border)", fontSize: 13, flexWrap: "wrap" }}>
                {importing
                  ? <span style={{ color: "var(--brief)", fontWeight: 600 }}>● 转写中</span>
                  : <span style={{ color: "var(--live)", fontWeight: 600 }}>✓ 转写完成</span>
                }
                <span style={{ color: "var(--muted)" }}>{importFile}</span>
                <span style={{ color: "var(--muted)", fontSize: 11 }}>{lines.length} 段</span>
                {importing && mediaProgress.totalMs > 0 && (
                  <>
                    <progress value={mediaProgress.positionMs} max={mediaProgress.totalMs} style={{ width: 130 }} />
                    <span style={{ color: "var(--muted)", fontSize: 11 }}>
                      {fmtTime(mediaProgress.positionMs)} / {fmtTime(mediaProgress.totalMs)}
                    </span>
                    {[1, 2, 4, 0].map((speed) => (
                      <button
                        key={speed}
                        onClick={() => api.setFilePlaybackSpeed(speed).catch((error) => setStatus(`调速失败: ${error}`))}
                        style={{ padding: "3px 7px", borderRadius: 6, cursor: "pointer", border: "1px solid var(--border)", background: mediaProgress.speed === speed ? "var(--live)" : "var(--surface)", color: mediaProgress.speed === speed ? "#fff" : "var(--text)" }}
                      >
                        {speed === 0 ? "极速" : `${speed}x`}
                      </button>
                    ))}
                  </>
                )}
                <span style={{ marginLeft: "auto", display: "flex", gap: 8 }}>
                  {importing && (
                    <button onClick={handleCancelImport} style={{ padding: "4px 12px", borderRadius: 7, cursor: "pointer", border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}>
                      取消
                    </button>
                  )}
                  {importDone && importSessionId != null && (
                    <>
                      <button
                        onClick={() => { setNavPage("history"); void refreshHistory(); }}
                        style={{ padding: "4px 12px", borderRadius: 7, cursor: "pointer", border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}
                      >
                        查看历史
                      </button>
                      <button onClick={handleResetImport} style={{ padding: "4px 12px", borderRadius: 7, cursor: "pointer", border: "none", background: "var(--live)", color: "#fff", fontWeight: 600 }}>
                        新导入
                      </button>
                    </>
                  )}
                </span>
              </div>
            )}

            {/* 空闲状态：输入源选择器 */}
            {!listening && !importing && !importDone && (
              <div style={{ display: "flex", gap: 10, padding: "10px 0" }}>
                <button
                  onClick={handleFileImport}
                  style={{ display: "flex", alignItems: "center", gap: 6, padding: "9px 18px", borderRadius: 9, cursor: "pointer", border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)", fontSize: 13 }}
                >
                  📂 导入录音文件
                </button>
                <span style={{ alignSelf: "center", fontSize: 11, color: "var(--muted)" }}>或使用左侧"开始监听"进行实时转写 · 支持 WAV、MP3、MP4</span>
              </div>
            )}

            <TranscriptCard
              mode={mode}
              setMode={(m) => { setMode(m); saveTranscriptMode(m); }}
              meta={importing || importDone
                ? `${lines.length} 段 · 文件导入`
                : `${lines.length} 段 · ${mode === "timeline" ? "时间线" : mode === "focus" ? "专注" : "密集"}`}
              lines={lines}
              timer={{ listening, paused, seconds: listenElapsed }}
            />
            <KeyPointsCard
                points={points}
                flushRecords={flushRecords}
                dismissedKeys={dismissedPointKeys}
                onDelete={(text) => {
                  const key = keyPointKey(text);
                  if (!key) return;
                  setDismissedPointKeys((prev) => new Set(prev).add(key));
                }}
                listening={listening}
                onFlush={async () => {
                  const now = new Date();
                  const time = `${String(now.getHours()).padStart(2,"0")}:${String(now.getMinutes()).padStart(2,"0")}`;
                  const pointsBefore = points.length;
                  // 先插入 pending，避免极快回执早于 UI 记录而丢失。
                  setFlushRecords((prev) => [...prev, { time, pointsBefore, msg: "整理中…", done: false }]);
                  try {
                    const started = await api.flushKeyPoints();
                    if (started && !started.startsWith("已启动")) {
                      setFlushRecords((prev) => prev.map((r, i) => i === prev.length - 1 ? { ...r, msg: started, done: true } : r));
                    }
                  } catch (e) {
                    const msg = String(e).replace(/^Error: /, "");
                    setFlushRecords((prev) => prev.map((r, i) => i === prev.length - 1 ? { ...r, msg, done: true } : r));
                  }
                }}
                pluginLabel={(() => {
                  const llmEnabled = config?.plugins?.["key_point_llm"]?.enabled !== false;
                  if (llmEnabled) return config?.llm?.default ?? "deepseek";
                  return "已禁用";
                })()}
              />
          </>
        )}

        {navPage === "history" && (
          <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
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
                onExportText={handleExportText}
                onExportAudio={handleExportAudio}
                onGenerateHighlights={async (id) => api.generateHighlights(id)}
                onRenameSession={handleRenameSession}
                onDeleteSession={handleDeleteSession}
                onDeleteSessions={handleDeleteSessions}
                onUpdateSegment={handleUpdateSegment}
                onDeleteSegment={handleDeleteSegment}
                notesBusy={notesBusy}
                trioBusy={trioBusy}
              />
            </div>
          </section>
        )}

        {navPage === "chat" && (
          <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
            <div style={{ flex: 1, minHeight: 0, overflow: "hidden", padding: "var(--pad)", display: "flex", flexDirection: "column" }}>
              <ChatSection onOpenSettings={() => setNavPage("settings")} />
            </div>
          </section>
        )}

        {navPage === "models" && (
          <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
            <div style={{ flex: 1, minHeight: 0, overflow: "hidden", padding: "var(--pad)", display: "flex", flexDirection: "column" }}>
              <ModelsSection listening={listening} />
            </div>
          </section>
        )}

        {navPage === "knowledge" && (
          <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
            <div style={{ flex: 1, minHeight: 0, overflow: "hidden", padding: "var(--pad)", display: "flex", flexDirection: "column" }}>
              <KnowledgeSection
                config={config}
                onSave={handleSaveConfig}
                pinnedNotePaths={pinnedNotePaths}
                onTogglePin={(path) => setPinnedNotePaths((prev) => togglePinnedPath(prev, path))}
              />
            </div>
          </section>
        )}

        {navPage === "settings" && (
          <section style={{ background: "var(--card-bg)", border: "var(--card-border)", borderRadius: "var(--card-radius)", boxShadow: "var(--card-shadow)", overflow: "hidden", display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}>
            <div style={{ flex: 1, minHeight: 0, overflow: "hidden", padding: "var(--pad)", display: "flex", flexDirection: "column" }}>
              <SettingsSection
                config={config}
                onSave={handleSaveConfig}
                onOpenModels={() => { if (confirmLeaveSettings()) setNavPage("models"); }}
                onDirtyChange={setSettingsDirty}
              />
            </div>
          </section>
        )}
      </div>

      {/* 右栏：术语/知识库属于实时转写上下文，仅转写页显示（历史/设置页右侧为设置内容） */}
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
          knowledgeEnabled={knowledgeSourceReady(config)}
          knowledgeFolder={knowledgeBaseSettings(config).folder}
          knowledgeDocs={kbDocs}
          pinnedPaths={pinnedNotePaths}
          onTogglePin={(path) => setPinnedNotePaths((prev) => togglePinnedPath(prev, path))}
          expandedTerms={expandedTerms}
          onToggleTerm={(id) => setExpandedTerms((prev) => ({ ...prev, [id]: !prev[id] }))}
          dismissedTermKeys={dismissedTermKeys}
          onDeleteTerm={(term) => {
            const key = termKey(term);
            if (!key) return;
            setDismissedTermKeys((prev) => {
              const next = new Set(prev);
              next.add(key);
              return next;
            });
          }}
          onAskTerm={async (term) => {
            // 结果由后端以 Term 事件推回来（监听中还会一并入库），这里不手动插卡片
            await api.explainTerm(term);
          }}
        />
      )}

      {showDebug && <DebugWindow events={rawEvents} readLogs={api.readLogs} onClose={() => setShowDebug(false)} />}

      {/* 监听中关闭窗口的确认对话框（桌面端 X / Cmd+Q 触发 close-requested） */}
      {confirmClose && (
        <div
          style={{
            position: "fixed",
            inset: 0,
            background: "rgba(0,0,0,0.55)",
            zIndex: 1000,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
          onClick={() => setConfirmClose(false)}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            style={{
              width: 380,
              maxWidth: "calc(100vw - 48px)",
              background: "var(--surface)",
              border: "1px solid var(--border)",
              borderRadius: 12,
              padding: "18px 20px",
              boxSizing: "border-box",
              boxShadow: "0 8px 30px rgba(0,0,0,0.35)",
            }}
          >
            <div style={{ fontSize: 15, fontWeight: 700, marginBottom: 8, color: "var(--text)" }}>正在监听中</div>
            <div style={{ fontSize: 13, lineHeight: 1.7, color: "var(--text-2)", marginBottom: 16 }}>
              当前会话仍在实时监听，直接关闭会中断转写与录音。确定要停止监听并退出吗？
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
              <button
                onClick={() => setConfirmClose(false)}
                style={{ fontSize: 12, padding: "6px 14px", borderRadius: 8, cursor: "pointer", background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--border)" }}
              >
                继续监听
              </button>
              <button
                onClick={() => void handleConfirmClose()}
                style={{ fontSize: 12, padding: "6px 14px", borderRadius: 8, cursor: "pointer", background: "var(--danger)", color: "#fff", border: "none" }}
              >
                停止并退出
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
