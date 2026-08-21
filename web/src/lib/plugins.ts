// 插件设置表单的纯逻辑：元数据 → 控件描述 → 提交载荷。
//
// 设置页不再认识具体插件：控件由 `/plugins` 返回的元数据生成，键名与默认值
// 都来自 Rust 侧 `plugin_metadata()`。本文件只做数据变换，不碰 React ——
// 前端测试只覆盖 lib 层纯函数，把逻辑放在这里才测得到（组件渲染不测）。

import type { PluginMeta } from "./api";

/** 一个可编辑字段。`kind` 决定设置页渲染什么控件。 */
export interface PluginField {
  key: string;
  kind: "bool" | "number" | "string";
  /** 插件默认值（用于「恢复默认」语义与类型判定）。 */
  default: boolean | number | string;
  /**
   * 由宿主裁决：装配时被场景参数覆盖，改了也不生效。设置页置灰它。
   * 仍然渲染而不是藏起来 —— 用户该看见这个键存在，以及为什么动不了。
   */
  hostManaged: boolean;
}

/** 表单值：`{ 插件 id: { 配置键: 值 } }`。与提交载荷的 `plugins` 同形。 */
export type PluginValues = Record<string, Record<string, boolean | number | string>>;

/**
 * 默认值的 JSON 类型即控件类型。**没有独立的 schema 语言** —— 见 Rust 侧
 * `plugin_metadata()` 的注释。
 *
 * 认不出的类型（数组 / 对象 / null）返回 null：设置页跳过该键，而不是拿
 * 一个渲染不了的控件去糊。这样的键仍然保留在配置文件里（后端逐键合并，
 * 未提交的键不动），只是不能在设置页改。
 */
export function fieldKind(value: unknown): PluginField["kind"] | null {
  if (typeof value === "boolean") return "bool";
  if (typeof value === "number" && Number.isFinite(value)) return "number";
  if (typeof value === "string") return "string";
  return null;
}

/**
 * 一个插件的可编辑字段列表。`enabled` 永远排第一 —— 它是所有插件的约定键，
 * 也是用户最常动的开关，不该混在字母序里。
 */
export function pluginFields(meta: PluginMeta): PluginField[] {
  const fields: PluginField[] = [];
  for (const [key, value] of Object.entries(meta.schema ?? {})) {
    const kind = fieldKind(value);
    if (!kind) continue;
    fields.push({
      key,
      kind,
      default: value as boolean | number | string,
      hostManaged: (meta.host_managed ?? []).includes(key),
    });
  }
  fields.sort((a, b) => (a.key === "enabled" ? -1 : b.key === "enabled" ? 1 : 0));
  return fields;
}

/**
 * 表单初值：以插件默认配置为底，用当前生效配置覆盖。
 *
 * 后端 `/config` 返回的已经是「默认 + 用户覆盖」的生效配置，所以正常情况下
 * 每个键都能取到值；这里仍然回落到 schema 默认，是为了让设置页在
 * `/config` 尚未加载完 / 该插件是新增的情况下也有可渲染的值 ——
 * 否则受控 input 会从 undefined 起步，React 会把它当非受控组件。
 *
 * 类型不匹配的配置值（例如手写 toml 把 `enabled` 写成了字符串）按默认值处理：
 * 设置页不是校验器，但也不该把一个 bool 开关绑到字符串上。
 */
export function initialPluginValues(
  metas: PluginMeta[],
  config: Record<string, Record<string, unknown> | undefined> | undefined,
): PluginValues {
  const out: PluginValues = {};
  for (const meta of metas) {
    const current = config?.[meta.id];
    const values: Record<string, boolean | number | string> = {};
    for (const field of pluginFields(meta)) {
      const v = current?.[field.key];
      values[field.key] = fieldKind(v) === field.kind ? (v as boolean | number | string) : field.default;
    }
    out[meta.id] = values;
  }
  return out;
}

/**
 * 提交载荷的 `plugins` 段：`{ <插件 id>: { <键>: <值> } }`。
 *
 * 只提交元数据里声明过的键 —— 表单值里若混进了不认识的 id/键（例如元数据
 * 刷新后旧 state 还在），一律丢弃，避免往配置文件里写死键。
 *
 * 缺失的值补插件默认：宁可显式写一遍默认值，也不要提交 undefined ——
 * 后端逐键合并，undefined 序列化后会变成 null 覆盖掉好值。
 */
export function buildPluginUpdates(metas: PluginMeta[], values: PluginValues): PluginValues {
  const out: PluginValues = {};
  for (const meta of metas) {
    const entry: Record<string, boolean | number | string> = {};
    for (const field of pluginFields(meta)) {
      const v = values[meta.id]?.[field.key];
      entry[field.key] = fieldKind(v) === field.kind ? (v as boolean | number | string) : field.default;
    }
    out[meta.id] = entry;
  }
  return out;
}

/**
 * 受场景 allowlist 约束的插件 id（Rust 侧 `ANALYSIS_PLUGIN_IDS` 的投影）。
 * 场景自定义面板据此渲染勾选框，前端因此不必自己维护一份 id 列表。
 */
export function analysisPluginIds(metas: PluginMeta[]): string[] {
  return metas.filter((m) => m.analysis).map((m) => m.id);
}

/** 把插件配置键渲染成人话标签；认不出的键原样显示（够用，不值得再造一层元数据）。 */
export function fieldLabel(key: string): string {
  const known: Record<string, string> = {
    enabled: "启用",
    cooldown_seconds: "冷却间隔（秒）",
    min_ms: "最短时长（ms）",
    min_score: "最低匹配分",
  };
  return known[key] ?? key;
}
