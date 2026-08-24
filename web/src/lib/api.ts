// 统一 API 抽象：核心域与传输层解耦。
//
// Tauri 模式 → IpcApi（invoke + 事件监听）
// headless 模式（M4 预留）→ HttpApi（fetch + WebSocket），实现同一接口。
//
// 界面代码只依赖本接口，不感知载体。

/** 应用配置快照（与 Rust 侧 Config 对应）。 */
export interface AppConfig {
  asr: {
    client_engine: string;
    user_engine: string;
    backend: string;
    terminology: {
      enabled: boolean;
      hotword_score: number;
      terms: string[];
      corrections: Record<string, string>;
    };
  };
  llm: {
    default: string;
    providers: Record<string, { base_url: string | null; model: string; api_key: string }>;
  };
  server: {
    enabled: boolean;
    host: string;
    port: number;
    token: string;
  };
  /**
   * 通用插件表：键是插件 id，值的结构由插件自己定义（`enabled` 是唯一约定键）。
   * 后端返回的是「插件默认值 + 用户覆盖」的生效配置，所以每个内置插件都会出现。
   */
  plugins: Record<string, { enabled?: boolean; [key: string]: unknown }>;
  audio: {
    input_gain_db: number;
    vad: { preset: "standard" | "sensitive" | "strict"; threshold: number | null };
    denoise: { enabled: boolean; gate_threshold: number; highpass: boolean };
    endpoint: { enabled: boolean; stable_ms: number; quiet_ms: number; force_quiet_ms: number; quiet_rms: number; min_segment_ms: number };
    min_segment_ms: number | null;
  };
  /** 会议录音（边用边录，形成测试闭环）。 */
  recording: {
    enabled: boolean;
    dir: string;
    clean_silence: boolean;
  };
  /** 会话质量评估（噪音检测阈值）。 */
  quality: {
    auto_detect: boolean;
    text_noise_threshold: number;
    min_speech_ratio: number;
    max_speech_ratio: number;
    silence_rms: number;
    high_rms: number;
  };
  /** 会议结束 Webhook（n8n/Zapier/CRM 推送）。 */
  webhooks: {
    enabled: boolean;
    urls: string[];
  };
  /** 场景模式是一组完整运行预设。 */
  scene: {
    mode: SceneMode;
    custom: SceneParams;
  };
  [key: string]: unknown;
}

export type SceneMode = "dictation" | "conversation" | "translation" | "meeting" | "lecture" | "custom";

/**
 * 插件元数据（与 Rust 侧 `plugin_metadata()` 对应）。设置页据此**生成**表单。
 *
 * `schema` 是兼容现有表单的默认配置；`config_schema` 是后端校验与外部客户端
 * 使用的结构化契约。
 */
export interface PluginMeta {
  /** 提交时 `plugins.<id>` 的键。 */
  id: string;
  /** 显示名（插件自己给；缺省是 id）。 */
  label: string;
  /** 插件用途说明。 */
  description?: string;
  /** descriptor 分类和执行阶段。 */
  category?: "infrastructure" | "analysis";
  phase?: "filter" | "observer" | "finalizer";
  capabilities?: string[];
  after?: string[];
  /** 是否受场景 allowlist 约束（「会议辅助功能」那一类）。 */
  analysis: boolean;
  /** 默认配置。键即配置键，值即默认值。 */
  schema: Record<string, unknown>;
  /** 结构化配置契约；旧后端可能不返回，因此为可选。 */
  config_schema?: {
    type: "object";
    additionalProperties: boolean;
    properties: Record<string, { type: string; default: unknown }>;
  };
  /**
   * 由宿主裁决的配置键：装配时被场景参数/运行期能力无条件覆盖，用户改不动。
   * 设置页把这些控件置灰 —— 能改却不生效的输入框比没有更糟。
   */
  host_managed: string[];
}

export interface PluginStatusInfo {
  id: string;
  label: string;
  status: "active" | "disabled" | "unavailable" | "invalid_config";
  missing_capabilities?: string[];
  issues?: Array<{ path: string; message: string }>;
}

/** 场景参数集（自定义模式可全量编辑）。 */
export interface SceneParams {
  vad_preset: "standard" | "sensitive" | "strict";
  vad_threshold: number | null;
  vad_min_speech_ms: number | null;
  vad_min_silence_ms: number | null;
  vad_max_speech_ms: number | null;
  denoise_enabled: boolean;
  denoise_gate: number;
  min_segment_ms: number;
  user_engine: string;
  client_enabled: boolean;
  client_engine: string;
  user_language: "zh" | "en";
  client_language: "zh" | "en";
  translation_mode: "off" | "client_to_user" | "bidirectional";
  /** 该场景允许启用的分析类插件 id；不在列表里的一律关闭（allowlist，非 denylist）。 */
  plugin_allowlist: string[];
  /** off=无角色；channel=按输入通道；voiceprint=WeSpeaker 多人聚类。 */
  speaker_mode: "off" | "channel" | "voiceprint";
  noise_auto_detect: boolean;
}

/** 领域事件（与 Rust 侧 DomainEvent 对应，tag = type）。 */
export type AudioSource = "microphone" | "system_loopback" | "imported_file" | "unknown";
export type SpeakerRole = "owner" | "client" | "other" | "unknown";
export interface SpeakerAttribution {
  source: AudioSource;
  role: SpeakerRole;
  voice?: { id: string; confidence?: number };
}

export type DomainEvent =
  | {
      type: "segment";
      speaker_id: number;
      speaker_label: string;
      speaker_attribution?: SpeakerAttribution;
      text: string;
      is_partial: boolean;
      ts_ms: number;
      duration_ms?: number;
      rms?: number;
    }
  | { type: "term"; result_id: string; status: "skeleton" | "final"; content: string }
  | { type: "translation"; result_id: string; status: "skeleton" | "final"; direction: "zh_en" | "en_zh"; content: string }
  | { type: "key_point"; result_id: string; status: "skeleton" | "final"; category: string; content: string }
  | { type: "brief"; source: string; text: string }
  | { type: "state"; topic: string; open_questions: string[]; recent_decisions: string[] }
  | { type: "status"; stage: string; message: string }
  | { type: "level"; mic_rms: number; loopback_rms: number }
  | {
      type: "session_stats";
      speaker_label: string;
      total_ms: number;
      speech_ms: number;
      final_segments: number;
      samples: number;
      avg_rms: number;
      max_rms: number;
      recording: string | null;
      vad_preset: string;
      vad_threshold: number;
      words?: number;
      questions?: number;
    }
  | { type: "metrics"; metrics: ConversationMetrics }
  | { type: "nudge"; nudge: NudgeEvent }
  | {
      type: "model_progress";
      engine: string;
      stage: "downloading" | "extracting" | "done" | "error";
      percent?: number;
      message?: string;
    }
  | {
      type: "snapshot";
      revision: number;
      committed: { speaker_id: number; speaker_label: string; speaker_attribution?: SpeakerAttribution; text: string; ts_ms: number; duration_ms?: number }[];
      hypothesis: { speaker_id: number; speaker_label: string; text: string; ts_ms: number }[];
      processed_until_sample?: number;
      committed_until_sample?: number;
      stage: string;
    };

/** 会话指标（会中实时；借鉴 Call.md conversation-metrics）。 */
export interface ConversationMetrics {
  talk_ratio_me: number;
  talk_ratio_them: number;
  pace_wpm: number;
  questions_me: number;
  monologue_detected: boolean;
  longest_monologue_ms: number;
  interruption_count: number;
  words_me: number;
  words_them: number;
  segment_count_me: number;
  segment_count_them: number;
  avg_segment_ms_me: number;
  avg_segment_ms_them: number;
  health_score: number;
  call_duration_ms: number;
}

/** 会中提示（借鉴 Call.md nudge-engine）。 */
export interface NudgeEvent {
  id: string;
  kind: "talk_ratio" | "questions" | "pace" | "next_steps";
  severity: "low" | "medium" | "high";
  message: string;
  action: "ask_question" | "confirm" | "pause" | "clarify" | null;
  timestamp_ms: number;
}

/** 三段式智能纪要（借鉴 Call.md summary-generator）。 */
export interface TrioSummary {
  short_overview: string;
  key_points: { topic: string; points: string[] }[];
  action_items: string[];
};

/** 会话概要（历史列表）。 */
export interface SessionRecord {
  id: number;
  started_at: number;
  ended_at: number | null;
  segment_count: number;
  term_count: number;
  /** 会话质量：clean / noise / silent / low（老数据为 undefined）。 */
  quality?: string;
  /** 会话时长（ms）。 */
  duration_ms?: number;
  /** 语音占比（0..1）。 */
  speech_ratio?: number;
}

/** 会话元数据（统计 + 质量评估）。 */
export interface StreamMeta {
  speaker_label: string;
  total_ms: number;
  speech_ms: number;
  final_segments: number;
  avg_rms: number;
  max_rms: number;
  recording: string | null;
  vad_preset: string;
  vad_threshold: number;
}

export interface SessionRuntimeInfo {
  app_version: string;
  scene_mode: string;
  user_engine: string;
  client_engine?: string | null;
  client_enabled: boolean;
  vad_preset: string;
  vad_threshold: number;
  vad_min_silence_ms?: number | null;
  denoise_enabled: boolean;
  min_segment_ms: number;
  input_gain_db: number;
  speaker_mode: string;
  sample_rate: number;
}

export interface SessionMeta {
  quality: string;
  skipped_analysis: boolean;
  duration_ms: number;
  speech_ms: number;
  speech_ratio: number;
  avg_rms: number;
  max_rms: number;
  text_noise: number;
  /** 默认回放文件：单流复用分轨，双流为左右声道完整录音。 */
  master_recording?: string | null;
  streams: StreamMeta[];
  evaluated_at: number;
  /** 运行环境快照（模型/场景/参数），老数据缺省。 */
  runtime_info?: SessionRuntimeInfo | null;
}

/** 搜索命中。 */
export interface SegmentHit {
  session_id: number;
  speaker_label: string;
  text: string;
  ts_ms: number;
}

/** 会话详情。 */
export interface SessionDetail {
  id: number;
  started_at: number;
  ended_at: number | null;
  segments: { speaker_id: number; speaker_label: string; speaker_attribution?: SpeakerAttribution; text: string; ts_ms: number; duration_ms?: number; rms?: number }[];
  terms: string[];
  translations: string[];
  notes: string | null;
  /** 三段式智能纪要（JSON 字符串；借鉴 Call.md）。 */
  trio: string | null;
  meta?: SessionMeta | null;
}

/** 纪要模板概要。 */
export interface NotesTemplate {
  id: string;
  name: string;
  description: string;
}

export interface AsrModelInfo {
  id: string;
  label: string;
  languages: string;
  streaming: boolean;
  speed: "realtime" | "balanced" | "accurate";
  description: string;
  installed: boolean;
  /** 已安装目录磁盘占用（MB）。 */
  size_mb?: number;
  /** 下载预估大小（MB）。 */
  download_size_mb?: number;
  /** 是否正在下载（残留 .part/.staging）。 */
  downloading?: boolean;
}

/** 应用 API 表面。 */
export interface AppApi {
  getVersion(): Promise<string>;
  getConfig(): Promise<AppConfig>;
  listAsrModels(): Promise<AsrModelInfo[]>;
  /** 下载/安装 ASR 引擎（进度经 model_progress 事件推送）。 */
  downloadModel(engine: string): Promise<void>;
  /** 删除 ASR 引擎模型目录。 */
  removeModel(engine: string): Promise<void>;
  /** 插件元数据（设置页据此生成插件表单）。 */
  listPlugins(): Promise<PluginMeta[]>;
  /** 按当前配置和宿主能力预检插件注册状态。 */
  listPluginStatus(): Promise<PluginStatusInfo[]>;
  /** 保存配置（写入 talksage.toml / 服务端配置）。 */
  saveConfig(updates: Record<string, unknown>): Promise<void>;
  ping(): Promise<void>;
  /** 开始实时监听（麦克风 → VAD → 本地 ASR → 事件推送）。 */
  startListen(): Promise<void>;
  /** 停止实时监听。 */
  stopListen(): Promise<void>;
  /** 暂停或继续当前监听，会话保持不变。 */
  setListenPaused(paused: boolean): Promise<void>;
  /** 实时调节噪音电平阈值（0 = 关闭，监听中生效，无需重启）。 */
  setNoiseLevel(level: number): Promise<void>;
  /** 说话人声纹状态。 */
  getVoiceprintStatus(): Promise<{ model_available: boolean; enrolled: boolean }>;
  /** 注册主人声音（录制麦克风 seconds 秒 → 提取声纹保存）。 */
  enrollVoice(seconds: number): Promise<{ ok: boolean; dim: number; voiced_ms: number; windows: number }>;
  /** 删除主人声纹。 */
  removeVoiceprint(): Promise<void>;
  /** 最小化到系统托盘（Windows；隐藏主窗口）。 */
  minimizeToTray(): Promise<void>;
  /** 退出应用（桌面；不依赖窗口关闭权限，最可靠）。 */
  quitApp(): Promise<void>;
  /** 历史：会话列表。 */
  listSessions(): Promise<SessionRecord[]>;
  /** 历史：全文检索。 */
  searchSessions(query: string): Promise<SegmentHit[]>;
  /** 历史：会话详情。 */
  getSession(id: number): Promise<SessionDetail>;
  /** 历史：删除会话（含段/术语/翻译）。 */
  deleteSession(id: number): Promise<void>;
  /** 纪要：模板列表。 */
  listNotesTemplates(): Promise<NotesTemplate[]>;
  /** 纪要：按模板生成并保存。 */
  generateNotes(sessionId: number, templateId: string): Promise<string>;
  /** 纪要：三段式智能纪要（概述 / 归属要点 / 行动项）生成并保存。 */
  generateTrioNotes(sessionId: number, meetingName?: string, meetingDescription?: string): Promise<TrioSummary>;
  /** 导出会话为 Markdown 单文件（转写 + 纪要 + 指标 + 质量；path 为桌面端落盘路径，headless 为空）。 */
  exportSessionMarkdown(sessionId: number): Promise<{ path: string; content: string }>;
  /** LLM 提炼核心要点（历史详情；需配置 LLM）。 */
  generateHighlights(sessionId: number): Promise<string[]>;
  /** 调试：读取最近日志（尾部 N 行）。 */
  readLogs(lines?: number): Promise<string>;
  /** 订阅领域事件流，返回取消函数。 */
  onEvent(handler: (ev: DomainEvent) => void): () => void;
  /** 传输载体标识（调试用）。 */
  readonly transport: "ipc" | "http";
}
