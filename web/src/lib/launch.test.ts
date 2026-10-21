import { describe, expect, it } from "vitest";
import {
  defaultSceneChoice,
  enabledPlugins,
  launchSummary,
  offeredPlugins,
  resolveSceneChoice,
  sessionPlugins,
  tasksEnabled,
  toggleFeature,
} from "./launch";

describe("listen launch", () => {
  it("多人会议默认可开要点和专业知识，并按人汇总，不带翻译", () => {
    const choice = defaultSceneChoice("meeting");
    const offered = offeredPlugins("meeting");
    expect(offered).toEqual(["key_point_llm", "term_explainer"]);
    expect(enabledPlugins(choice, offered)).toEqual(["key_point_llm", "term_explainer"]);
    expect(tasksEnabled(choice, offered)).toBe(true);
    expect(sessionPlugins(choice, offered)).toEqual(["key_point_llm", "term_explainer"]);
  });

  it("听写只有要点，默认不按人汇总", () => {
    const offered = offeredPlugins("dictation");
    const choice = defaultSceneChoice("dictation");
    expect(offered).toEqual(["key_point_llm"]);
    expect(tasksEnabled(choice, offered)).toBe(false);
  });

  it("实时翻译只提供翻译，双语对话才同时有要点和翻译", () => {
    expect(offeredPlugins("live_translation")).toEqual(["translator"]);
    expect(offeredPlugins("bilingual")).toEqual(["key_point_llm", "term_explainer", "translator"]);
  });

  it("自定义场景在翻译关闭时不提供翻译开关", () => {
    const custom = { plugin_allowlist: ["translator", "key_point_llm"], translation_mode: "off" as const };
    expect(offeredPlugins("custom", custom)).toEqual(["key_point_llm"]);
    expect(offeredPlugins("custom", { ...custom, translation_mode: "bidirectional" })).toEqual([
      "key_point_llm",
      "translator",
    ]);
  });

  it("关掉要点后按人汇总跟着关闭", () => {
    const offered = offeredPlugins("meeting");
    const choice = toggleFeature(defaultSceneChoice("meeting"), "key_point_llm", offered);
    expect(enabledPlugins(choice, offered)).not.toContain("key_point_llm");
    expect(tasksEnabled(choice, offered)).toBe(false);
    expect(toggleFeature(choice, "tasks", offered).tasks).toBe(false);
  });

  it("全局关掉的插件默认不出现在本次监听", () => {
    const choice = defaultSceneChoice("meeting", { pluginEnabled: { term_explainer: false } });
    expect(enabledPlugins(choice, offeredPlugins("meeting"))).toEqual(["key_point_llm"]);
  });

  it("记住的功能开关按当前场景能开的项收口", () => {
    const choice = resolveSceneChoice("live_translation", { disabled: ["key_point_llm"], tasks: true });
    expect(enabledPlugins(choice, offeredPlugins("live_translation"))).toEqual(["translator"]);
    expect(tasksEnabled(choice, offeredPlugins("live_translation"))).toBe(false);
  });

  it("摘要只写本次功能", () => {
    expect(launchSummary(defaultSceneChoice("meeting"), offeredPlugins("meeting"))).toBe("要点、专业知识、按人汇总");
    expect(launchSummary(defaultSceneChoice("dictation"), offeredPlugins("dictation"))).toBe("要点");
  });
});