// 设置面板：按 Tab 归类。保存写入 talksage.toml。
// 模型安装/删除在独立的「模型管理」页，不占用本页。

import { useEffect, useLayoutEffect, useState } from "react";
import type { AppConfig, AsrModelInfo, AsrRuntimeStatus, OfflineUpgradeResult, PluginMeta, PluginStatusInfo, SceneMode, SceneParams, UpdateCheckResult } from "../lib/api";
import type { PluginValues } from "../lib/plugins";
import { analysisPluginIds, buildPluginUpdates, fieldLabel, initialPluginValues, pluginFields, pluginStatusLabel } from "../lib/plugins";
import { knowledgeBaseSettings } from "../lib/knowledge";
import { llmProviderApiKey } from "../lib/llm";
import { getApi } from "../lib/transport";

const api = getApi();
const PROVIDERS = ["deepseek", "kimi", "minimax", "groq", "ollama", "claude"];
const VOICE_ENROLL_SECONDS = 16;
const VOICE_ENROLL_TEXT =
  "你好，我正在为拓思者录制声音标识。今天阳光明亮，我会清楚、自然、连续地读完这段文字。会议结束后，请帮我整理重点、时间和下一步行动。";

type SettingsTab = "scene" | "asr" | "audio" | "terminology" | "plugins" | "voice" | "llm" | "webhooks" | "network" | "upgrade";

/** 场景清单：chip 渲染与「当前生效场景」文案共用同一份数据。 */
const SCENE_MODES: { key: SceneMode; label: string; desc: string }[] = [
  { key: "dictation", label: "单人听写", desc: "单麦克风、灵敏 VAD、最低资源消耗" },
  { key: "conversation", label: "一对一会话", desc: "双人会话，按输入通道区分双方，两流使用相同语言" },
  { key: "bilingual", label: "双语对话", desc: "双语会话：我说中文，对方说英文（或反向），双向翻译" },
  { key: "live_translation", label: "实时翻译", desc: "说一种语言，自动翻译并显示另一种语言" },
  { key: "meeting", label: "多人会议", desc: "两人以上，启用 WeSpeaker 在线角色识别" },
  { key: "lecture", label: "演讲/课堂", desc: "长段单流，术语和简报增强，关闭角色识别" },
  { key: "custom", label: "自定义", desc: "使用下方全部参数" },
];

/** 配置补丁里的字段路径 → 它属于哪个 tab。按前缀匹配，先列更具体的。 */
const TAB_OF_PATH: { prefix: string; tab: SettingsTab }[] = [
  { prefix: "scene.", tab: "scene" },
  { prefix: "asr.terminology.", tab: "terminology" },
  { prefix: "asr.", tab: "asr" },
  { prefix: "audio.", tab: "audio" },
  { prefix: "recording.", tab: "audio" },
  { prefix: "plugins.", tab: "plugins" },
  { prefix: "knowledge_base.", tab: "plugins" },
  { prefix: "quality.", tab: "audio" }, // 会话质量阈值并入「音频与录音」
  { prefix: "llm.", tab: "llm" },
  { prefix: "webhooks.", tab: "webhooks" },
  { prefix: "network.", tab: "network" },
];

/** 嵌套配置对象拍平成「叶子路径 → JSON 值」，用于逐字段比较。数组按整体算一个叶子。 */
function flattenLeaves(value: unknown, prefix = "", out: Record<string, string> = {}): Record<string, string> {
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    for (const [k, v] of Object.entries(value)) flattenLeaves(v, prefix ? `${prefix}.${k}` : k, out);
  } else {
    out[prefix] = JSON.stringify(value ?? null);
  }
  return out;
}

/** 未保存标记：tab 与 chip 共用同一个小圆点，视觉语言保持一致。 */
function DirtyDot({ inset }: { inset?: boolean }) {
  return (
    <span
      title="有未保存的改动"
      style={{
        width: 6,
        height: 6,
        borderRadius: "50%",
        background: "var(--brief)",
        display: "inline-block",
        ...(inset
          ? { position: "absolute", top: -3, right: -3, border: "1px solid var(--surface-2)" }
          : { marginLeft: 5, verticalAlign: "middle" }),
      }}
    />
  );
}

const TABS: { key: SettingsTab; label: string; desc: string }[] = [
  { key: "scene", label: "场景模式", desc: "听写 / 会话 / 双语 / 会议 / 课堂 / 自定义" },
  { key: "asr", label: "ASR 转写", desc: "引擎 / 输入增益" },
  { key: "audio", label: "音频与录音", desc: "采集 / 灵敏度 / 断句 / 降噪 / 质量阈值 / 录音" },
  { key: "terminology", label: "术语纠错", desc: "热词与误识别替换" },
  { key: "plugins", label: "插件", desc: "专业术语 / 翻译 / 知识源 / 要点聚合" },
  { key: "voice", label: "声音标识", desc: "注册主人声音，识别说话人" },
  { key: "llm", label: "LLM", desc: "默认模型与密钥" },
  { key: "webhooks", label: "Webhook", desc: "会议结束推送（n8n/Zapier/CRM）" },
  { key: "network", label: "网络", desc: "代理服务器" },
  { key: "upgrade", label: "升级", desc: "在线检查 / 离线安装升级包" },
];

export default function SettingsSection({
  config,
  onSave,
  onOpenModels,
  onDirtyChange,
}: {
  config: AppConfig | null;
  onSave: (updates: Record<string, unknown>) => Promise<void>;
  onOpenModels: () => void;
  /** 有未保存改动时上报给 App：离开设置页前要拦一下。 */
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [tab, setTab] = useState<SettingsTab>("scene");
  // 场景模式
  const [sceneMode, setSceneMode] = useState<SceneMode>(() => {
    const m = (config?.scene?.mode ?? "conversation") as string;
    return (m === "translation" ? "bilingual" : m) as SceneMode;
  });
  // 模板场景的语言选择（非自定义模式）。
  // 从 custom.language 读取：handleSave 在非自定义模式时将 sceneLanguage 写入 custom，
  // 因此重新打开设置时能正确恢复上次选择的语言。
  const [sceneLanguage, setSceneLanguage] = useState<"zh" | "en">(() => {
    const mode = (config?.scene?.mode ?? "conversation") as string;
    // 只有非自定义场景才从 custom.language 还原语言选择；自定义场景语言在 sceneCustom 里管理。
    if (mode !== "custom") {
      return (config?.scene?.custom?.language as "zh" | "en") ?? "zh";
    }
    return "zh";
  });
  const [sceneClientLanguage, setSceneClientLanguage] = useState<"zh" | "en">(() => {
    const mode = (config?.scene?.mode ?? "conversation") as string;
    if (mode !== "custom") {
      return (config?.scene?.custom?.client_language as "zh" | "en") ?? "en";
    }
    return "en";
  });
  const [sceneCustom, setSceneCustom] = useState<SceneParams>(() => ({
    vad_preset: config?.scene?.custom?.vad_preset ?? "standard",
    vad_threshold: config?.scene?.custom?.vad_threshold ?? null,
    vad_min_speech_ms: config?.scene?.custom?.vad_min_speech_ms ?? null,
    vad_min_silence_ms: config?.scene?.custom?.vad_min_silence_ms ?? null,
    vad_max_speech_ms: config?.scene?.custom?.vad_max_speech_ms ?? null,
    denoise_enabled: config?.scene?.custom?.denoise_enabled ?? false,
    denoise_gate: config?.scene?.custom?.denoise_gate ?? 0.008,
    min_segment_ms: config?.scene?.custom?.min_segment_ms ?? 0,
    asr_segment_ms: config?.scene?.custom?.asr_segment_ms ?? 4000,
    user_engine: config?.scene?.custom?.user_engine ?? "qwen3-asr",
    client_enabled: config?.scene?.custom?.client_enabled ?? true,
    client_engine: config?.scene?.custom?.client_engine ?? "qwen3-asr",
    language: (config?.scene?.custom?.language as "zh" | "en") ?? "zh",
    client_language: (config?.scene?.custom?.client_language as "zh" | "en") ?? "en",
    translation_mode: config?.scene?.custom?.translation_mode ?? "off",
    // 配置没给 allowlist 时，元数据到货后回填「全部分析类插件」（见下方 effect）——
    // 这里不能写死 id 列表：哪些插件算分析类归 Rust descriptor。
    plugin_allowlist: config?.scene?.custom?.plugin_allowlist ?? [],
    speaker_mode: config?.scene?.custom?.speaker_mode ?? "channel",
    noise_auto_detect: config?.scene?.custom?.noise_auto_detect ?? true,
  }));
  const [defaultProvider, setDefaultProvider] = useState(config?.llm?.default ?? "deepseek");
  const [apiKey, setApiKey] = useState(() => llmProviderApiKey(config, config?.llm?.default ?? "deepseek"));
  const [llmTesting, setLlmTesting] = useState(false);
  const [llmTestResult, setLlmTestResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [aliyunTesting, setAliyunTesting] = useState(false);
  const [aliyunTestResult, setAliyunTestResult] = useState<{ ok: boolean; text: string } | null>(null);
  // 插件表单由 /plugins 元数据生成：设置页不认识任何具体插件。
  const [pluginMeta, setPluginMeta] = useState<PluginMeta[]>([]);
  const [pluginStatus, setPluginStatus] = useState<PluginStatusInfo[]>([]);
  const [pluginValues, setPluginValues] = useState<PluginValues>({});
  const [kbFolder, setKbFolder] = useState(() => knowledgeBaseSettings(config).folder);
  const [kbEnabled, setKbEnabled] = useState(() => knowledgeBaseSettings(config).enabled);
  // 引擎显示值：自定义场景读场景参数；否则读全局 ASR 配置（与 pipeline 实际
  // 生效逻辑一致：非自定义场景的用户流引擎跟随 [asr].user_engine）。
  const initialEngineZh = () =>
    config?.scene?.mode === "custom"
      ? config?.scene?.custom?.user_engine ?? config?.asr?.engine_zh ?? "qwen3-asr"
      : config?.asr?.engine_zh ?? "qwen3-asr";
  const initialEngineEn = () =>
    config?.scene?.mode === "custom"
      ? config?.scene?.custom?.client_engine ?? config?.asr?.engine_en ?? "qwen3-asr"
      : config?.asr?.engine_en ?? "qwen3-asr";
  const [engineEn, setEngineEn] = useState<string>(initialEngineEn);
  const [engineZh, setEngineZh] = useState<string>(initialEngineZh);
  const [punctEnabled, setPunctEnabled] = useState<boolean>(config?.asr?.punct_enabled ?? true);
  const [asrMode, setAsrMode] = useState(config?.asr?.asr_mode ?? "auto");
  const [languageMode, setLanguageMode] = useState(config?.asr?.language_mode ?? "scene");
  const configuredBackend = config?.asr?.backend ?? "auto";
  const [asrBackend, setAsrBackend] = useState(
    configuredBackend === "coreml" || configuredBackend === "metal" ? "auto" : configuredBackend,
  );
  const [aliyunKeyId, setAliyunKeyId] = useState(config?.asr?.aliyun_access_key_id ?? "");
  const [aliyunKeySecret, setAliyunKeySecret] = useState(config?.asr?.aliyun_access_key_secret ?? "");
  const [aliyunAppKey, setAliyunAppKey] = useState(config?.asr?.aliyun_app_key ?? "");
  const [gpuStatus, setGpuStatus] = useState<AsrRuntimeStatus | null>(null);
  const [terminologyEnabled, setTerminologyEnabled] = useState(config?.asr?.terminology?.enabled ?? false);
  const [hotwordScore, setHotwordScore] = useState(config?.asr?.terminology?.hotword_score ?? 1.5);
  const [terminologyTerms, setTerminologyTerms] = useState((config?.asr?.terminology?.terms ?? []).join("\n"));
  const [terminologyCorrections, setTerminologyCorrections] = useState(
    Object.entries(config?.asr?.terminology?.corrections ?? {}).map(([wrong, right]) => `${wrong} => ${right}`).join("\n")
  );
  const [audioSource, setAudioSource] = useState<"mic" | "loopback">(config?.audio?.audio_source ?? "mic");
  const [vadPreset, setVadPreset] = useState<string>(config?.audio?.vad?.preset ?? "standard");
  const [denoiseEnabled, setDenoiseEnabled] = useState(config?.audio?.denoise?.enabled ?? false);
  const [highpass, setHighpass] = useState(config?.audio?.denoise?.highpass ?? true);
  const [minSegmentMs, setMinSegmentMs] = useState<number>(config?.audio?.min_segment_ms ?? 0);
  const [inputGainDb, setInputGainDb] = useState<number>(config?.audio?.input_gain_db ?? 12);
  const [endpointEnabled, setEndpointEnabled] = useState(config?.audio?.endpoint?.enabled ?? true);
  const [endpointStableMs, setEndpointStableMs] = useState(config?.audio?.endpoint?.stable_ms ?? 350);
  const [endpointQuietMs, setEndpointQuietMs] = useState(config?.audio?.endpoint?.quiet_ms ?? 450);
  const [endpointForceQuietMs, setEndpointForceQuietMs] = useState(config?.audio?.endpoint?.force_quiet_ms ?? 850);
  const [recEnabled, setRecEnabled] = useState(config?.recording?.enabled ?? true);
  const [recDir, setRecDir] = useState<string>(config?.recording?.dir ?? "");
  // 自动检测背景噪音：实际生效字段是 scene.custom.noise_auto_detect（pipeline
  // finalize 用场景值覆盖 quality.auto_detect）。与场景 tab 自定义模式共用同一
  // 开关（同一 state），避免两处不一致；qAutoDetect 仅为 audio tab 内联副本。
  const qAutoDetect = sceneCustom.noise_auto_detect;
  const setQAutoDetect = (v: boolean) => setSceneCustom({ ...sceneCustom, noise_auto_detect: v });
  const [qTextNoise, setQTextNoise] = useState(config?.quality?.text_noise_threshold ?? 0.45);
  const [qMinRatio, setQMinRatio] = useState(config?.quality?.min_speech_ratio ?? 0.15);
  const [qMaxRatio, setQMaxRatio] = useState(config?.quality?.max_speech_ratio ?? 0.85);
  const [qSilenceRms, setQSilenceRms] = useState(config?.quality?.silence_rms ?? 0.01);
  const [qHighRms, setQHighRms] = useState(config?.quality?.high_rms ?? 0.5);
  const [whEnabled, setWhEnabled] = useState(config?.webhooks?.enabled ?? false);
  const [whUrls, setWhUrls] = useState<string>((config?.webhooks?.urls ?? []).join("\n"));
  const [proxy, setProxy] = useState(config?.network?.proxy ?? "");
  const [proxyTestResult, setProxyTestResult] = useState<{ ok: boolean; msg: string } | null>(null);
  const [proxyTesting, setProxyTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");
  const [asrModels, setAsrModels] = useState<AsrModelInfo[]>([]);
  const [voiceStatus, setVoiceStatus] = useState<{ model_available: boolean; enrolled: boolean } | null>(null);
  const [enrolling, setEnrolling] = useState(false);
  const [enrollCount, setEnrollCount] = useState(0);
  const [enrollStage, setEnrollStage] = useState<"idle" | "countdown" | "recording" | "processing">("idle");
  // 应用升级（在线检查框架 + 离线安装升级包）
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [upgradeChecking, setUpgradeChecking] = useState(false);
  const [upgradeCheckResult, setUpgradeCheckResult] = useState<UpdateCheckResult | null>(null);
  const [upgradeInstalling, setUpgradeInstalling] = useState(false);
  const [upgradeInstallResult, setUpgradeInstallResult] = useState<OfflineUpgradeResult | null>(null);

  // 加载声纹状态 / ASR 模型 / 插件元数据。
  //
  // 三个请求各自兜底：一个挂了不该带走另外两个。插件元数据挂了整个插件页就是
  // 空的（宁可空，也不要退回硬编码的三个开关 —— 那样用户会以为只有三个插件）。
  useEffect(() => {
    const warn = (what: string) => (e: unknown) => {
      console.error(`读取${what}失败:`, e);
      return null;
    };
    (async () => {
      api.getAsrRuntimeStatus()
        .then(setGpuStatus)
        .catch(() => {});
      const [voice, models, metas, statuses] = await Promise.all([
        api.getVoiceprintStatus().catch(warn("声纹状态")),
        api.listAsrModels().catch(warn("ASR 模型列表")),
        api.listPlugins().catch(warn("插件元数据")),
        api.listPluginStatus().catch(warn("插件状态")),
      ]);
      if (voice) setVoiceStatus(voice);
      if (models) setAsrModels(models);
      if (statuses) setPluginStatus(statuses);
      if (metas) {
        setPluginMeta(metas);
        setPluginValues(initialPluginValues(metas, config?.plugins));
        // 配置里没有 allowlist（老配置 / headless 的 /config 不返回 scene）时，
        // 按元数据回填成「分析类插件全开」—— 与阶段 5 之前的默认行为一致。
        if (!config?.scene?.custom?.plugin_allowlist) {
          setSceneCustom((s) => ({ ...s, plugin_allowlist: analysisPluginIds(metas) }));
        }
      }
      // 异步数据到齐（含失败兜底）：此刻的表单就是「已保存状态」，取作脏状态基线。
      setRebaseTick((t) => t + 1);
    })();
    // 只在挂载时跑一次：config 也只在挂载时读（本组件其余 state 同一约定），
    // 设置页是从 App 的 navPage 切进来的，切进来就是一次新的挂载。
  }, []);

  // API Key 必须跟当前 provider / 已加载配置走：state 初始值在 config 尚未到达时是空的，
  // 且切换供应商时也不能继续显示上一家的密钥。
  useEffect(() => {
    setApiKey(llmProviderApiKey(config, defaultProvider));
  }, [config, defaultProvider]);

  useEffect(() => {
    const kb = knowledgeBaseSettings(config);
    setKbEnabled(kb.enabled);
    setKbFolder(kb.folder);
  }, [config]);

  /** 改某个插件的某个配置键。 */
  function setPluginField(id: string, key: string, value: boolean | number | string) {
    setPluginValues((prev) => ({ ...prev, [id]: { ...prev[id], [key]: value } }));
  }

  const modelOptions = (selected: string) => asrModels.filter((m) => m.selectable !== false).map((m) => {
    const speed = m.speed === "realtime" ? "实时" : m.speed === "balanced" ? "平衡" : "准确优先";
    const unavailable = !m.installed && m.id !== selected;
    return <option key={m.id} value={m.id} disabled={unavailable}>{m.label}（{speed}{m.streaming ? " / 流式" : " / 段级"}）{m.installed ? "" : " — 未安装"}</option>;
  });

  /** 固定文本引导注册：准备倒计时 → 录制 → 本地多窗口声纹处理。 */
  async function handleEnroll() {
    setEnrolling(true);
    setMessage("");
    try {
      setEnrollStage("countdown");
      for (let count = 3; count > 0; count--) {
        setEnrollCount(count);
        await new Promise((resolve) => window.setTimeout(resolve, 1000));
      }
      setEnrollStage("recording");
      setEnrollCount(VOICE_ENROLL_SECONDS);
      const timer = window.setInterval(() => setEnrollCount((c) => Math.max(0, c - 1)), 1000);
      const processingTimer = window.setTimeout(() => setEnrollStage("processing"), VOICE_ENROLL_SECONDS * 1000);
      const pending = api.enrollVoice(VOICE_ENROLL_SECONDS);
      const r = await pending.finally(() => {
        window.clearInterval(timer);
        window.clearTimeout(processingTimer);
      });
      setVoiceStatus({ model_available: true, enrolled: true });
      setMessage(`主人声纹已保存：有效语音 ${(r.voiced_ms / 1000).toFixed(1)} 秒，${r.windows} 个窗口，维度 ${r.dim}。`);
    } catch (e) {
      setMessage(`注册失败：${e}。请保持 20–40 厘米距离，按提示连续朗读后重试。`);
    } finally {
      setEnrollCount(0);
      setEnrollStage("idle");
      setEnrolling(false);
    }
  }

  /** 删除主人声纹。 */
  async function handleRemoveVoice() {
    try {
      await api.removeVoiceprint();
      setVoiceStatus((s) => (s ? { ...s, enrolled: false } : s));
      setMessage("声音标识已删除");
    } catch (e) {
      setMessage(`删除失败: ${e}`);
    }
  }

  const inputStyle: React.CSSProperties = {
    fontSize: 12,
    padding: "4px 8px",
    borderRadius: 4,
    border: "1px solid var(--border)",
    background: "var(--surface-2)",
    color: "var(--text)",
    boxSizing: "border-box",
  };
  const numStyle: React.CSSProperties = { ...inputStyle, width: 90, marginLeft: 8 };
  const labelBlock: React.CSSProperties = { display: "block", marginBottom: 4 };
  const hint: React.CSSProperties = { marginTop: 4, color: "var(--muted)", fontSize: 11, lineHeight: 1.6 };
  const groupTitle: React.CSSProperties = { margin: "0 0 6px", fontSize: 13 };

  /**
   * 按元数据渲染一组插件的控件。
   *
   * `enabled` 用插件显示名当勾选框文案（沿用阶段 5 之前的样子）；其余键按
   * 默认值的类型渲染，缩进一级挂在开关下面，插件关掉时置灰但仍可编辑 ——
   * 与本页 terminology / denoise 的处理一致。
   *
   * `hostManaged` 的键渲染成 disabled 并注明原因：它们在装配时被场景参数
   * 覆盖，留着可编辑就是骗用户。
   */
  function renderPluginGroup(metas: PluginMeta[]) {
    return metas.map((meta) => {
      const values = pluginValues[meta.id] ?? {};
      const userEnabled = values.enabled !== false;
      const registration = pluginStatus.find((item) => item.id === meta.id);
      const statusText = pluginStatusLabel(registration);
      const statusColor = registration?.status === "active" ? "#2e7d32"
        : registration?.status === "disabled" ? "#777" : "#b45309";
      return (
        <div key={meta.id} style={{ marginBottom: 4 }}>
          {pluginFields(meta).map((f) => {
            const locked = f.hostManaged;
            const note = locked ? <span style={{ ...hint, marginLeft: 8, display: "inline" }}>由场景参数决定</span> : null;
            if (f.key === "enabled") {
              return (
                <label key={f.key} style={{ ...labelBlock, opacity: locked ? 0.5 : 1 }}>
                  <input
                    type="checkbox"
                    checked={values.enabled === true}
                    disabled={locked}
                    onChange={(e) => setPluginField(meta.id, f.key, e.target.checked)}
                  />{" "}
                  {meta.label}
                  {note}
                  {meta.description ? <span style={{ ...hint, marginLeft: 8, display: "inline" }}>{meta.description}</span> : null}
                  <span style={{ ...hint, marginLeft: 8, display: "inline", color: statusColor }}>［{statusText}］</span>
                </label>
              );
            }
            const dim = locked || !userEnabled;
            return (
              <label key={f.key} style={{ ...labelBlock, marginLeft: 20, opacity: dim ? 0.5 : 1 }}>
                {f.kind === "bool" ? (
                  <>
                    <input
                      type="checkbox"
                      checked={values[f.key] === true}
                      disabled={locked}
                      onChange={(e) => setPluginField(meta.id, f.key, e.target.checked)}
                    />{" "}
                    {fieldLabel(f.key)}
                  </>
                ) : f.kind === "number" ? (
                  <>
                    {fieldLabel(f.key)}：
                    <input
                      type="number"
                      step={Number.isInteger(f.default as number) ? 1 : 0.05}
                      disabled={locked}
                      value={typeof values[f.key] === "number" ? (values[f.key] as number) : (f.default as number)}
                      onChange={(e) => {
                        const n = Number(e.target.value);
                        setPluginField(meta.id, f.key, Number.isFinite(n) ? n : (f.default as number));
                      }}
                      style={numStyle}
                    />
                  </>
                ) : (
                  <>
                    {fieldLabel(f.key)}：
                    <input
                      disabled={locked}
                      value={typeof values[f.key] === "string" ? (values[f.key] as string) : (f.default as string)}
                      onChange={(e) => setPluginField(meta.id, f.key, e.target.value)}
                      style={{ ...inputStyle, marginLeft: 8 }}
                    />
                  </>
                )}
                {note}
              </label>
            );
          })}
        </div>
      );
    });
  }

  /** 验证 LLM 连接：用表单当前值（可未保存）发最小请求。 */
  async function handleTestLlm() {
    if (llmTesting) return;
    setLlmTesting(true);
    setLlmTestResult(null);
    try {
      await api.testLlm({
        provider: defaultProvider,
        apiKey: apiKey.trim() || undefined,
      });
      setLlmTestResult({ ok: true, text: "连接正常，API Key 有效" });
    } catch (e) {
      setLlmTestResult({ ok: false, text: String(e) });
    } finally {
      setLlmTesting(false);
    }
  }

  /** 验证阿里云 ASR 凭据：请求 NLS AccessToken（HMAC-SHA1 签名）。 */
  async function handleTestAliyun() {
    if (aliyunTesting) return;
    setAliyunTesting(true);
    setAliyunTestResult(null);
    try {
      const r = await api.testAliyunAsr({
        accessKeyId: aliyunKeyId.trim(),
        accessKeySecret: aliyunKeySecret.trim(),
        appKey: aliyunAppKey.trim(),
      });
      const hours = Math.round(r.valid_for_secs / 3600);
      setAliyunTestResult({ ok: true, text: `凭据有效，Token 有效期约 ${hours} 小时${r.app_key ? `（AppKey: ${r.app_key}）` : ""}` });
    } catch (e) {
      setAliyunTestResult({ ok: false, text: String(e) });
    } finally {
      setAliyunTesting(false);
    }
  }

  async function handlePickKbFolder() {
    try {
      const path = await api.pickFolder();
      if (path) {
        setKbFolder(path);
        setKbEnabled(true);
      }
    } catch (e) {
      setMessage(String(e));
    }
  }

  // ── 未保存改动检测 ────────────────────────────────
  // 整页是一个大表单：任何字段改完不点「保存设置」就切走都会静默丢失，而场景 chip
  // 点下去立刻高亮，最容易被误以为「选了就生效」。这里用「当前快照 vs 基线快照」
  // 逐字段比对，把结果落到 tab 圆点、场景 chip 和底部保存栏上。

  /** 表单 → 配置补丁。handleSave 提交它，脏状态检测也比较它——必须是同一份数据。 */
  function buildSnapshot(): Record<string, unknown> {
    const corrections = Object.fromEntries(terminologyCorrections.split("\n").map((line) => {
      const [wrong, ...right] = line.split("=>");
      return [wrong?.trim(), right.join("=>").trim()];
    }).filter(([wrong, right]) => wrong && right));
    return {
      llm: {
        default: defaultProvider,
        providers: {
          [defaultProvider]: {
            api_key: apiKey.trim(),
          },
        },
      },
      // 按元数据组装，键与值都来自 /plugins —— 组件里没有插件名
      plugins: {
        ...buildPluginUpdates(pluginMeta, pluginValues),
        knowledge_obsidian: { enabled: kbEnabled, folder: kbFolder.trim() },
      },
      knowledge_base: {
        enabled: kbEnabled,
        folder: kbFolder.trim(),
      },
      asr: {
        engine_en: engineEn,
        engine_zh: engineZh,
        backend: asrBackend,
        punct_enabled: punctEnabled,
        asr_mode: asrMode,
        language_mode: languageMode,
        aliyun_access_key_id: aliyunKeyId,
        aliyun_access_key_secret: aliyunKeySecret,
        aliyun_app_key: aliyunAppKey,
        terminology: {
          enabled: terminologyEnabled,
          hotword_score: hotwordScore,
          terms: terminologyTerms.split("\n").map((v) => v.trim()).filter(Boolean),
          corrections,
        },
      },
      audio: {
        audio_source: audioSource,
        input_gain_db: inputGainDb,
        vad: { preset: vadPreset },
        denoise: {
          enabled: denoiseEnabled,
          highpass,
        },
        endpoint: {
          enabled: endpointEnabled,
          stable_ms: endpointStableMs,
          quiet_ms: endpointQuietMs,
          force_quiet_ms: endpointForceQuietMs,
        },
        min_segment_ms: minSegmentMs,
      },
      recording: {
        enabled: recEnabled,
        dir: recDir.trim(),
      },
      // auto_detect 实际由场景级 scene.custom.noise_auto_detect 生效（finalize 覆盖），
      // 这里只提交阈值；开关已并入场景字段，避免两处脏点与误导。
      quality: {
        text_noise_threshold: qTextNoise,
        min_speech_ratio: qMinRatio,
        max_speech_ratio: qMaxRatio,
        silence_rms: qSilenceRms,
        high_rms: qHighRms,
      },
      webhooks: {
        enabled: whEnabled,
        urls: whUrls.split("\n").map((s) => s.trim()).filter((s) => s.length > 0),
      },
      network: {
        proxy: proxy.trim(),
      },
      scene: {
        mode: sceneMode,
        custom: {
          ...sceneCustom,
          language: sceneMode === "custom" ? sceneCustom.language : sceneLanguage,
          client_language: sceneMode === "custom" ? sceneCustom.client_language : sceneClientLanguage,
        },
      },
    };
  }

  const snapshot = buildSnapshot();
  // baseline = null：异步数据（插件元数据等）还没到齐，此时不判断脏不脏。
  const [baseline, setBaseline] = useState<Record<string, unknown> | null>(null);
  // 递增 → 下一次渲染后把当前快照定为新基线（水合完成 / 保存成功 / 恢复默认后）。
  const [rebaseTick, setRebaseTick] = useState(0);
  useEffect(() => {
    if (rebaseTick === 0) return;
    setBaseline(buildSnapshot());
    // 基线只在 tick 变化时取一次：buildSnapshot 读的是本次渲染的表单状态。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rebaseTick]);

  const dirtyPaths = (() => {
    if (!baseline) return [] as string[];
    const now = flattenLeaves(snapshot);
    const base = flattenLeaves(baseline);
    return Object.keys(now).filter((k) => now[k] !== base[k]);
  })();
  const dirtyCount = dirtyPaths.length;
  const dirtyTabs = new Set<SettingsTab>(
    dirtyPaths
      .map((path) => TAB_OF_PATH.find((m) => path.startsWith(m.prefix))?.tab)
      .filter((t): t is SettingsTab => !!t),
  );
  const sceneModeDirty = dirtyPaths.includes("scene.mode");
  // 已生效的场景（配置里的值），与刚点选、尚未保存的 sceneMode 对照展示。
  // 与组件顶部 sceneMode 初始化同一套归一化：旧配置里的 translation === bilingual。
  const savedSceneMode = ((m: string) => (m === "translation" ? "bilingual" : m))(
    (config?.scene?.mode ?? "conversation") as string,
  );
  const savedSceneLabel = SCENE_MODES.find((m) => m.key === savedSceneMode)?.label ?? savedSceneMode;
  const pendingSceneLabel = SCENE_MODES.find((m) => m.key === sceneMode)?.label ?? sceneMode;

  // 上报给 App：离开设置页前弹确认。卸载时清掉，避免拦截状态残留。
  // 用 layout effect：改完立刻按 ⌘/Ctrl+Shift+L 开始监听时，拦截状态必须在下一个
  // 事件之前就绪，异步的 useEffect 会漏掉这种紧挨着的操作。
  useLayoutEffect(() => {
    onDirtyChange?.(dirtyCount > 0);
  }, [dirtyCount, onDirtyChange]);
  useLayoutEffect(() => () => onDirtyChange?.(false), [onDirtyChange]);

  async function handleSave() {
    setSaving(true);
    setMessage("");
    try {
      await onSave(snapshot);
      const [statuses, runtimeStatus] = await Promise.all([
        api.listPluginStatus(),
        api.getAsrRuntimeStatus(),
      ]);
      setPluginStatus(statuses);
      setGpuStatus(runtimeStatus);
      setRebaseTick((t) => t + 1);
      setMessage("已保存（部分设置重启后生效）");
    } catch (e) {
      setMessage(`保存失败: ${e}`);
    } finally {
      setSaving(false);
    }
  }

  /** 恢复噪音检测默认值（写入配置，不依赖表单状态）。 */
  async function handleResetQuality() {
    setSaving(true);
    setMessage("");
    try {
      await onSave({ quality: null }); // Rust 侧：null → 恢复默认
      setSceneCustom((s) => ({ ...s, noise_auto_detect: true }));
      setQTextNoise(0.45);
      setQMinRatio(0.15);
      setQMaxRatio(0.85);
      setQSilenceRms(0.01);
      setQHighRms(0.5);
      setRebaseTick((t) => t + 1);
      setMessage("噪音检测阈值已恢复默认");
    } catch (e) {
      setMessage(`恢复默认失败: ${e}`);
    } finally {
      setSaving(false);
    }
  }

  // ── 应用升级 ──
  useEffect(() => {
    api.getVersion().then(setAppVersion).catch(() => setAppVersion("未知"));
  }, []);

  async function handleCheckUpdates() {
    setUpgradeChecking(true);
    setUpgradeCheckResult(null);
    try {
      setUpgradeCheckResult(await api.checkForUpdates());
    } catch (e) {
      setUpgradeCheckResult({
        available: false,
        configured: false,
        current_version: appVersion ?? "",
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setUpgradeChecking(false);
    }
  }

  async function handleInstallUpgradePackage() {
    setUpgradeInstalling(true);
    setUpgradeInstallResult(null);
    try {
      const path = await api.pickUpgradePackage();
      if (!path) return; // 用户取消
      setUpgradeInstallResult(await api.installOfflineUpgrade(path));
    } catch (e) {
      setUpgradeInstallResult({
        ok: false,
        version: "",
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setUpgradeInstalling(false);
    }
  }

  return (
    <div style={{ border: "1px solid var(--border)", borderRadius: 8, padding: 10, fontSize: 12, display: "flex", flexDirection: "column", height: "100%", minHeight: 0, boxSizing: "border-box" }}>
      {/* Tab 导航 */}
      <div style={{ display: "flex", gap: 6, marginBottom: 12, flexWrap: "wrap", flexShrink: 0 }}>
        {TABS.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            title={t.desc}
            style={{
              padding: "5px 12px",
              borderRadius: 8,
              border: "1px solid var(--border)",
              cursor: "pointer",
              font: "inherit",
              fontSize: 12,
              fontWeight: 600,
              background: tab === t.key ? "var(--me)" : "var(--surface-2)",
              color: tab === t.key ? "#fff" : "var(--text-2)",
            }}
          >
            {t.label}
            {dirtyTabs.has(t.key) && <DirtyDot />}
          </button>
        ))}
      </div>

      <div style={{ flex: 1, minHeight: 0, overflowY: "auto" }}>
      {/* ── 场景模式 ── */}
      {tab === "scene" && (
        <div>
          <h3 style={groupTitle}>场景模式</h3>
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 8 }}>
            {SCENE_MODES.map((m) => {
              const selected = sceneMode === m.key;
              // 已点选但没保存：虚线边框 + 角标，与"已生效"的实心 chip 区分开。
              const pending = selected && sceneModeDirty;
              return (
                <button
                  key={m.key}
                  onClick={() => setSceneMode(m.key)}
                  title={pending ? `${m.desc}（尚未保存）` : m.desc}
                  style={{
                    position: "relative",
                    padding: "6px 14px",
                    borderRadius: 8,
                    border: pending ? "1px dashed var(--brief)" : "1px solid var(--border)",
                    cursor: "pointer",
                    font: "inherit",
                    fontSize: 12,
                    fontWeight: 600,
                    background: selected ? (pending ? "var(--surface-2)" : "var(--me)") : "var(--surface-2)",
                    color: selected ? (pending ? "var(--brief)" : "#fff") : "var(--text-2)",
                  }}
                >
                  {m.label}
                  {pending && <DirtyDot inset />}
                </button>
              );
            })}
          </div>

          {/* 点了 chip 就以为生效是最常见的误解：显式对照"已生效 / 待保存" */}
          {sceneModeDirty && (
            <div
              style={{
                marginBottom: 8,
                padding: "6px 9px",
                borderRadius: 8,
                border: "1px dashed var(--brief)",
                background: "var(--surface-2)",
                fontSize: 11,
                lineHeight: 1.6,
                color: "var(--text-2)",
              }}
            >
              已选择「<b style={{ color: "var(--brief)" }}>{pendingSceneLabel}</b>」，需点击下方
              <b>「保存设置」</b>才会生效 · 当前生效：<b>{savedSceneLabel}</b>
            </div>
          )}

          {/* 单语言场景：识别语言选择器 */}
          {(sceneMode === "dictation" || sceneMode === "conversation" || sceneMode === "meeting" || sceneMode === "lecture") && (
            <div style={{ marginBottom: 8 }}>
              <label>
                识别语言：
                <select
                  value={sceneLanguage}
                  onChange={(e) => setSceneLanguage(e.target.value as "zh" | "en")}
                  style={{ ...inputStyle, marginLeft: 8 }}
                >
                  <option value="zh">中文</option>
                  <option value="en">英语</option>
                </select>
              </label>
              <span style={{ fontSize: 11, color: "var(--text-2)", marginLeft: 12 }}>
                两条通道均使用此语言识别（按通道区分说话人，不混合）
              </span>
            </div>
          )}

          {/* 双语对话：我的语言 + 对方语言 */}
          {sceneMode === "bilingual" && (
            <div style={{ display: "flex", gap: 12, marginBottom: 8, flexWrap: "wrap" }}>
              <label>
                我的语言：
                <select
                  value={sceneLanguage}
                  onChange={(e) => setSceneLanguage(e.target.value as "zh" | "en")}
                  style={{ ...inputStyle, marginLeft: 8 }}
                >
                  <option value="zh">中文</option>
                  <option value="en">英语</option>
                </select>
              </label>
              <label>
                对方语言：
                <select
                  value={sceneClientLanguage}
                  onChange={(e) => setSceneClientLanguage(e.target.value as "zh" | "en")}
                  style={{ ...inputStyle, marginLeft: 8 }}
                >
                  <option value="zh">中文</option>
                  <option value="en">英语</option>
                </select>
              </label>
            </div>
          )}

          {/* 实时翻译：输入语言 + 翻译目标语言 */}
          {sceneMode === "live_translation" && (
            <div style={{ display: "flex", gap: 12, marginBottom: 8, flexWrap: "wrap" }}>
              <label>
                我说的语言：
                <select
                  value={sceneLanguage}
                  onChange={(e) => setSceneLanguage(e.target.value as "zh" | "en")}
                  style={{ ...inputStyle, marginLeft: 8 }}
                >
                  <option value="zh">中文</option>
                  <option value="en">英语</option>
                </select>
              </label>
              <label>
                翻译为：
                <select
                  value={sceneClientLanguage}
                  onChange={(e) => setSceneClientLanguage(e.target.value as "zh" | "en")}
                  style={{ ...inputStyle, marginLeft: 8 }}
                >
                  <option value="zh">中文</option>
                  <option value="en">英语</option>
                </select>
              </label>
            </div>
          )}

          {sceneMode !== "custom" ? (
            <div style={{ background: "var(--surface-2)", borderRadius: 6, padding: 8, fontSize: 11, color: "var(--text-2)", lineHeight: 1.9 }}>
              {sceneMode === "dictation" && (
                <>
                  <div>· 单麦克风听写；灵敏 VAD，短句不丢</div>
                  <div>· 角色、翻译和分析插件关闭，资源消耗最低</div>
                </>
              )}
              {sceneMode === "conversation" && (
                <>
                  <div>· 双方使用所选语言实时模型；最短提交 300ms</div>
                  <div>· 按麦克风/系统音频通道标记"我/对方"，不加载声纹模型</div>
                  <div>· 开启术语和简报，关闭翻译</div>
                </>
              )}
              {sceneMode === "bilingual" && (
                <>
                  <div>· 两通道分别使用各自语言的识别模型，不混用</div>
                  <div>· 按通道确定角色，双向实时翻译；不加载声纹模型</div>
                  <div>· macOS 远程通话需选择可捕获系统音频的设备</div>
                </>
              )}
              {sceneMode === "live_translation" && (
                <>
                  <div>· 单流（麦克风），用你选择的语言说话</div>
                  <div>· 实时转写并翻译成目标语言，同时在界面显示原文和译文</div>
                  <div>· 适合实时演示、口译练习或字幕辅助场景</div>
                </>
              )}
              {sceneMode === "meeting" && (
                <>
                  <div>· 两人以上会议，开启 WeSpeaker 在线聚类和段内换人检测</div>
                  <div>· 术语、简报等会议分析开启，翻译默认关闭</div>
                  <div>· 角色识别会增加 CPU 和内存占用</div>
                </>
              )}
              {sceneMode === "lecture" && (
                <>
                  <div>· 单流，严格 VAD；段尾静音 700ms，最长语音 60s</div>
                  <div>· 关闭角色识别；开启术语和简报，适合长时间连续发言</div>
                </>
              )}
              <div style={{ marginTop: 6, color: "var(--muted)" }}>
                内置模板只读；选择「自定义」可修改全部参数。参数在下次开始监听时生效。
              </div>
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              <label style={labelBlock}>
                VAD 灵敏度：
                <select
                  value={sceneCustom.vad_preset}
                  onChange={(e) => setSceneCustom({ ...sceneCustom, vad_preset: e.target.value as SceneParams["vad_preset"] })}
                  style={inputStyle}
                >
                  <option value="standard">标准（平衡灵敏度与抗噪）</option>
                  <option value="sensitive">灵敏（弱语音/短句，会议室轻声）</option>
                  <option value="strict">严格（抗背景噪音，长句稳定）</option>
                </select>
              </label>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                <label>
                  最小语音 ms：
                  <input type="number" min={0} step={50} value={sceneCustom.vad_min_speech_ms ?? 250} onChange={(e) => setSceneCustom({ ...sceneCustom, vad_min_speech_ms: Number(e.target.value) || null })} style={numStyle} />
                </label>
                <label>
                  段尾静音 ms：
                  <input type="number" min={0} step={50} value={sceneCustom.vad_min_silence_ms ?? 500} onChange={(e) => setSceneCustom({ ...sceneCustom, vad_min_silence_ms: Number(e.target.value) || null })} style={numStyle} />
                </label>
                <label>
                  最长语音 ms：
                  <input type="number" min={1000} step={1000} value={sceneCustom.vad_max_speech_ms ?? 30000} onChange={(e) => setSceneCustom({ ...sceneCustom, vad_max_speech_ms: Number(e.target.value) || null })} style={numStyle} />
                </label>
              </div>
              <label style={labelBlock}>
                最短提交时长（噪音短段抑制）：
                <input type="number" min={0} step={100} value={sceneCustom.min_segment_ms} onChange={(e) => setSceneCustom({ ...sceneCustom, min_segment_ms: Math.max(0, Number(e.target.value) || 0) })} style={numStyle} />
                ms（0 = 不限制）
              </label>
              <label style={labelBlock}>
                段级 ASR 最长上下文：
                <input type="number" min={0} max={60000} step={500} value={sceneCustom.asr_segment_ms} onChange={(e) => setSceneCustom({ ...sceneCustom, asr_segment_ms: Math.min(60000, Math.max(0, Number(e.target.value) || 0)) })} style={numStyle} />
                ms（0 = 只按 VAD 自然停顿切段）
              </label>
              <label style={labelBlock}>
                <input type="checkbox" checked={sceneCustom.denoise_enabled} onChange={(e) => setSceneCustom({ ...sceneCustom, denoise_enabled: e.target.checked })} /> 开启降噪（噪声门 + 高通；弱信号环境慎开）
              </label>
              <label style={labelBlock}>
                我的引擎：
                <select value={sceneCustom.user_engine} onChange={(e) => setSceneCustom({ ...sceneCustom, user_engine: e.target.value })} style={inputStyle}>
                  {modelOptions(sceneCustom.user_engine)}
                </select>
              </label>
              <label style={labelBlock}>
                我的语言：
                <select value={sceneCustom.language} onChange={(e) => setSceneCustom({ ...sceneCustom, language: e.target.value as "zh" | "en" })} style={inputStyle}>
                  <option value="zh">中文</option><option value="en">英语</option>
                </select>
              </label>
              <label style={labelBlock}>
                <input type="checkbox" checked={sceneCustom.client_enabled} onChange={(e) => setSceneCustom({ ...sceneCustom, client_enabled: e.target.checked })} /> 启用客户流（双流：系统回环 + 客户引擎）
              </label>
              <label style={labelBlock}>
                客户引擎：
                <select value={sceneCustom.client_engine} onChange={(e) => setSceneCustom({ ...sceneCustom, client_engine: e.target.value })} style={inputStyle}>
                  {modelOptions(sceneCustom.client_engine)}
                </select>
              </label>
              <label style={labelBlock}>
                对方语言：
                <select value={sceneCustom.client_language} onChange={(e) => setSceneCustom({ ...sceneCustom, client_language: e.target.value as SceneParams["client_language"] })} style={inputStyle}>
                  <option value="zh">中文</option><option value="en">英语</option>
                </select>
              </label>
              <label style={labelBlock}>
                翻译策略：
                <select value={sceneCustom.translation_mode} onChange={(e) => setSceneCustom({ ...sceneCustom, translation_mode: e.target.value as SceneParams["translation_mode"] })} style={inputStyle}>
                  <option value="off">关闭</option>
                  <option value="client_to_user">仅对方 → 我的语言</option>
                  <option value="bidirectional">双向翻译</option>
                </select>
              </label>
              <div style={hint}>「离线段级」模型在 VAD 段结束后对整段识别——准确率更高（尤其中英夹杂），但没有逐字增量（partial）；「流式」模型实时增量、延迟低。</div>
              <label style={labelBlock}>
                {pluginMeta
                  .filter((m) => m.analysis)
                  .map((m, i) => (
                    <span key={m.id} style={i === 0 ? undefined : { marginLeft: 12 }}>
                      <input
                        type="checkbox"
                        checked={sceneCustom.plugin_allowlist.includes(m.id)}
                        onChange={(e) =>
                          setSceneCustom({
                            ...sceneCustom,
                            // allowlist：勾上=加进列表，取消=从列表移除
                            plugin_allowlist: e.target.checked
                              ? [...sceneCustom.plugin_allowlist, m.id]
                              : sceneCustom.plugin_allowlist.filter((x) => x !== m.id),
                          })
                        }
                      />{" "}
                      {m.label}
                    </span>
                  ))}
              </label>
              <label style={labelBlock}>
                角色识别：
                <select value={sceneCustom.speaker_mode} onChange={(e) => setSceneCustom({ ...sceneCustom, speaker_mode: e.target.value as SceneParams["speaker_mode"] })} style={inputStyle}>
                  <option value="off">关闭</option>
                  <option value="channel">按输入通道（低资源）</option>
                  <option value="voiceprint">声纹多人识别（较高资源）</option>
                </select>
                <span style={{ marginLeft: 12 }}>
                  <input type="checkbox" checked={sceneCustom.noise_auto_detect} onChange={(e) => setSceneCustom({ ...sceneCustom, noise_auto_detect: e.target.checked })} /> 质量评估自动检测背景噪音
                </span>
              </label>
            </div>
          )}
        </div>
      )}

      {/* ── ASR 转写：选用引擎 + 输入增益；安装模型在独立页 ── */}
      {tab === "asr" && (
        <div>
          <h3 style={groupTitle}>转写引擎</h3>
          <div style={{ display: "flex", gap: 10, marginBottom: 6, flexWrap: "wrap", alignItems: "center" }}>
            <label style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <span style={{ color: "var(--text-2)" }}>中文引擎</span>
              <select
                value={engineZh}
                onChange={(e) => {
                  const v = e.target.value;
                  setEngineZh(v);
                  setSceneCustom((s) => ({ ...s, user_engine: v }));
                }}
                style={inputStyle}
              >
                {modelOptions(engineZh)}
              </select>
            </label>
            <label style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <span style={{ color: "var(--text-2)" }}>英文引擎</span>
              <select
                value={engineEn}
                onChange={(e) => {
                  const v = e.target.value;
                  setEngineEn(v);
                  setSceneCustom((s) => ({ ...s, client_engine: v }));
                }}
                style={inputStyle}
              >
                {modelOptions(engineEn)}
              </select>
            </label>
            <button type="button" onClick={onOpenModels} style={{ fontSize: 12, padding: "4px 10px", cursor: "pointer" }}>
              打开模型管理
            </button>
          </div>
          <div style={hint}>中文引擎用于所有中文场景（听写、会话、会议、课堂等）；英文引擎用于英文场景及双语对话的英文通道。未安装的引擎请到「模型管理」下载。非自定义场景用此全局设置，自定义场景用「场景模式 → 自定义」。</div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>麦克风输入电平</h3>
          <label style={labelBlock}>
            输入增益：
            <input type="number" min={0} max={24} step={1} value={inputGainDb} onChange={(e) => setInputGainDb(Math.min(24, Math.max(0, Number(e.target.value) || 0)))} style={numStyle} /> dB
          </label>
          <div style={hint}>默认 +12dB；无线麦双声道自动选择电平较高的通道，并在放大后限幅，避免与静音通道平均导致声音变小。</div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>语义分句</h3>
          <label style={{ display: "flex", alignItems: "center", gap: 6, cursor: "pointer" }}>
            <input
              type="checkbox"
              checked={punctEnabled}
              onChange={(e) => setPunctEnabled(e.target.checked)}
            />
            <span>启用语义分句（标点恢复模型）</span>
          </label>
          <div style={hint}>在停顿断句基础上，用标点模型识别语义边界并自动拆分；本地流式、离线大模型和云端结果统一生效。</div>
          {punctEnabled && (() => {
            const punctModel = asrModels.find((m) => m.id === "punct");
            if (!punctModel) return null;
            return punctModel.installed ? (
              <div style={{ ...hint, color: "var(--live)" }}>✓ 标点恢复模型已安装</div>
            ) : (
              <div style={{ ...hint, color: "var(--brief)" }}>需要下载中英文标点恢复模型（约 294 MB）。请点击上方「打开模型管理」下载。</div>
            );
          })()}

          {gpuStatus && (
            <div style={{ marginTop: 10, fontSize: 12, color: "var(--text-2)" }}>
              <div>物理硬件：{gpuStatus.hardware_candidate ?? gpuStatus.display_name}</div>
              <div>当前推理后端：<span style={{ color: gpuStatus.is_accelerated ? "var(--live)" : undefined }}>{gpuStatus.display_name}</span></div>
              {gpuStatus.availability_note && <div>说明：{gpuStatus.availability_note}</div>}
              {gpuStatus.effective_route && <div>当前生效路线：{gpuStatus.effective_route}</div>}
              {gpuStatus.route_error && <div style={{ color: "var(--danger, #c33)" }}>配置不可用：{gpuStatus.route_error}</div>}
            </div>
          )}

          <h3 style={{ ...groupTitle, marginTop: 10 }}>ASR 模式</h3>
          <select
            value={asrMode}
            onChange={(e) => setAsrMode(e.target.value)}
            style={{ ...inputStyle, marginBottom: 4 }}
          >
            <option value="auto">自动（有已接入的 GPU 后端用本地，否则云端）</option>
            <option value="local">本地优先</option>
            <option value="cloud">阿里云云端</option>
          </select>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>语言策略</h3>
          <select
            value={languageMode}
            onChange={(e) => setLanguageMode(e.target.value)}
            style={{ ...inputStyle, marginBottom: 4 }}
          >
            <option value="scene">按场景设定（推荐）：中文场景固定识别中文，避免偶尔冒出英文</option>
            <option value="auto">自动检测：模型每段自己判断语言（适合真正中英混说）</option>
          </select>
          <div style={hint}>「按场景设定」会按每条输入流的语言固定 whisper.cpp 解码语言（如双语场景：我的流=中文、对方流=英文）；「自动检测」交给模型逐段判断，短句/口音/专业词时可能漂移。</div>
          {asrMode === "local" && (
            <label style={{ ...labelBlock, marginTop: 6 }}>
              <span>本地推理后端</span>
              <select
                value={asrBackend}
                onChange={(e) => setAsrBackend(e.target.value)}
                style={{ ...inputStyle, display: "block", width: "100%", marginTop: 2 }}
              >
                <option value="auto">自动检测</option>
                <option value="cpu">CPU（诊断/离线）</option>
                <option value="cuda">NVIDIA CUDA</option>
                {import.meta.env.TAURI_ENV_PLATFORM === "windows" && (
                  <option value="vulkan">Vulkan GPU（AMD/Intel/NVIDIA）</option>
                )}
                <option value="metal">Apple Metal（Apple Silicon）</option>
              </select>
            </label>
          )}
          <div style={hint}>
            自动模式：Windows 检测到 Vulkan 用 whisper.cpp GPU（Whisper large-v3-turbo），检测到 NVIDIA CUDA 用 Qwen3-ASR；Apple Silicon 用 Metal + Whisper large-v3-turbo。
          </div>

          {(asrMode === "auto" || asrMode === "cloud") && (
            <div style={{ border: "1px solid var(--border)", borderRadius: 6, padding: 10, marginTop: 6, display: "flex", flexDirection: "column", gap: 6 }}>
              <span style={{ fontSize: 12, fontWeight: 600, color: "var(--text-2)" }}>阿里云实时语音识别</span>
              <label style={labelBlock}>
                <span style={{ fontSize: 11 }}>AccessKey ID</span>
                <input
                  value={aliyunKeyId}
                  onChange={(e) => setAliyunKeyId(e.target.value)}
                  placeholder="LTAI..."
                  style={{ ...inputStyle, display: "block", width: "100%", marginTop: 2 }}
                />
              </label>
              <label style={labelBlock}>
                <span style={{ fontSize: 11 }}>AccessKey Secret</span>
                <input
                  type="password"
                  value={aliyunKeySecret}
                  onChange={(e) => setAliyunKeySecret(e.target.value)}
                  placeholder="••••••••"
                  style={{ ...inputStyle, display: "block", width: "100%", marginTop: 2 }}
                />
              </label>
              <label style={labelBlock}>
                <span style={{ fontSize: 11 }}>AppKey</span>
                <input
                  value={aliyunAppKey}
                  onChange={(e) => setAliyunAppKey(e.target.value)}
                  placeholder="NLS 项目 AppKey"
                  style={{ ...inputStyle, display: "block", width: "100%", marginTop: 2 }}
                />
              </label>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <button
                  onClick={() => void handleTestAliyun()}
                  disabled={aliyunTesting}
                  style={{ fontSize: 11, padding: "3px 10px", cursor: aliyunTesting ? "default" : "pointer", flexShrink: 0 }}
                >
                  {aliyunTesting ? "检查中…" : "检查"}
                </button>
                {aliyunTestResult && (
                  <span style={{ fontSize: 11, color: aliyunTestResult.ok ? "var(--live)" : "var(--danger)" }}>
                    {aliyunTestResult.ok ? `✓ ${aliyunTestResult.text}` : `✗ ${aliyunTestResult.text}`}
                  </span>
                )}
              </div>
              <div style={{ fontSize: 10, color: "var(--muted)" }}>
                检查会向阿里云 NLS 请求一个 AccessToken（CreateToken，HMAC-SHA1 签名）验证 AccessKey 是否有效；无需先保存。
              </div>
            </div>
          )}
        </div>
      )}

      {/* ── 音频处理 ── */}
      {tab === "audio" && (
        <div>
          <h3 style={groupTitle}>采集来源</h3>
          <div style={{ display: "flex", gap: 8, marginBottom: 6 }}>
            {([
              { value: "mic",      icon: "🎙", label: "麦克风", desc: "采集本地麦克风，适合单人讲话、面对面会议" },
              { value: "loopback", icon: "🔊", label: "系统音频", desc: "采集扬声器输出，视频会议时识别对方讲话" },
            ] as const).map(({ value, icon, label, desc }) => (
              <button
                key={value}
                onClick={() => setAudioSource(value)}
                title={desc}
                style={{
                  flex: 1, padding: "10px 8px", borderRadius: 9, cursor: "pointer", textAlign: "left",
                  border: audioSource === value ? "2px solid var(--live)" : "1px solid var(--border)",
                  background: audioSource === value ? "color-mix(in srgb, var(--live) 10%, var(--surface-2))" : "var(--surface-2)",
                  color: "var(--text)",
                }}
              >
                <div style={{ fontSize: 18, marginBottom: 3 }}>{icon} {label}</div>
                <div style={{ fontSize: 11, color: "var(--muted)", lineHeight: 1.4 }}>{desc}</div>
              </button>
            ))}
          </div>
          {audioSource === "loopback" && (
            <div style={{ fontSize: 12, color: "var(--brief)", marginBottom: 6, padding: "6px 10px", background: "color-mix(in srgb, var(--brief) 8%, var(--surface-2))", borderRadius: 7 }}>
              💡 系统音频模式仅支持 Windows，且只采集扬声器输出（对方的声音）。自己的发言不会被采集，适合只需记录对方内容的场景。
            </div>
          )}

          <h3 style={groupTitle}>识别灵敏度（VAD）</h3>
          <select
            value={vadPreset}
            onChange={(e) => setVadPreset(e.target.value)}
            style={{ ...inputStyle, width: "100%", marginBottom: 4 }}
          >
            <option value="standard">标准（平衡灵敏度与抗噪）</option>
            <option value="sensitive">灵敏（弱语音/快速问答，会议室轻声）</option>
            <option value="strict">严格（抗背景噪音，长句稳定）</option>
          </select>
          <label style={labelBlock}>
            <input type="checkbox" checked={endpointEnabled} onChange={(e) => setEndpointEnabled(e.target.checked)} /> 启用自然断句（文本稳定 + 短暂停顿）
          </label>
          {endpointEnabled && (
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 6 }}>
              <label>文本稳定 ms：<input type="number" min={100} step={50} value={endpointStableMs} onChange={(e) => setEndpointStableMs(Math.max(100, Number(e.target.value) || 350))} style={numStyle} /></label>
              <label>短暂停顿 ms：<input type="number" min={100} step={50} value={endpointQuietMs} onChange={(e) => setEndpointQuietMs(Math.max(100, Number(e.target.value) || 450))} style={numStyle} /></label>
              <label>强制停顿 ms：<input type="number" min={200} step={50} value={endpointForceQuietMs} onChange={(e) => setEndpointForceQuietMs(Math.max(200, Number(e.target.value) || 850))} style={numStyle} /></label>
            </div>
          )}
          <div style={hint}>短暂停顿需文本稳定；达到强制停顿时直接提交。两者均可独立于 Silero VAD 断句且不重复推理；关闭后完全使用 VAD。</div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>背景噪音处理</h3>
          <label style={labelBlock}>
            <input type="checkbox" checked={denoiseEnabled} onChange={(e) => setDenoiseEnabled(e.target.checked)} /> 启用降噪（噪声门 + 高通）
          </label>
          <label style={{ ...labelBlock, opacity: denoiseEnabled ? 1 : 0.5 }}>
            <input type="checkbox" checked={highpass} disabled={!denoiseEnabled} onChange={(e) => setHighpass(e.target.checked)} /> 高通滤波（去低频轰鸣/空调声）
          </label>
          <div style={hint}>降噪对弱语音环境有增益；若环境本就安静可关闭以保留细节。</div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>最短提交时长（噪音短段抑制）</h3>
          <label style={labelBlock}>
            短于该时长的 final 段直接丢弃：
            <input
              type="number"
              min={0}
              step={100}
              value={minSegmentMs}
              onChange={(e) => setMinSegmentMs(Math.max(0, Number(e.target.value) || 0))}
              style={numStyle}
            />
            ms（0 = 不限制）
          </label>
          <div style={hint}>噪音会话中偶发的「哒/咔」等短段会污染转写与历史；设为 400~800ms 可在不丢正常语句的前提下滤掉它们（下次监听生效）。</div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>录音保存</h3>
          <label style={labelBlock}>
            <input type="checkbox" checked={recEnabled} onChange={(e) => setRecEnabled(e.target.checked)} /> 监听时保存录音（用户流 + 客户流）
          </label>
          <input
            value={recDir}
            onChange={(e) => setRecDir(e.target.value)}
            placeholder="录音目录（留空 = 数据目录/recordings）"
            style={{ ...inputStyle, width: "100%" }}
          />
          <div style={hint}>
            保存的录音用于历史回放与测试闭环：<code style={{ color: "var(--term)" }}>talksage trim &lt;录音.wav&gt;</code> 去掉静音后，再回放验证转写。
          </div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>会话质量评估（噪音判定）</h3>
          <label style={labelBlock}>
            <input type="checkbox" checked={qAutoDetect} onChange={(e) => setQAutoDetect(e.target.checked)} /> 自动检测背景噪音并自动设置阈值
          </label>
          {qAutoDetect ? (
            <div style={hint}>
              监听时自动测量非语音段的背景噪音水平，并据此设置静音/高能量阈值（静音 = 背景×1.5，高能量 = 背景×5）。无需手工调整。
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 6, fontSize: 12 }}>
              <label>
                文本噪音阈值（0~1，默认 0.45）：
                <input type="number" min={0.05} max={0.95} step={0.05} value={qTextNoise} onChange={(e) => setQTextNoise(Number(e.target.value))} style={numStyle} />
              </label>
              <label>
                静音语音占比下限（默认 0.15）：
                <input type="number" min={0.05} max={0.5} step={0.05} value={qMinRatio} onChange={(e) => setQMinRatio(Number(e.target.value))} style={numStyle} />
              </label>
              <label>
                持续噪音语音占比上限（默认 0.85）：
                <input type="number" min={0.5} max={0.98} step={0.05} value={qMaxRatio} onChange={(e) => setQMaxRatio(Number(e.target.value))} style={numStyle} />
              </label>
              <label>
                静音能量阈值（RMS，默认 0.01）：
                <input type="number" min={0.001} max={0.1} step={0.005} value={qSilenceRms} onChange={(e) => setQSilenceRms(Number(e.target.value))} style={numStyle} />
              </label>
              <label>
                高能量噪音阈值（RMS，默认 0.5）：
                <input type="number" min={0.1} max={1} step={0.05} value={qHighRms} onChange={(e) => setQHighRms(Number(e.target.value))} style={numStyle} />
              </label>
            </div>
          )}
          <div style={hint}>
            噪音/静音会话会自动跳过要点聚合等下游分析，历史详情可见质量标记。自动检测开关与「场景模式 → 自定义」中的同一开关同步（实际生效字段在场景级）。
          </div>
        </div>
      )}

      {/* ── 术语纠错 ── */}
      {tab === "terminology" && (
        <div>
          <h3 style={groupTitle}>专业术语增强</h3>
          <label style={labelBlock}>
            <input type="checkbox" checked={terminologyEnabled} onChange={(e) => setTerminologyEnabled(e.target.checked)} /> 启用会议术语上下文
          </label>
          <textarea
            value={terminologyTerms}
            onChange={(e) => setTerminologyTerms(e.target.value)}
            disabled={!terminologyEnabled}
            placeholder={"每行一个术语，例如：\nTalkSage\nParaformer\n向量数据库"}
            rows={5}
            style={{ ...inputStyle, width: "100%", resize: "vertical", opacity: terminologyEnabled ? 1 : 0.5 }}
          />
          <label style={{ ...labelBlock, opacity: terminologyEnabled ? 1 : 0.5 }}>
            热词强度：
            <input type="number" min={0} max={10} step={0.1} disabled={!terminologyEnabled} value={hotwordScore} onChange={(e) => setHotwordScore(Math.min(10, Math.max(0, Number(e.target.value) || 0)))} style={numStyle} />
          </label>
          <textarea
            value={terminologyCorrections}
            onChange={(e) => setTerminologyCorrections(e.target.value)}
            disabled={!terminologyEnabled}
            placeholder={"常见误识别 => 标准术语，例如：\n怕热佛母 => Paraformer"}
            rows={4}
            style={{ ...inputStyle, width: "100%", resize: "vertical", opacity: terminologyEnabled ? 1 : 0.5 }}
          />
          <div style={hint}>Zipformer 与 Qwen3-ASR 会使用模型热词；Paraformer 使用纠错表。纠错同步作用于 partial/final，不增加模型推理延迟。修改后下次监听生效。</div>
        </div>
      )}

      {/* ── 插件 ── */}
      {tab === "plugins" && (
        <div>
          <h3 style={groupTitle}>插件</h3>
          {pluginMeta.length === 0 ? (
            <div style={hint}>正在读取插件列表…</div>
          ) : (
            <>
              {renderPluginGroup(pluginMeta.filter((m) => m.analysis))}
              <h3 style={{ ...groupTitle, marginTop: 10 }}>基础插件</h3>
              <div style={hint}>不受场景模式约束；关掉会影响转写与会后处理，改动前请确认。</div>
              {renderPluginGroup(pluginMeta.filter((m) => !m.analysis && m.category !== "knowledge_source" && m.id !== "knowledge_obsidian"))}
            </>
          )}

          <h3 style={{ ...groupTitle, marginTop: 10 }}>知识源：Obsidian</h3>
          <label style={labelBlock}>
            <input type="checkbox" checked={kbEnabled} onChange={(e) => setKbEnabled(e.target.checked)} /> 启用本地仓库
          </label>
          <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
            <input
              value={kbFolder}
              onChange={(e) => setKbFolder(e.target.value)}
              placeholder="Obsidian 仓库路径，例如 D:\Obsidian"
              style={{ ...inputStyle, flex: 1 }}
            />
            <button
              type="button"
              onClick={() => void handlePickKbFolder()}
              style={{ fontSize: 12, padding: "4px 10px", cursor: "pointer", flexShrink: 0 }}
            >
              浏览…
            </button>
          </div>
          <div style={hint}>
            保存后立即刷新索引，供会中材料包、纪要和 AI 助手引用。只支持一个 vault 根路径；.obsidian / .trash / .git 不会进索引。
          </div>
        </div>
      )}

      {/* ── Webhook（会议结束推送；借鉴 Call.md workflow-webhook） ── */}
      {tab === "webhooks" && (
        <div>
          <h3 style={groupTitle}>会议结束 Webhook</h3>
          <label style={labelBlock}>
            <input type="checkbox" checked={whEnabled} onChange={(e) => setWhEnabled(e.target.checked)} /> 会议结束后推送结构化数据（n8n / Zapier / CRM 自动化）
          </label>
          <label style={{ ...labelBlock, opacity: whEnabled ? 1 : 0.5 }}>
            目标 URL（每行一个，仅 http/https）：
            <textarea
              value={whUrls}
              onChange={(e) => setWhUrls(e.target.value)}
              disabled={!whEnabled}
              placeholder={"https://hooks.example.com/meeting-ended\nhttps://your-crm.example/api/meetings"}
              rows={4}
              style={{ ...inputStyle, width: "100%", marginTop: 4, fontFamily: "monospace", resize: "vertical" }}
            />
          </label>
          <div style={hint}>
            payload 包含：会议元数据、会话指标（发言占比/语速/提问/健康分）、质量评估、纪要/智能纪要、完整转写。
            安全：调用前做 <b>SSRF 防护</b>（拒绝内网/回环/localhost 地址），配置保存后仍会在每次调用时重新校验。
          </div>
        </div>
      )}

      {/* ── 网络代理 ── */}
      {tab === "network" && (
        <div>
          <h3 style={groupTitle}>代理服务器</h3>
          <label style={labelBlock}>
            代理地址（留空则直连）：
            <input
              type="text"
              value={proxy}
              onChange={(e) => { setProxy(e.target.value); setProxyTestResult(null); }}
              placeholder="http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
              style={{ ...inputStyle, width: "100%", marginTop: 4, fontFamily: "monospace" }}
            />
          </label>
          <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 4 }}>
            <button
              disabled={proxyTesting || !proxy.trim()}
              onClick={async () => {
                setProxyTesting(true);
                setProxyTestResult(null);
                try {
                  const msg = await api.testProxy(proxy.trim());
                  setProxyTestResult({ ok: true, msg });
                } catch (e: unknown) {
                  setProxyTestResult({ ok: false, msg: e instanceof Error ? e.message : String(e) });
                } finally {
                  setProxyTesting(false);
                }
              }}
              style={{ fontSize: 12, padding: "3px 10px", cursor: proxyTesting || !proxy.trim() ? "default" : "pointer" }}
            >
              {proxyTesting ? "测试中…" : "测试"}
            </button>
            {proxyTestResult && (
              <span style={{ fontSize: 12, color: proxyTestResult.ok ? "var(--live)" : "var(--danger)" }}>
                {proxyTestResult.msg}
              </span>
            )}
          </div>
          <div style={{ ...hint, marginTop: 8 }}>
            代理仅对<b>外网请求</b>生效：模型下载（HuggingFace / GitHub）、LLM API、Webhook。
            阿里云 ASR 始终直连，不受此设置影响（国内服务走代理会增加延迟或被拒绝）。
            修改后需点击「保存」，下次启动下载 / 调用 API 时生效。
          </div>
        </div>
      )}

      {/* ── 升级 ── */}
      {tab === "upgrade" && (
        <div>
          <h3 style={groupTitle}>应用升级</h3>
          <div style={{ fontSize: 12, marginBottom: 8 }}>
            当前版本：<b style={{ fontFamily: "monospace" }}>{appVersion ?? "…"}</b>
            {api.transport === "http" && (
              <span style={{ color: "var(--muted)" }}>（headless 浏览器模式不支持升级）</span>
            )}
          </div>
          <div style={{ ...hint, marginBottom: 12 }}>
            离线升级：选择本机已下载的安装包，校验版本（须高于当前）与架构后安装。
            Windows：<code>talksage.ps1 package</code> 产出的 NSIS <code>.exe</code> / MSI，静默安装后请再开应用。
            macOS：<code>talksage.sh package</code> 产出的 <code>.dmg</code> 或 <code>拓思者.app</code>，会替换
            <code>/Applications/拓思者.app</code> 并重新打开。
            在线升级为框架预留：配置更新源与签名公钥后，「检查更新」即可查询新版本。
          </div>
          <h3 style={{ ...groupTitle, marginTop: 10 }}>在线升级</h3>
          <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
            <button
              disabled={upgradeChecking || api.transport === "http"}
              onClick={handleCheckUpdates}
              style={{ fontSize: 12, padding: "3px 10px", cursor: upgradeChecking || api.transport === "http" ? "default" : "pointer" }}
            >
              {upgradeChecking ? "检查中…" : "检查更新"}
            </button>
            {upgradeCheckResult && (
              <span
                style={{
                  fontSize: 12,
                  color: upgradeCheckResult.available
                    ? "var(--live)"
                    : upgradeCheckResult.configured === false
                      ? "var(--muted)"
                      : "var(--text-2)",
                }}
              >
                {upgradeCheckResult.available && upgradeCheckResult.latest_version
                  ? `${upgradeCheckResult.message}（${upgradeCheckResult.current_version} → ${upgradeCheckResult.latest_version}）`
                  : upgradeCheckResult.message}
              </span>
            )}
          </div>
          <h3 style={{ ...groupTitle, marginTop: 10 }}>离线升级</h3>
          <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
            <button
              disabled={upgradeInstalling || api.transport === "http"}
              onClick={handleInstallUpgradePackage}
              style={{ fontSize: 12, padding: "3px 10px", cursor: upgradeInstalling || api.transport === "http" ? "default" : "pointer" }}
            >
              {upgradeInstalling ? "安装中…" : "选择升级包并安装"}
            </button>
            {upgradeInstallResult && (
              <span style={{ fontSize: 12, color: upgradeInstallResult.ok ? "var(--live)" : "var(--danger)" }}>
                {upgradeInstallResult.message}
              </span>
            )}
          </div>
        </div>
      )}

      {/* ── 声音标识 ── */}
      {tab === "voice" && (
        <div>
          <h3 style={groupTitle}>说话人识别（多人区分 + 可选主人识别）</h3>
          <div style={{ display: "flex", flexDirection: "column", gap: 8, marginBottom: 8 }}>
            <div style={{ fontSize: 12 }}>
              声纹模型：
              {voiceStatus === null ? (
                <span style={{ color: "var(--muted)" }}> 检查中…</span>
              ) : voiceStatus.model_available ? (
                <span style={{ color: "var(--live)" }}> 已安装 ✓</span>
              ) : (
                <span style={{ color: "var(--danger)" }}> 未安装（运行 scripts/download_models.py wespeaker）</span>
              )}
            </div>
            <div style={{ fontSize: 12 }}>
              我的声音：
              {voiceStatus === null ? (
                <span style={{ color: "var(--muted)" }}> 检查中…</span>
              ) : voiceStatus.enrolled ? (
                <span style={{ color: "var(--live)" }}> 已注册 ✓ 匹配的讲话者将标记为「我」</span>
              ) : (
                <span style={{ color: "var(--brief)" }}> 未注册</span>
              )}
            </div>
          </div>
          <div style={{ ...hint, marginBottom: 8 }}>
            声音只在本机处理。请保持环境安静，距离麦克风约 20–40 厘米，用平时开会的音量连续朗读。
          </div>
          <div style={{ padding: "14px 16px", borderRadius: 10, border: `1px solid ${enrollStage === "recording" ? "var(--live)" : "var(--border)"}`, background: "var(--surface-2)" }}>
            <div style={{ fontSize: 10, color: "var(--muted)", fontWeight: 700, letterSpacing: "0.08em", marginBottom: 8 }}>
              固定朗读文本
            </div>
            <div style={{ fontSize: 16, lineHeight: 1.9, color: "var(--text)", userSelect: "text" }}>{VOICE_ENROLL_TEXT}</div>
            {enrolling && (
              <div style={{ marginTop: 10 }}>
                <div style={{ fontSize: 12, color: enrollStage === "recording" ? "var(--live)" : "var(--brief)", fontWeight: 700 }}>
                  {enrollStage === "countdown" && `${enrollCount} 秒后开始，请准备`}
                  {enrollStage === "recording" && `● 正在录制，请朗读（剩余约 ${enrollCount} 秒）`}
                  {enrollStage === "processing" && "正在本地检查音频并生成声纹…"}
                </div>
                {enrollStage === "recording" && (
                  <div style={{ height: 5, background: "var(--border)", borderRadius: 3, marginTop: 7, overflow: "hidden" }}>
                    <div style={{ height: "100%", width: `${((VOICE_ENROLL_SECONDS - enrollCount) / VOICE_ENROLL_SECONDS) * 100}%`, background: "var(--live)", transition: "width 1s linear" }} />
                  </div>
                )}
              </div>
            )}
          </div>
          <div style={{ display: "flex", gap: 8, marginTop: 8, alignItems: "center", flexWrap: "wrap" }}>
            <button
              onClick={handleEnroll}
              disabled={enrolling || !voiceStatus?.model_available}
              style={{ fontSize: 12 }}
            >
              {enrolling ? "注册进行中…" : voiceStatus?.enrolled ? "重新录制主人声纹" : "开始录制主人声纹"}
            </button>
            {voiceStatus?.enrolled && (
              <button onClick={handleRemoveVoice} disabled={enrolling} style={{ fontSize: 12 }}>
                删除声音标识
              </button>
            )}
          </div>
          <div style={hint}>主人声纹不是多人区分的前置条件：未注册时仍会聚类为「讲话者 / 客户1 / 客户2」；注册后可把匹配身份稳定标记为「我」。固定文字用于覆盖更多发音特征，无需刻意模仿播音腔。</div>
        </div>
      )}

      {/* ── LLM ── */}
      {tab === "llm" && (
        <div>
          <h3 style={groupTitle}>LLM（术语解释 / 翻译 / 纪要生成）</h3>
          <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
            <select value={defaultProvider} onChange={(e) => setDefaultProvider(e.target.value)} style={inputStyle}>
              {PROVIDERS.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
            <input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={`${defaultProvider} API Key（Ollama 可留空）`}
              style={{ ...inputStyle, flex: 1 }}
            />
            <button
              onClick={() => void handleTestLlm()}
              disabled={llmTesting}
              style={{ fontSize: 12, padding: "4px 10px", cursor: llmTesting ? "default" : "pointer", flexShrink: 0 }}
            >
              {llmTesting ? "检查中…" : "检查"}
            </button>
          </div>
          {llmTestResult && (
            <div style={{ fontSize: 11, marginBottom: 6, color: llmTestResult.ok ? "var(--live)" : "var(--danger)" }}>
              {llmTestResult.ok ? `✓ ${llmTestResult.text}` : `✗ ${llmTestResult.text}`}
            </div>
          )}
          <div style={hint}>未配置密钥时，术语/翻译插件将只做本地检测（不产生最终结果）。「检查」会用当前填写的 Key 向服务商发一个最小请求验证有效性，无需先保存。</div>
        </div>
      )}
      </div>

      {/* 底部操作：固定在面板底部，无需滚到页尾 */}
      <div
        style={{
          marginTop: 10,
          borderTop: "1px solid var(--border)",
          paddingTop: 10,
          display: "flex",
          gap: 8,
          alignItems: "center",
          flexWrap: "wrap",
          flexShrink: 0,
        }}
      >
        {/* 有改动时高亮并报数；没改动时置灰，避免"点了以为存了"和"存了还点"两种误会 */}
        <button
          onClick={handleSave}
          disabled={saving || (baseline !== null && dirtyCount === 0)}
          style={{
            fontSize: 12,
            fontWeight: 600,
            padding: "5px 12px",
            borderRadius: 8,
            cursor: saving || (baseline !== null && dirtyCount === 0) ? "default" : "pointer",
            border: dirtyCount > 0 ? "1px solid var(--me)" : "1px solid var(--border)",
            background: dirtyCount > 0 ? "var(--me)" : "var(--surface-2)",
            color: dirtyCount > 0 ? "#fff" : "var(--muted)",
          }}
        >
          {saving ? "保存中…" : dirtyCount > 0 ? `保存设置（${dirtyCount} 项未保存）` : "保存设置"}
        </button>
        {baseline !== null && dirtyCount === 0 && !saving && !message && (
          <span style={{ fontSize: 11, color: "var(--muted)" }}>已保存</span>
        )}
        {tab === "audio" && (
          <button onClick={handleResetQuality} disabled={saving} style={{ fontSize: 12 }}>
            恢复噪音阈值默认
          </button>
        )}
        {/* 又有新改动后收起"已保存"提示，避免和"N 项未保存"同时出现互相打架 */}
        {message && (dirtyCount === 0 || message.startsWith("保存失败")) && (
          <span style={{ fontSize: 11, color: message.startsWith("保存失败") ? "var(--danger)" : "var(--live)" }}>{message}</span>
        )}
      </div>
    </div>
  );
}
