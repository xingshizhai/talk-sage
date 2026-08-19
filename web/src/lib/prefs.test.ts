import { describe, expect, it, beforeEach } from "vitest";
import { loadTheme, saveTheme, loadTranscriptMode, saveTranscriptMode } from "./prefs";

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

  it("ignores invalid stored values", () => {
    store["talksage_theme"] = "neon";
    expect(loadTheme()).toBe("dark");
    store["talksage_transcript_mode"] = "weird";
    expect(loadTranscriptMode()).toBe("timeline");
  });
});
