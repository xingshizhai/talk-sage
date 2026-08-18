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

/** 应用 API 表面。 */
export interface AppApi {
  getVersion(): Promise<string>;
  getConfig(): Promise<AppConfig>;
  ping(): Promise<void>;
  /** 开始实时监听（麦克风 → VAD → 本地 ASR → 事件推送）。 */
  startListen(): Promise<void>;
  /** 停止实时监听。 */
  stopListen(): Promise<void>;
  /** 订阅领域事件流，返回取消函数。 */
  onEvent(handler: (ev: DomainEvent) => void): () => void;
  /** 传输载体标识（调试用）。 */
  readonly transport: "ipc" | "http";
}
