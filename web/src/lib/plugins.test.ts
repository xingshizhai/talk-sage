import { describe, expect, it } from "vitest";
import type { PluginMeta } from "./api";
import {
  analysisPluginIds,
  buildPluginUpdates,
  fieldKind,
  initialPluginValues,
  pluginFields,
} from "./plugins";

/** 三个插件覆盖三种控件类型 + 一个渲染不了的键。 */
const METAS: PluginMeta[] = [
  {
    id: "short_segment",
    label: "短段抑制",
    analysis: false,
    schema: { enabled: true, min_ms: 0 },
    host_managed: ["min_ms"],
  },
  {
    id: "term_explainer",
    label: "术语解释",
    analysis: true,
    schema: { cooldown_seconds: 10, enabled: true },
    host_managed: [],
  },
  {
    id: "webhook",
    label: "会议结束推送",
    analysis: false,
    // urls 是数组：设置页渲染不了，必须被跳过而不是崩
    schema: { enabled: false, note: "", urls: [] },
    host_managed: [],
  },
];

describe("fieldKind", () => {
  it("按默认值的 JSON 类型判定控件", () => {
    expect(fieldKind(true)).toBe("bool");
    expect(fieldKind(3.5)).toBe("number");
    expect(fieldKind("hi")).toBe("string");
  });

  it("渲染不了的类型返回 null", () => {
    expect(fieldKind([])).toBeNull();
    expect(fieldKind({})).toBeNull();
    expect(fieldKind(null)).toBeNull();
    expect(fieldKind(undefined)).toBeNull();
    expect(fieldKind(NaN)).toBeNull();
  });
});

describe("pluginFields", () => {
  it("enabled 永远排第一（约定键，用户最常动）", () => {
    expect(pluginFields(METAS[1]).map((f) => f.key)).toEqual(["enabled", "cooldown_seconds"]);
  });

  it("跳过渲染不了的键，保留其余", () => {
    expect(pluginFields(METAS[2]).map((f) => f.key)).toEqual(["enabled", "note"]);
  });

  it("空 schema 得到空表单而非异常", () => {
    expect(pluginFields({ id: "x", label: "X", analysis: false, schema: {}, host_managed: [] })).toEqual([]);
  });

  it("标出宿主裁决的键（设置页据此置灰）", () => {
    const fields = pluginFields(METAS[0]);
    expect(fields.find((f) => f.key === "min_ms")?.hostManaged).toBe(true);
    expect(fields.find((f) => f.key === "enabled")?.hostManaged).toBe(false);
  });

  it("后端没给 host_managed 时当作空（不炸）", () => {
    const meta = { id: "x", label: "X", analysis: false, schema: { enabled: true } } as unknown as PluginMeta;
    expect(pluginFields(meta)[0].hostManaged).toBe(false);
  });
});

describe("initialPluginValues", () => {
  it("以插件默认为底，用生效配置覆盖", () => {
    const values = initialPluginValues(METAS, {
      short_segment: { enabled: false, min_ms: 400 },
      term_explainer: { cooldown_seconds: 25 },
    });
    expect(values.short_segment).toEqual({ enabled: false, min_ms: 400 });
    // 配置里没写 enabled → 取插件默认 true
    expect(values.term_explainer).toEqual({ enabled: true, cooldown_seconds: 25 });
    // 配置里完全没有的插件 → 全用默认
    expect(values.webhook).toEqual({ enabled: false, note: "" });
  });

  it("配置缺失时每个插件都有可渲染的值（受控 input 不能从 undefined 起步）", () => {
    const values = initialPluginValues(METAS, undefined);
    expect(Object.keys(values)).toEqual(["short_segment", "term_explainer", "webhook"]);
    expect(values.short_segment.enabled).toBe(true);
  });

  it("类型不匹配的配置值按默认处理（手写 toml 把 enabled 写成了字符串）", () => {
    const values = initialPluginValues(METAS, { short_segment: { enabled: "yes", min_ms: "400" } });
    expect(values.short_segment).toEqual({ enabled: true, min_ms: 0 });
  });
});

describe("buildPluginUpdates", () => {
  it("产出 { 插件 id: { 键: 值 } } 的提交载荷", () => {
    const updates = buildPluginUpdates(METAS, {
      short_segment: { enabled: true, min_ms: 600 },
      term_explainer: { enabled: false, cooldown_seconds: 10 },
      webhook: { enabled: true, note: "prod" },
    });
    expect(updates).toEqual({
      short_segment: { enabled: true, min_ms: 600 },
      term_explainer: { enabled: false, cooldown_seconds: 10 },
      webhook: { enabled: true, note: "prod" },
    });
  });

  it("三个分析插件的开关能真正关掉（阶段 5 之前的硬编码行为不能丢）", () => {
    const metas: PluginMeta[] = [
      { id: "term_explainer", label: "术语解释", analysis: true, schema: { enabled: true }, host_managed: [] },
      { id: "translator", label: "实时翻译", analysis: true, schema: { enabled: true }, host_managed: [] },
      { id: "brief_retriever", label: "简报检索", analysis: true, schema: { enabled: true }, host_managed: [] },
    ];
    const updates = buildPluginUpdates(metas, {
      term_explainer: { enabled: false },
      translator: { enabled: false },
      brief_retriever: { enabled: false },
    });
    expect(updates).toEqual({
      term_explainer: { enabled: false },
      translator: { enabled: false },
      brief_retriever: { enabled: false },
    });
  });

  it("缺失的值补插件默认，不提交 undefined（会序列化成 null 覆盖好值）", () => {
    const updates = buildPluginUpdates(METAS, { short_segment: { enabled: false } });
    expect(updates.short_segment).toEqual({ enabled: false, min_ms: 0 });
    expect(updates.term_explainer).toEqual({ enabled: true, cooldown_seconds: 10 });
  });

  it("丢弃元数据里没有的 id 与键（旧 state 不该写死键进配置文件）", () => {
    const updates = buildPluginUpdates(METAS, {
      short_segment: { enabled: true, min_ms: 100, ghost_key: 1 },
      removed_plugin: { enabled: true },
    });
    expect(updates.short_segment).toEqual({ enabled: true, min_ms: 100 });
    expect(updates).not.toHaveProperty("removed_plugin");
  });

  it("渲染不了的键不出现在载荷里（后端逐键合并，未提交的键原样保留）", () => {
    const updates = buildPluginUpdates(METAS, { webhook: { enabled: true, note: "x" } });
    expect(updates.webhook).not.toHaveProperty("urls");
  });

  it("初值原样回提 = 配置不变（打开设置页直接点保存不应改动任何值）", () => {
    const config = { short_segment: { enabled: false, min_ms: 400 }, webhook: { enabled: true, note: "n" } };
    const values = initialPluginValues(METAS, config);
    const updates = buildPluginUpdates(METAS, values);
    expect(updates.short_segment).toEqual(config.short_segment);
    expect(updates.webhook).toEqual(config.webhook);
  });
});

describe("analysisPluginIds", () => {
  it("只取 analysis 标记为真的插件，顺序照元数据", () => {
    expect(analysisPluginIds(METAS)).toEqual(["term_explainer"]);
  });

  it("元数据为空时返回空列表", () => {
    expect(analysisPluginIds([])).toEqual([]);
  });
});
