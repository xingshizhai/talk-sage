import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppApi, AppConfig, DomainEvent } from "./api";

/** Tauri IPC 适配器（默认载体）。 */
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

  async listSessions(): Promise<import("./api").SessionRecord[]> {
    return invoke("list_sessions");
  },

  async searchSessions(query: string): Promise<import("./api").SegmentHit[]> {
    return invoke("search_sessions", { query });
  },

  async getSession(id: number): Promise<import("./api").SessionDetail> {
    return invoke("get_session", { sessionId: id });
  },

  async listNotesTemplates(): Promise<import("./api").NotesTemplate[]> {
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

/** 选择当前载体（M0 仅 IPC；M4 检测 headless 模式切换 HTTP）。 */
export function getApi(): AppApi {
  // headless 模式：服务注入全局标记后使用 HTTP 适配器（预留）
  const boot = (window as unknown as { __TALKSAGE_BOOT__?: { transport?: string } }).__TALKSAGE_BOOT__;
  if (boot?.transport === "http") {
    // TODO(M4): 返回 httpApi
  }
  return ipcApi;
}
