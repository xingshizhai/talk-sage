import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppApi, AppConfig, DomainEvent, NotesTemplate, SegmentHit, SessionDetail, SessionRecord } from "./api";

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

  async ping(): Promise<void> {
    await invoke("ping");
  },

  async startListen(): Promise<void> {
    await invoke("start_listen");
  },

  async stopListen(): Promise<void> {
    await invoke("stop_listen");
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

  async listNotesTemplates(): Promise<NotesTemplate[]> {
    return invoke("list_notes_templates");
  },

  async generateNotes(sessionId: number, templateId: string): Promise<string> {
    return invoke("generate_notes", { sessionId, templateId });
  },

  onEvent(handler: (ev: DomainEvent) => void): () => void {
    let unlisten: (() => void) | undefined;
    listen<DomainEvent>("talksage://event", (e) => handler(e.payload))
      .then((fn) => {
        unlisten = fn;
      })
      .catch((err) => console.error("failed to listen events:", err));
    return () => unlisten?.();
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

  async ping(): Promise<void> {
    await req("/health");
  },

  async startListen(): Promise<void> {
    await req("/listen/start", { method: "POST" });
  },

  async stopListen(): Promise<void> {
    await req("/listen/stop", { method: "POST" });
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
