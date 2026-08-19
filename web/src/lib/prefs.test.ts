import { describe, expect, it, beforeEach } from "vitest";
import {
  loadTheme,
  saveTheme,
  loadTranscriptMode,
  saveTranscriptMode,
  loadNavPage,
  saveNavPage,
  loadAsideCollapsed,
  saveAsideCollapsed,
} from "./prefs";

// node 环境无 localStorage：验证容错默认值
describe("prefs without localStorage", () => {
  beforeEach(() => {
    (globalThis as Record<string, unknown>).localStorage = undefined;
  });

  it("loadTheme defaults to dark", () => {
    expect(loadTheme()).toBe("dark");
  });

  it("saveTheme does not throw", () => {
    expect(() => saveTheme("light")).not.toThrow();
  });

  it("loadTranscriptMode defaults to timeline", () => {
    expect(loadTranscriptMode()).toBe("timeline");
  });

  it("saveTranscriptMode does not throw", () => {
    expect(() => saveTranscriptMode("focus")).not.toThrow();
  });

  it("loadNavPage defaults to transcript", () => {
    expect(loadNavPage()).toBe("transcript");
  });

  it("saveNavPage does not throw", () => {
    expect(() => saveNavPage("history")).not.toThrow();
  });

  it("loadAsideCollapsed defaults to false", () => {
    expect(loadAsideCollapsed()).toBe(false);
  });

  it("saveAsideCollapsed does not throw", () => {
    expect(() => saveAsideCollapsed(true)).not.toThrow();
  });
});

describe("prefs with mocked localStorage", () => {
  let store: Record<string, string>;
  beforeEach(() => {
    store = {};
    (globalThis as Record<string, unknown>).localStorage = {
      getItem: (k: string) => store[k] ?? null,
      setItem: (k: string, v: string) => {
        store[k] = v;
      },
    };
  });

  it("persists and restores theme", () => {
    saveTheme("light");
    expect(loadTheme()).toBe("light");
  });

  it("persists and restores transcript mode", () => {
    saveTranscriptMode("dense");
    expect(loadTranscriptMode()).toBe("dense");
    saveTranscriptMode("focus");
    expect(loadTranscriptMode()).toBe("focus");
  });

  it("persists and restores nav page", () => {
    saveNavPage("history");
    expect(loadNavPage()).toBe("history");
    saveNavPage("settings");
    expect(loadNavPage()).toBe("settings");
    saveNavPage("transcript");
    expect(loadNavPage()).toBe("transcript");
  });

  it("persists and restores aside collapsed", () => {
    saveAsideCollapsed(true);
    expect(loadAsideCollapsed()).toBe(true);
    saveAsideCollapsed(false);
    expect(loadAsideCollapsed()).toBe(false);
  });

  it("ignores invalid stored values", () => {
    store["talksage_theme"] = "neon";
    expect(loadTheme()).toBe("dark");
    store["talksage_transcript_mode"] = "weird";
    expect(loadTranscriptMode()).toBe("timeline");
    store["talksage_nav_page"] = "bogus";
    expect(loadNavPage()).toBe("transcript");
    store["talksage_aside_collapsed"] = "maybe";
    expect(loadAsideCollapsed()).toBe(false);
  });
});
