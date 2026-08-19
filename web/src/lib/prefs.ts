// 前端 UI 偏好持久化（localStorage）。
// 主题、转写视图模式等偏好，下次启动自动恢复。

import type { Theme } from "./theme";
import type { TranscriptMode } from "../components/TranscriptCard";

const KEY_THEME = "talksage_theme";
const KEY_MODE = "talksage_transcript_mode";
const KEY_NAV = "talksage_nav_page";
const KEY_ASIDE = "talksage_aside_collapsed";

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null; // 无 localStorage 环境（测试/隐私模式）
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 忽略写入失败
  }
}

/** 读取主题偏好（默认深色）。 */
export function loadTheme(): Theme {
  const v = read(KEY_THEME);
  return v === "light" ? "light" : "dark";
}

/** 保存主题偏好。 */
export function saveTheme(t: Theme): void {
  write(KEY_THEME, t);
}

/** 读取转写视图模式偏好（默认时间线）。 */
export function loadTranscriptMode(): TranscriptMode {
  const v = read(KEY_MODE);
  return v === "focus" || v === "dense" ? v : "timeline";
}

/** 保存转写视图模式偏好。 */
export function saveTranscriptMode(m: TranscriptMode): void {
  write(KEY_MODE, m);
}

/** 读取导航页偏好（默认实时转写）。 */
export function loadNavPage(): string {
  const v = read(KEY_NAV);
  return v === "history" || v === "settings" ? v : "transcript";
}

/** 保存导航页偏好。 */
export function saveNavPage(p: string): void {
  write(KEY_NAV, p);
}

/** 读取右栏折叠偏好（默认展开）。 */
export function loadAsideCollapsed(): boolean {
  return read(KEY_ASIDE) === "1";
}

/** 保存右栏折叠偏好。 */
export function saveAsideCollapsed(collapsed: boolean): void {
  write(KEY_ASIDE, collapsed ? "1" : "0");
}
