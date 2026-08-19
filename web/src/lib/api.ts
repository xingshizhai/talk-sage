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
  plugins: {
    term_explainer: { enabled: boolean; cooldown_seconds: number };
    translator: { enabled: boolean; cooldown_seconds: number };
    brief_retriever: { enabled: boolean; cooldown_seconds: number };
  };
  audio: {
    vad: { preset: "standard" | "sensitive" | "strict"; threshold: number | null };
    denoise: { enabled: boolean; gate_threshold: number; highpass: boolean };
  };
  [key: string]: unknown;
}

/** 领域事件（与 Rust 侧 DomainEvent 对应，tag = type）。 */
export type DomainEvent =
  | { type: "segment"; speaker_id: number; speaker_label: string; text: string; is_partial: boolean; ts_ms: number }
  | { type: "term"; result_id: string; status: "skeleton" | "final"; content: string }
  | { type: "translation"; result_id: string; status: "skeleton" | "final"; direction: "zh_en" | "en_zh"; content: string }
  | { type: "key_point"; result_id: string; status: "skeleton" | "final"; category: string; content: string }
  | { type: "brief"; source: string; text: string }
  | { type: "state"; topic: string; open_questions: string[]; recent_decisions: string[] }
  | { type: "status"; stage: string; message: string }
  | { type: "level"; mic_rms: number; loopback_rms: number };

/** 会话概要（历史列表）。 */
export interface SessionRecord {
  id: number;
  started_at: number;
  ended_at: number | null;
  segment_count: number;
  term_count: number;
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
  segments: { speaker_id: number; speaker_label: string; text: string; ts_ms: number }[];
  terms: string[];
  translations: string[];
  notes: string | null;
}

/** 纪要模板概要。 */
export interface NotesTemplate {
  id: string;
  name: string;
  description: string;
}

/** 应用 API 表面。 */
export interface AppApi {
  getVersion(): Promise<string>;
  getConfig(): Promise<AppConfig>;
  /** 保存配置（写入 talksage.toml / 服务端配置）。 */
  saveConfig(updates: Record<string, unknown>): Promise<void>;
  ping(): Promise<void>;
  /** 开始实时监听（麦克风 → VAD → 本地 ASR → 事件推送）。 */
  startListen(): Promise<void>;
  /** 停止实时监听。 */
  stopListen(): Promise<void>;
  /** 历史：会话列表。 */
  listSessions(): Promise<SessionRecord[]>;
  /** 历史：全文检索。 */
  searchSessions(query: string): Promise<SegmentHit[]>;
  /** 历史：会话详情。 */
  getSession(id: number): Promise<SessionDetail>;
  /** 纪要：模板列表。 */
  listNotesTemplates(): Promise<NotesTemplate[]>;
  /** 纪要：按模板生成并保存。 */
  generateNotes(sessionId: number, templateId: string): Promise<string>;
  /** 调试：读取最近日志（尾部 N 行）。 */
  readLogs(lines?: number): Promise<string>;
  /** 订阅领域事件流，返回取消函数。 */
  onEvent(handler: (ev: DomainEvent) => void): () => void;
  /** 传输载体标识（调试用）。 */
  readonly transport: "ipc" | "http";
}
