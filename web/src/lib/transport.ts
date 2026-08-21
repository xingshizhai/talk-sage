import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppApi, AppConfig, AsrModelInfo, DomainEvent, NotesTemplate, PluginMeta, SegmentHit, SessionDetail, SessionRecord, TrioSummary } from "./api";

/** 统一 fetch 辅助（同源 /api）。 */
async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const headers: Record<string, string> = { ...(init?.headers as Record<string, string> | undefined) };
  const token = getToken();
  if (token) headers["X-Talksage-Token"] = token;
  const r = await fetch(`/api${path}`, { ...init, headers });
  if (!r.ok) {
    const body = await r.text().catch(() => "");
    throw new Error(body || `HTTP ${r.status}`);
  }
  return r.json() as Promise<T>;
}

function getToken(): string {
  const boot = (window as unknown as { __TALKSAGE_BOOT__?: { token?: string } }).__TALKSAGE_BOOT__;
  if (boot?.token) return boot.token;
  try {
    const q = new URLSearchParams(location.search).get("token");
    if (q) {
      localStorage.setItem("talksage_token", q);
      return q;
    }
    return localStorage.getItem("talksage_token") ?? "";
  } catch {
    return "";
  }
}

/** Tauri IPC 适配器（桌面壳）。 */
export const ipcApi: AppApi = {
  transport: "ipc",

  async getVersion(): Promise<string> {
    return invoke<string>("get_version");
  },

  async getConfig(): Promise<AppConfig> {
    return invoke<AppConfig>("get_config");
  },

  async listAsrModels(): Promise<AsrModelInfo[]> {
    return invoke<AsrModelInfo[]>("list_asr_models");
  },

  async listPlugins(): Promise<PluginMeta[]> {
    return invoke<PluginMeta[]>("list_plugins");
  },

  async saveConfig(updates: Record<string, unknown>): Promise<void> {
    await invoke("save_config", { updates });
  },

  async ping(): Promise<void> {
    await invoke("ping");
  },

  async startListen(): Promise<void> {
    await invoke("start_listen");
  },

  async stopListen(): Promise<void> {
    await invoke("stop_listen");
  },

  async setListenPaused(paused: boolean): Promise<void> {
    await invoke("set_listen_paused", { paused });
  },

  async setNoiseLevel(level: number): Promise<void> {
    await invoke("set_noise_level", { level });
  },

  async getVoiceprintStatus(): Promise<{ model_available: boolean; enrolled: boolean }> {
    return invoke("get_voiceprint_status");
  },

  async enrollVoice(seconds: number): Promise<{ ok: boolean; dim: number; voiced_ms: number; windows: number }> {
    return invoke("enroll_voice", { seconds });
  },

  async removeVoiceprint(): Promise<void> {
    await invoke("remove_voiceprint");
  },

  async minimizeToTray(): Promise<void> {
    await invoke("minimize_to_tray");
  },

  async listSessions(): Promise<SessionRecord[]> {
    return invoke("list_sessions");
  },

  async searchSessions(query: string): Promise<SegmentHit[]> {
    return invoke("search_sessions", { query });
  },

  async getSession(id: number): Promise<SessionDetail> {
    return invoke("get_session", { sessionId: id });
  },

  async deleteSession(id: number): Promise<void> {
    await invoke("delete_session", { sessionId: id });
  },

  async listNotesTemplates(): Promise<NotesTemplate[]> {
    return invoke("list_notes_templates");
  },

  async generateNotes(sessionId: number, templateId: string): Promise<string> {
    return invoke("generate_notes", { sessionId, templateId });
  },

  async generateTrioNotes(sessionId: number, meetingName?: string, meetingDescription?: string): Promise<TrioSummary> {
    return invoke("generate_trio_notes", { sessionId, meetingName: meetingName ?? null, meetingDescription: meetingDescription ?? null });
  },

  async exportSessionMarkdown(sessionId: number): Promise<{ path: string; content: string }> {
    return invoke("export_session_markdown", { sessionId });
  },

  async generateHighlights(sessionId: number): Promise<string[]> {
    return invoke("generate_highlights", { sessionId });
  },

  async readLogs(lines?: number): Promise<string> {
    return invoke("read_logs", { lines });
  },

  onEvent(handler: (ev: DomainEvent) => void): () => void {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen<DomainEvent>("talksage://event", (e) => handler(e.payload))
      .then((fn) => {
        if (cancelled) {
          // 卸载发生在 listen 注册完成之前：立即取消，避免监听器残留（双收事件）
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((err) => console.error("failed to listen events:", err));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  },
};

/** headless 服务适配器（浏览器访问，M4）。 */
export const httpApi: AppApi = {
  transport: "http",

  async getVersion(): Promise<string> {
    const h = await req<{ version: string }>("/health");
    return h.version;
  },

  async getConfig(): Promise<AppConfig> {
    return req<AppConfig>("/config");
  },

  async listAsrModels(): Promise<AsrModelInfo[]> {
    return req<AsrModelInfo[]>("/asr/models");
  },

  async listPlugins(): Promise<PluginMeta[]> {
    return req<PluginMeta[]>("/plugins");
  },

  async saveConfig(updates: Record<string, unknown>): Promise<void> {
    await req("/config", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(updates),
    });
  },

  async ping(): Promise<void> {
    await req("/health");
  },

  async startListen(): Promise<void> {
    await req("/listen/start", { method: "POST" });
  },

  async stopListen(): Promise<void> {
    await req("/listen/stop", { method: "POST" });
  },

  async setListenPaused(paused: boolean): Promise<void> {
    await req("/listen/pause", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ paused }),
    });
  },

  async setNoiseLevel(level: number): Promise<void> {
    await req("/noise_level", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ level }),
    });
  },

  async getVoiceprintStatus(): Promise<{ model_available: boolean; enrolled: boolean }> {
    return req("/voiceprint/status");
  },

  async enrollVoice(seconds: number): Promise<{ ok: boolean; dim: number; voiced_ms: number; windows: number }> {
    const r = await req<{ ok: boolean; dim: number; voiced_ms: number; windows: number }>("/voiceprint/enroll", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ seconds }),
    });
    return r;
  },

  async removeVoiceprint(): Promise<void> {
    await req("/voiceprint/remove", { method: "POST" });
  },

  async minimizeToTray(): Promise<void> {
    // headless（浏览器）无桌面窗口，最小化到托盘无意义
  },

  async listSessions(): Promise<SessionRecord[]> {
    return req<SessionRecord[]>("/sessions");
  },

  async searchSessions(query: string): Promise<SegmentHit[]> {
    return req<SegmentHit[]>(`/search?q=${encodeURIComponent(query)}`);
  },

  async getSession(id: number): Promise<SessionDetail> {
    return req<SessionDetail>(`/session/${id}`);
  },

  async deleteSession(id: number): Promise<void> {
    await req(`/session/${id}`, { method: "DELETE" });
  },

  async listNotesTemplates(): Promise<NotesTemplate[]> {
    return req<NotesTemplate[]>("/templates");
  },

  async generateNotes(sessionId: number, templateId: string): Promise<string> {
    const r = await req<{ notes: string }>(`/session/${sessionId}/notes`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ template_id: templateId }),
    });
    return r.notes;
  },

  async generateTrioNotes(sessionId: number, meetingName?: string, meetingDescription?: string): Promise<TrioSummary> {
    return req<TrioSummary>(`/session/${sessionId}/trio-notes`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ meeting_name: meetingName ?? null, meeting_description: meetingDescription ?? null }),
    });
  },

  async exportSessionMarkdown(sessionId: number): Promise<{ path: string; content: string }> {
    const token = getToken();
    const headers: Record<string, string> = {};
    if (token) headers["X-Talksage-Token"] = token;
    const r = await fetch(`/api/session/${sessionId}/export`, { headers });
    if (!r.ok) throw new Error(`导出失败: HTTP ${r.status}`);
    return { path: "", content: await r.text() };
  },

  async generateHighlights(sessionId: number): Promise<string[]> {
    const r = await req<{ points: string[] }>(`/session/${sessionId}/highlights`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{}",
    });
    return r.points;
  },

  async readLogs(_lines?: number): Promise<string> {
    const r = await req<{ logs: string }>("/logs");
    return r.logs;
  },

  onEvent(handler: (ev: DomainEvent) => void): () => void {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.onmessage = (e) => {
      try {
        handler(JSON.parse(e.data) as DomainEvent);
      } catch {
        /* 忽略坏消息 */
      }
    };
    ws.onerror = () => console.error("ws error");
    return () => ws.close();
  },
};

/** 选择当前载体：Tauri 运行时 → IPC；浏览器（headless 服务）→ HTTP。 */
export function getApi(): AppApi {
  const isTauri = !!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  return isTauri ? ipcApi : httpApi;
}

/** 把录音路径转为可播放 URL。
 * - Tauri：本地文件经 asset 协议（需在 tauri.conf.json 配置 assetProtocol scope）
 * - headless：按文件名走服务端 `/api/recordings/<name>` 端点
 */
export function recordingUrl(recordingPath: string | null | undefined): string | null {
  if (!recordingPath) return null;
  const isTauri = !!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  if (isTauri) {
    return convertFileSrc(recordingPath);
  }
  const name = recordingPath.split(/[\\/]/).pop() ?? recordingPath;
  const token = getToken();
  const query = token ? `?token=${encodeURIComponent(token)}` : "";
  return `/api/recordings/${encodeURIComponent(name)}${query}`;
}
