// 统一 API 抽象：核心域与传输层解耦。
//
// Tauri 模式 → IpcApi（invoke + 事件监听）
// headless 模式（M4 预留）→ HttpApi（fetch + WebSocket），实现同一接口。
//
// 界面代码只依赖本接口，不感知载体。

/**
 * 应用配置快照（与 Rust 侧 Config 对应，getConfig 返回）。
 *
 * 桌面端与浏览器端形状相同，由 Rust 侧
 * talksage_config::ui_config_json 统一组装。
 *
 * 密钥字段（llm.providers[].api_key / asr.aliyun_access_key_secret /
 * server.token）在浏览器模式（transport = http）下是**掩码**，形如
 * `sk-••••••••cdef`：够判断「已配置」，但不是真值。掩码原样提交回
 * saveConfig / 检查接口都视作「未修改」，所以设置页照常读写即可；
 * 想改就直接覆盖输入框，想清空就留空。
 */
export interface AppConfig {
  asr: {
    engine_zh: string;
    engine_en: string;
    backend: string;
    punct_enabled: boolean;
    aliyun_access_key_id: string;
    aliyun_access_key_secret: string;
    aliyun_app_key: string;
    asr_mode: string;
    language_mode: string;
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
    audio_source: "mic" | "loopback";
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
  /** 网络代理（仅对模型下载 / LLM API / Webhook 生效；阿里云 ASR 始终直连）。 */
  network: {
    proxy: string;
  };
  /** 场景模式是一组完整运行预设。 */
  scene: {
    mode: SceneMode;
    custom: SceneParams;
  };
  /** 本地知识库（Obsidian 仓库或普通 .md/.txt 文件夹）。 */
  knowledge_base: {
    enabled: boolean;
    folder: string;
  };
  [key: string]: unknown;
}

export type SceneMode =
  | "dictation"
  | "conversation"
  | "bilingual"
  | "live_translation"
  | "meeting"
  | "lecture"
  | "custom";

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
  category?: "infrastructure" | "analysis" | "knowledge_source";
  phase?: "filter" | "observer" | "finalizer" | "source";
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
  /** 段级 ASR 最长上下文（ms；0 = 不主动切分）。 */
  asr_segment_ms: number;
  /** 自定义模式：用户流引擎（其他模式由 engine_zh/engine_en 决定）。 */
  user_engine: string;
  client_enabled: boolean;
  /** 自定义模式：客户流引擎。 */
  client_engine: string;
  /** 本场景主语言："zh" | "en"。双语中为「我的语言」，实时翻译中为「输入语言」。 */
  language: "zh" | "en";
  /** 对方语言（双语）或翻译目标（实时翻译）。 */
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
  | { type: "media_progress"; position_ms: number; total_ms: number; speed: number }
  | { type: "media_completed"; session_id?: number | null; cancelled: boolean; error?: string }
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
  | { type: "key_point"; result_id: string; status: "skeleton" | "final"; category: string; content: string; ts_ms?: number; manual?: boolean; owner?: string; due_date?: string; source_refs?: number[] }
  | { type: "brief"; source: string; text: string }
  | {
      type: "knowledge_query";
      query_id: string;
      trigger: "key_point" | "term" | "question" | "manual";
      query: string;
      scope: "pinned" | "all" | "pinned_then_all";
      hits: { hit_id: string; path: string; heading: string; excerpt: string; score: number; pinned: boolean }[];
    }
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
  | {
      /** AI 助手回答增量：按 message_id 把 delta 拼到同一条消息上。 */
      type: "chat_delta";
      thread_id: number;
      message_id: number;
      delta?: string;
      done: boolean;
      error?: string;
    }
  | {
      /** 手动「立即整理」的结果回执（整理在后台跑，命令先于结果返回）。 */
      type: "key_point_flush";
      added: number;
      message: string;
    }
  | { type: "metrics"; metrics: ConversationMetrics }
  | { type: "nudge"; nudge: NudgeEvent }
  | {
      type: "model_progress";
      engine: string;
      stage: "downloading" | "extracting" | "done" | "cancelled" | "error";
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
/** AI 助手话题（左侧列表）。 */
export interface ChatThread {
  id: number;
  /** 话题名；null = 尚未命名（首条提问后自动生成）。 */
  title: string | null;
  created_at: number;
  updated_at: number;
  message_count: number;
}

/** AI 助手的一条消息。 */
export interface ChatMessageRecord {
  id: number;
  /** user | assistant */
  role: string;
  content: string;
  ts_ms: number;
}

/** 提交提问后立刻拿到的两条消息 id；回答正文随后由 ChatDelta 事件补齐。 */
export interface ChatSendResult {
  user_message_id: number;
  assistant_message_id: number;
}

export interface SessionRecord {
  id: number;
  started_at: number;
  ended_at: number | null;
  /** 用户自定义会话名（null/未命名时界面回退到 "#id · 时间"）。 */
  title?: string | null;
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

/** 会中落库的要点（插件 `key_point_extractor` 产出）。 */
export interface SessionKeyPoint {
  result_id: string;
  category: string;
  content: string;
  ts_ms: number;
  owner?: string | null;
  due_date?: string | null;
  source_refs?: number[];
}

/** 历史详情中的转写段：比实时事件段多一个数据库 id，用于编辑/删除。 */
export interface SessionSegment {
  /** 数据库段 id；实时事件流里的段没有该字段。 */
  id?: number;
  speaker_id: number;
  speaker_label: string;
  speaker_attribution?: SpeakerAttribution;
  text: string;
  ts_ms: number;
  duration_ms?: number;
  rms?: number;
}

/** 会话详情。 */
export interface SessionDetail {
  id: number;
  started_at: number;
  ended_at: number | null;
  /** 用户自定义会话名（null = 未命名）。 */
  title?: string | null;
  segments: SessionSegment[];
  terms: string[];
  translations: string[];
  key_points: SessionKeyPoint[];
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
  /** 已接入可运行引擎；false 表示目前仅可预下载。 */
  selectable?: boolean;
  installed: boolean;
  /** 已安装目录磁盘占用（MB）。 */
  size_mb?: number;
  /** 下载预估大小（MB）。 */
  download_size_mb?: number;
  /** 是否正在下载（残留 .part/.staging）。 */
  downloading?: boolean;
}

export interface AsrRuntimeStatus {
  backend: string;
  display_name: string;
  hardware_candidate?: string;
  availability_note?: string;
  is_accelerated: boolean;
  effective_route?: string | null;
  route_error?: string | null;
}

/** 在线检查更新结果（框架：只检查并返回信息，不自动安装）。 */
export interface UpdateCheckResult {
  available: boolean;
  /** 更新源是否已配置（公钥/端点）；false = 在线升级尚未启用。 */
  configured: boolean;
  current_version: string;
  /** 仅在 available 为 true 时出现（见 src-tauri/src/updater.rs）。 */
  latest_version?: string;
  message: string;
}

/** 离线升级结果。 */
export interface OfflineUpgradeResult {
  ok: boolean;
  version: string;
  message: string;
}

/** 应用 API 表面。 */
export interface AppApi {
  getVersion(): Promise<string>;
  getConfig(): Promise<AppConfig>;
  getAsrRuntimeStatus(): Promise<AsrRuntimeStatus>;
  listAsrModels(): Promise<AsrModelInfo[]>;
  /** 下载/安装 ASR 引擎（进度经 model_progress 事件推送）。 */
  downloadModel(engine: string): Promise<void>;
  /** 取消正在进行的模型下载。 */
  cancelModelDownload(engine: string): Promise<void>;
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
  startListen(pinnedNotePaths?: string[]): Promise<void>;
  /** 知识库文档列表（材料包挑选）。 */
  listKnowledgeDocuments(): Promise<{ path: string; title: string; text: string }[]>;
  /** 停止实时监听。 */
  stopListen(): Promise<void>;
  /** 暂停或继续当前监听，会话保持不变。 */
  setListenPaused(paused: boolean): Promise<void>;
  /** 实时调节噪音电平阈值（0 = 关闭，监听中生效，无需重启）。 */
  setNoiseLevel(level: number): Promise<void>;
  /** 手动触发要点聚合：立即处理当前积累的转写段，返回诊断消息。 */
  flushKeyPoints(): Promise<string>;
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
  /** 专业术语：手动查询一个词（用户点名要问的，不做专业度筛选）。 */
  explainTerm(term: string): Promise<string>;
  /** AI 助手：话题列表（最近活跃在前）。 */
  listChatThreads(): Promise<ChatThread[]>;
  /** AI 助手：新建话题，返回 id。 */
  createChatThread(): Promise<number>;
  /** AI 助手：话题内全部消息。 */
  getChatMessages(threadId: number): Promise<ChatMessageRecord[]>;
  /** AI 助手：重命名话题；空串 = 清除自定义名。 */
  renameChatThread(threadId: number, title: string): Promise<void>;
  /** AI 助手：删除话题及其消息。 */
  deleteChatThread(threadId: number): Promise<void>;
  /** AI 助手：提交提问；回答经 ChatDelta 事件流式返回。 */
  sendChatMessage(threadId: number, text: string): Promise<ChatSendResult>;
  /** AI 助手：停止正在生成的回答。 */
  cancelChatMessage(messageId: number): Promise<void>;
  /** 历史：重命名会话；传空串 = 清除自定义名。 */
  renameSession(id: number, title: string): Promise<void>;
  /** 历史：编辑某条转写段文本；段改完后纪要/智能纪要/会中要点会被清除，需重新整理。 */
  updateSegment(sessionId: number, segmentId: number, text: string): Promise<void>;
  /** 历史：删除某条转写段；同样清除派生的纪要/要点。 */
  deleteSegment(sessionId: number, segmentId: number): Promise<void>;
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
  /** 导出会话为纯文本转写（无 Markdown 标记；path 为桌面端落盘路径，headless 为空）。 */
  exportSessionText(sessionId: number): Promise<{ path: string; content: string }>;
  /** 导出会话完整录音（桌面端复制到 exports/ 返回落盘路径；headless 触发浏览器下载）。 */
  exportSessionAudio(sessionId: number): Promise<string>;
  /** 整理会中要点（历史详情；需配置 LLM）。 */
  generateHighlights(sessionId: number): Promise<string[]>;
  /** 验证 LLM 连接（设置页「检查」按钮）：用表单当前值（可未保存）发最小请求。 */
  testLlm(opts: { provider: string; baseUrl?: string; model?: string; apiKey?: string }): Promise<void>;
  /** 验证阿里云 ASR 凭据（设置页「检查」按钮）：请求 NLS AccessToken。返回有效期秒数。 */
  testAliyunAsr(opts: { accessKeyId?: string; accessKeySecret?: string; appKey?: string }): Promise<{ ok: boolean; expire_at: number; valid_for_secs: number; app_key: string }>;
  /** 验证代理可达性（设置页「测试」按钮）：通过配置的代理发送请求到 google.com。 */
  testProxy(proxyUrl: string): Promise<string>;
  /** 调试：读取最近日志（尾部 N 行）。 */
  readLogs(lines?: number): Promise<string>;
  /** 订阅领域事件流，返回取消函数。 */
  onEvent(handler: (ev: DomainEvent) => void): () => void;
  /** 打开系统文件对话框，选择一个 WAV 录音文件；用户取消时返回 null。 */
  pickAudioFile(): Promise<string | null>;
  /** 打开系统文件夹对话框，选择知识库 / Obsidian 仓库目录；取消时返回 null。 */
  pickFolder(): Promise<string | null>;
  /** 启动本地媒体会话，立即返回 session_id；事件通过统一 onEvent 推送。 */
  startFileImport(path: string): Promise<number>;
  /** 调整文件会话处理速度；0 表示极速。 */
  setFilePlaybackSpeed(speed: number): Promise<void>;
  /** 在线检查更新（框架；未配置公钥时 configured=false）。 */
  checkForUpdates(): Promise<UpdateCheckResult>;
  /** 选择离线升级包；取消时返回 null。仅桌面。Windows：NSIS/MSI；macOS：.dmg/.app。 */
  pickUpgradePackage(): Promise<string | null>;
  /** 校验并安装离线升级包，成功后应用会退出。仅桌面。 */
  installOfflineUpgrade(path: string): Promise<OfflineUpgradeResult>;
  /** 传输载体标识（调试用）。 */
  readonly transport: "ipc" | "http";
}
