import type { SceneMode, SceneParams } from "./api";

/** 开始监听时可以开关的分析插件。简报检索由宿主关闭，不在这里。 */
export type PluginFeature = "key_point_llm" | "term_explainer" | "translator";

/** 按人汇总不是管道插件，只决定转写页怎么分组。 */
export type FeatureId = PluginFeature | "tasks";

export interface LaunchChoice {
  /** 相对该场景可开的插件，本次关掉的项。 */
  disabled: FeatureId[];
  /** 要点开着时，是否把行动项按人汇总。 */
  tasks: boolean;
}

export interface SceneLaunchOptions {
  custom?: Pick<SceneParams, "plugin_allowlist" | "translation_mode">;
  /** 插件配置里 enabled === false 的项，默认本次也不开。 */
  pluginEnabled?: Partial<Record<PluginFeature, boolean>>;
}

export const FEATURE_LABELS: Record<FeatureId, string> = {
  key_point_llm: "要点",
  term_explainer: "术语解释",
  translator: "翻译",
  tasks: "按人汇总",
};

const STORAGE_KEY = "talksage.listenLaunch.v2";

const PLUGIN_FEATURES: readonly PluginFeature[] = ["key_point_llm", "term_explainer", "translator"];

export function offeredPlugins(
  scene: SceneMode,
  custom?: Pick<SceneParams, "plugin_allowlist" | "translation_mode">,
): PluginFeature[] {
  switch (scene) {
    case "dictation":
      return ["key_point_llm"];
    case "conversation":
    case "meeting":
    case "lecture":
      return ["key_point_llm", "term_explainer"];
    case "bilingual":
      return ["key_point_llm", "term_explainer", "translator"];
    case "live_translation":
      return ["translator"];
    case "custom":
      return customPlugins(custom);
    default: {
      const unreachable: never = scene;
      return unreachable;
    }
  }
}

function customPlugins(
  custom?: Pick<SceneParams, "plugin_allowlist" | "translation_mode">,
): PluginFeature[] {
  const allow = new Set(custom?.plugin_allowlist ?? []);
  const plugins: PluginFeature[] = [];
  if (allow.has("key_point_llm")) plugins.push("key_point_llm");
  if (allow.has("term_explainer")) plugins.push("term_explainer");
  if (allow.has("translator") && custom?.translation_mode && custom.translation_mode !== "off") {
    plugins.push("translator");
  }
  return plugins;
}

function isFeatureId(value: unknown): value is FeatureId {
  return value === "tasks" || (typeof value === "string" && PLUGIN_FEATURES.some((id) => id === value));
}

export function defaultSceneChoice(scene: SceneMode, options: SceneLaunchOptions = {}): LaunchChoice {
  const offered = offeredPlugins(scene, options.custom);
  const disabled = offered.filter((id) => options.pluginEnabled?.[id] === false);
  const keyPointsOn = offered.includes("key_point_llm") && !disabled.includes("key_point_llm");
  return {
    disabled,
    tasks: scene === "meeting" && keyPointsOn,
  };
}

/** 把记住的选择收成该场景此刻真正能用的集合。 */
export function resolveSceneChoice(
  scene: SceneMode,
  stored: Partial<LaunchChoice> | null,
  options: SceneLaunchOptions = {},
): LaunchChoice {
  const base = defaultSceneChoice(scene, options);
  if (!stored) return base;
  const offered = offeredPlugins(scene, options.custom);
  const disabled = Array.isArray(stored.disabled)
    ? stored.disabled.filter((id): id is FeatureId => isFeatureId(id) && (id === "tasks" || offered.includes(id as PluginFeature)))
    : base.disabled;
  const keyPointsOn = offered.includes("key_point_llm") && !disabled.includes("key_point_llm");
  return {
    disabled,
    tasks: keyPointsOn && stored.tasks === true,
  };
}

export function loadSceneChoice(scene: SceneMode, options: SceneLaunchOptions = {}): LaunchChoice {
  return resolveSceneChoice(scene, readStoredChoice(scene), options);
}

export function rememberSceneChoice(scene: SceneMode, choice: LaunchChoice): void {
  try {
    const store = readStore();
    store[scene] = {
      disabled: choice.disabled,
      tasks: choice.tasks,
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store));
  } catch {
    // 隐私模式写不进本地存储时，本次选择仍然生效。
  }
}

export function toggleFeature(choice: LaunchChoice, feature: FeatureId, offered: readonly PluginFeature[]): LaunchChoice {
  if (feature === "tasks") {
    if (!offered.includes("key_point_llm") || choice.disabled.includes("key_point_llm")) return choice;
    return { ...choice, tasks: !choice.tasks };
  }
  if (!offered.includes(feature)) return choice;
  const turningOff = !choice.disabled.includes(feature);
  const disabled = turningOff
    ? [...choice.disabled, feature]
    : choice.disabled.filter((id) => id !== feature);
  return {
    ...choice,
    disabled,
    tasks: feature === "key_point_llm" && turningOff ? false : choice.tasks,
  };
}

export function enabledPlugins(choice: LaunchChoice, offered: readonly PluginFeature[]): PluginFeature[] {
  return offered.filter((feature) => !choice.disabled.includes(feature));
}

export function tasksEnabled(choice: LaunchChoice, offered: readonly PluginFeature[]): boolean {
  return choice.tasks && offered.includes("key_point_llm") && !choice.disabled.includes("key_point_llm");
}

/** 交给管道的分析插件 id。按人汇总不在这里。 */
export function sessionPlugins(choice: LaunchChoice, offered: readonly PluginFeature[]): string[] {
  return enabledPlugins(choice, offered);
}

export function launchSummary(choice: LaunchChoice, offered: readonly PluginFeature[]): string {
  const labels = [
    ...enabledPlugins(choice, offered).map((feature) => FEATURE_LABELS[feature]),
    ...(tasksEnabled(choice, offered) ? [FEATURE_LABELS.tasks] : []),
  ];
  return labels.length > 0 ? labels.join("、") : "未开分析功能";
}

function readStoredChoice(scene: SceneMode): Partial<LaunchChoice> | null {
  const raw = readStore()[scene];
  return raw ?? null;
}

function readStore(): Partial<Record<SceneMode, Partial<LaunchChoice>>> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Partial<Record<SceneMode, Partial<LaunchChoice>>>;
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}
