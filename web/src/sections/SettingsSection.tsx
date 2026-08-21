// 设置面板：按 Tab 归类（ASR 转写 / 插件分析 / 会议录音 / 噪音检测 / 声音标识 / LLM）。
// 保存写入 talksage.toml。

import { useEffect, useState } from "react";
import type { AppConfig, AsrModelInfo, PluginMeta, PluginStatusInfo, SceneMode, SceneParams } from "../lib/api";
import type { PluginValues } from "../lib/plugins";
import { analysisPluginIds, buildPluginUpdates, fieldLabel, initialPluginValues, pluginFields, pluginStatusLabel } from "../lib/plugins";
import { getApi } from "../lib/transport";

const api = getApi();
const PROVIDERS = ["deepseek", "kimi", "minimax", "groq", "ollama", "claude"];
const VOICE_ENROLL_SECONDS = 16;
const VOICE_ENROLL_TEXT =
  "你好，我正在为拓思者录制声音标识。今天阳光明亮，我会清楚、自然、连续地读完这段文字。会议结束后，请帮我整理重点、时间和下一步行动。";

type SettingsTab = "scene" | "asr" | "plugins" | "recording" | "quality" | "voice" | "llm" | "webhooks";

const TABS: { key: SettingsTab; label: string; desc: string }[] = [
  { key: "scene", label: "场景模式", desc: "听写 / 会话 / 双语 / 会议 / 课堂 / 自定义" },
  { key: "asr", label: "ASR 转写", desc: "引擎 / 灵敏度 / 降噪" },
  { key: "plugins", label: "插件分析", desc: "术语 / 翻译 / 简报 / 知识库" },
  { key: "recording", label: "会议录音", desc: "录音开关与目录" },
  { key: "quality", label: "噪音检测", desc: "会话质量阈值" },
  { key: "voice", label: "声音标识", desc: "注册主人声音，识别说话人" },
  { key: "llm", label: "LLM", desc: "默认模型与密钥" },
  { key: "webhooks", label: "Webhook", desc: "会议结束推送（n8n/Zapier/CRM）" },
];

export default function SettingsSection({
  config,
  onSave,
}: {
  config: AppConfig | null;
  onSave: (updates: Record<string, unknown>) => Promise<void>;
}) {
  const [tab, setTab] = useState<SettingsTab>("asr");
  // 场景模式
  const [sceneMode, setSceneMode] = useState<SceneMode>(config?.scene?.mode ?? "conversation");
  const [sceneCustom, setSceneCustom] = useState<SceneParams>(() => ({
    vad_preset: config?.scene?.custom?.vad_preset ?? "standard",
    vad_threshold: config?.scene?.custom?.vad_threshold ?? null,
    vad_min_speech_ms: config?.scene?.custom?.vad_min_speech_ms ?? null,
    vad_min_silence_ms: config?.scene?.custom?.vad_min_silence_ms ?? null,
    vad_max_speech_ms: config?.scene?.custom?.vad_max_speech_ms ?? null,
    denoise_enabled: config?.scene?.custom?.denoise_enabled ?? false,
    denoise_gate: config?.scene?.custom?.denoise_gate ?? 0.008,
    min_segment_ms: config?.scene?.custom?.min_segment_ms ?? 0,
    user_engine: config?.scene?.custom?.user_engine ?? "paraformer-zh",
    client_enabled: config?.scene?.custom?.client_enabled ?? true,
    client_engine: config?.scene?.custom?.client_engine ?? "zipformer-en",
    user_language: config?.scene?.custom?.user_language ?? "zh",
    client_language: config?.scene?.custom?.client_language ?? "en",
    translation_mode: config?.scene?.custom?.translation_mode ?? "off",
    // 配置没给 allowlist 时，元数据到货后回填「全部分析类插件」（见下方 effect）——
    // 这里不能写死 id 列表：哪些插件算分析类归 Rust descriptor。
    plugin_allowlist: config?.scene?.custom?.plugin_allowlist ?? [],
    speaker_mode: config?.scene?.custom?.speaker_mode ?? "channel",
    noise_auto_detect: config?.scene?.custom?.noise_auto_detect ?? true,
  }));
  const [defaultProvider, setDefaultProvider] = useState(config?.llm?.default ?? "deepseek");
  const [apiKey, setApiKey] = useState<string>("");
  // 插件表单由 /plugins 元数据生成：设置页不认识任何具体插件。
  const [pluginMeta, setPluginMeta] = useState<PluginMeta[]>([]);
  const [pluginStatus, setPluginStatus] = useState<PluginStatusInfo[]>([]);
  const [pluginValues, setPluginValues] = useState<PluginValues>({});
  const [kbFolder, setKbFolder] = useState<string>("");
  const [kbEnabled, setKbEnabled] = useState(false);
  const [clientEngine, setClientEngine] = useState(config?.asr?.client_engine ?? "zipformer-en");
  const [userEngine, setUserEngine] = useState(config?.asr?.user_engine ?? "paraformer-zh");
  const [terminologyEnabled, setTerminologyEnabled] = useState(config?.asr?.terminology?.enabled ?? false);
  const [hotwordScore, setHotwordScore] = useState(config?.asr?.terminology?.hotword_score ?? 1.5);
  const [terminologyTerms, setTerminologyTerms] = useState((config?.asr?.terminology?.terms ?? []).join("\n"));
  const [terminologyCorrections, setTerminologyCorrections] = useState(
    Object.entries(config?.asr?.terminology?.corrections ?? {}).map(([wrong, right]) => `${wrong} => ${right}`).join("\n")
  );
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
  const [qAutoDetect, setQAutoDetect] = useState(config?.quality?.auto_detect ?? true);
  const [qTextNoise, setQTextNoise] = useState(config?.quality?.text_noise_threshold ?? 0.45);
  const [qMinRatio, setQMinRatio] = useState(config?.quality?.min_speech_ratio ?? 0.15);
  const [qMaxRatio, setQMaxRatio] = useState(config?.quality?.max_speech_ratio ?? 0.85);
  const [qSilenceRms, setQSilenceRms] = useState(config?.quality?.silence_rms ?? 0.01);
  const [qHighRms, setQHighRms] = useState(config?.quality?.high_rms ?? 0.5);
  const [whEnabled, setWhEnabled] = useState(config?.webhooks?.enabled ?? false);
  const [whUrls, setWhUrls] = useState<string>((config?.webhooks?.urls ?? []).join("\n"));
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");
  const [asrModels, setAsrModels] = useState<AsrModelInfo[]>([]);
  // 声音标识
  const [voiceStatus, setVoiceStatus] = useState<{ model_available: boolean; enrolled: boolean } | null>(null);
  const [enrolling, setEnrolling] = useState(false);
  const [enrollCount, setEnrollCount] = useState(0);
  const [enrollStage, setEnrollStage] = useState<"idle" | "countdown" | "recording" | "processing">("idle");

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
      const [voice, models, metas, statuses] = await Promise.all([
        api.getVoiceprintStatus().catch(warn("声纹状态")),
        api.listAsrModels().catch(warn("ASR 模型列表")),
        api.listPlugins().catch(warn("插件元数据")),
        api.listPluginStatus().catch(warn("插件状态")),
      ]);
      if (voice) setVoiceStatus(voice);
      if (models) setAsrModels(models);
      if (statuses) setPluginStatus(statuses);
      if (!metas) return;
      setPluginMeta(metas);
      setPluginValues(initialPluginValues(metas, config?.plugins));
      // 配置里没有 allowlist（老配置 / headless 的 /config 不返回 scene）时，
      // 按元数据回填成「分析类插件全开」—— 与阶段 5 之前的默认行为一致。
      if (!config?.scene?.custom?.plugin_allowlist) {
        setSceneCustom((s) => ({ ...s, plugin_allowlist: analysisPluginIds(metas) }));
      }
    })();
    // 只在挂载时跑一次：config 也只在挂载时读（本组件其余 state 同一约定），
    // 设置页是从 App 的 navPage 切进来的，切进来就是一次新的挂载。
  }, []);

  /** 改某个插件的某个配置键。 */
  function setPluginField(id: string, key: string, value: boolean | number | string) {
    setPluginValues((prev) => ({ ...prev, [id]: { ...prev[id], [key]: value } }));
  }

  const modelOptions = (selected: string) => asrModels.map((m) => {
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
      const on = values.enabled !== false;
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
            const dim = locked || !on;
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

  async function handleSave() {
    setSaving(true);
    setMessage("");
    try {
      const corrections = Object.fromEntries(terminologyCorrections.split("\n").map((line) => {
        const [wrong, ...right] = line.split("=>");
        return [wrong?.trim(), right.join("=>").trim()];
      }).filter(([wrong, right]) => wrong && right));
      const updates: Record<string, unknown> = {
        llm: {
          default: defaultProvider,
          providers: {
            [defaultProvider]: {
              api_key: apiKey.trim(),
            },
          },
        },
        // 按元数据组装，键与值都来自 /plugins —— 组件里没有插件名
        plugins: buildPluginUpdates(pluginMeta, pluginValues),
        knowledge_base: {
          enabled: kbEnabled,
          folder: kbFolder.trim(),
        },
        asr: {
          client_engine: clientEngine,
          user_engine: userEngine,
          terminology: {
            enabled: terminologyEnabled,
            hotword_score: hotwordScore,
            terms: terminologyTerms.split("\n").map((v) => v.trim()).filter(Boolean),
            corrections,
          },
        },
        audio: {
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
        quality: {
          auto_detect: qAutoDetect,
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
        scene: {
          mode: sceneMode,
          custom: sceneCustom,
        },
      };
      await onSave(updates);
      setPluginStatus(await api.listPluginStatus());
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
      setQAutoDetect(true);
      setQTextNoise(0.45);
      setQMinRatio(0.15);
      setQMaxRatio(0.85);
      setQSilenceRms(0.01);
      setQHighRms(0.5);
      setMessage("噪音检测阈值已恢复默认");
    } catch (e) {
      setMessage(`恢复默认失败: ${e}`);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div style={{ border: "1px solid var(--border)", borderRadius: 8, padding: 10, fontSize: 12 }}>
      {/* Tab 导航 */}
      <div style={{ display: "flex", gap: 6, marginBottom: 12, flexWrap: "wrap" }}>
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
          </button>
        ))}
      </div>

      {/* ── 场景模式 ── */}
      {tab === "scene" && (
        <div>
          <h3 style={groupTitle}>场景模式</h3>
          <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginBottom: 8 }}>
            {(
              [
                { key: "dictation", label: "单人听写", desc: "单麦克风、灵敏 VAD、最低资源消耗" },
                { key: "conversation", label: "一对一会话", desc: "双人会话，按输入通道区分双方，不运行声纹模型" },
                { key: "translation", label: "双语对话", desc: "中文与英文双通道识别，双向实时翻译" },
                { key: "meeting", label: "多人会议", desc: "两人以上，启用 WeSpeaker 在线角色识别" },
                { key: "lecture", label: "演讲/课堂", desc: "长段单流，术语和简报增强，关闭角色识别" },
                { key: "custom", label: "自定义", desc: "使用下方全部参数" },
              ] as const
            ).map((m) => (
              <button
                key={m.key}
                onClick={() => setSceneMode(m.key)}
                title={m.desc}
                style={{
                  padding: "6px 14px",
                  borderRadius: 8,
                  border: "1px solid var(--border)",
                  cursor: "pointer",
                  font: "inherit",
                  fontSize: 12,
                  fontWeight: 600,
                  background: sceneMode === m.key ? "var(--me)" : "var(--surface-2)",
                  color: sceneMode === m.key ? "#fff" : "var(--text-2)",
                }}
              >
                {m.label}
              </button>
            ))}
          </div>

          {sceneMode !== "custom" ? (
            <div style={{ background: "var(--surface-2)", borderRadius: 6, padding: 8, fontSize: 11, color: "var(--text-2)", lineHeight: 1.9 }}>
              {sceneMode === "dictation" && (
                <>
                  <div>· 单麦克风中文听写；灵敏 VAD，短句不丢</div>
                  <div>· 角色、翻译和分析插件关闭，资源消耗最低</div>
                </>
              )}
              {sceneMode === "conversation" && (
                <>
                  <div>· 双方默认使用中文实时模型；最短提交 300ms</div>
                  <div>· 按麦克风/系统音频通道标记“我/对方”，不加载声纹模型</div>
                  <div>· 开启术语和简报，关闭翻译</div>
                </>
              )}
              {sceneMode === "translation" && (
                <>
                  <div>· 我的通道：中文 Paraformer；对方通道：英文 Zipformer</div>
                  <div>· 双向中英翻译；按通道确定角色，不加载声纹模型</div>
                  <div>· macOS 远程通话需选择可捕获系统音频的设备；共享麦克风暂不自动分语言</div>
                </>
              )}
              {sceneMode === "meeting" && (
                <>
                  <div>· 两人以上会议，开启 WeSpeaker 在线聚类和段内换人检测</div>
                  <div>· 双方默认中文；术语、简报等会议分析开启，翻译默认关闭</div>
                  <div>· 角色识别会增加 CPU 和内存占用</div>
                </>
              )}
              {sceneMode === "lecture" && (
                <>
                  <div>· 单流中文，严格 VAD；段尾静音 700ms，最长语音 60s</div>
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
                <select value={sceneCustom.user_language} onChange={(e) => setSceneCustom({ ...sceneCustom, user_language: e.target.value as SceneParams["user_language"] })} style={inputStyle}>
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

      {/* ── ASR 转写 ── */}
      {tab === "asr" && (
        <div>
          <h3 style={groupTitle}>转写引擎</h3>
          <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
            <select value={clientEngine} onChange={(e) => setClientEngine(e.target.value)} style={inputStyle}>
              {modelOptions(clientEngine)}
            </select>
            <select value={userEngine} onChange={(e) => setUserEngine(e.target.value)} style={inputStyle}>
              {modelOptions(userEngine)}
            </select>
          </div>
          <div style={hint}>实时模型持续输出字幕；平衡/准确优先模型在 VAD 段结束后输出，延迟更高。未完整安装的模型不可选择。</div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>麦克风输入电平</h3>
          <label style={labelBlock}>
            输入增益：
            <input type="number" min={0} max={24} step={1} value={inputGainDb} onChange={(e) => setInputGainDb(Math.min(24, Math.max(0, Number(e.target.value) || 0)))} style={numStyle} /> dB
          </label>
          <div style={hint}>默认 +12dB；无线麦双声道自动选择电平较高的通道，并在放大后限幅，避免与静音通道平均导致声音变小。</div>

          <h3 style={{ ...groupTitle, marginTop: 10 }}>专业术语增强</h3>
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

          <h3 style={{ ...groupTitle, marginTop: 10 }}>识别灵敏度（VAD）</h3>
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
          <div style={hint}>噪音会话中偶发的"哒/咔"等短段会污染转写与历史；设为 400~800ms 可在不丢正常语句的前提下滤掉它们（下次监听生效）。</div>
        </div>
      )}

      {/* ── 插件分析 ── */}
      {tab === "plugins" && (
        <div>
          <h3 style={groupTitle}>会议分析插件</h3>
          {pluginMeta.length === 0 ? (
            <div style={hint}>正在读取插件列表…</div>
          ) : (
            <>
              {renderPluginGroup(pluginMeta.filter((m) => m.analysis))}
              <h3 style={{ ...groupTitle, marginTop: 10 }}>基础插件</h3>
              <div style={hint}>不受场景模式约束；关掉会影响转写与会后处理，改动前请确认。</div>
              {renderPluginGroup(pluginMeta.filter((m) => !m.analysis))}
            </>
          )}

          <h3 style={{ ...groupTitle, marginTop: 10 }}>知识库（客户简报）</h3>
          <label style={labelBlock}>
            <input type="checkbox" checked={kbEnabled} onChange={(e) => setKbEnabled(e.target.checked)} /> 启用
          </label>
          <input
            value={kbFolder}
            onChange={(e) => setKbFolder(e.target.value)}
            placeholder="简报 .md/.txt 文件夹路径"
            style={{ ...inputStyle, width: "100%" }}
          />
          <div style={hint}>知识库命中后，客户发言的相关简报显示在右侧「知识库命中」卡片。</div>
        </div>
      )}

      {/* ── 会议录音 ── */}
      {tab === "recording" && (
        <div>
          <h3 style={groupTitle}>会议录音</h3>
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
            录音用于测试闭环：<code style={{ color: "var(--term)" }}>talksage trim &lt;录音.wav&gt;</code> 去掉静音后，再回放验证转写。
          </div>
        </div>
      )}

      {/* ── 噪音检测 ── */}
      {tab === "quality" && (
        <div>
          <h3 style={groupTitle}>会话质量评估</h3>
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
          <div style={hint}>噪音/静音会话会自动跳过要点聚合等下游分析，历史详情可见质量标记。</div>
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
          </div>
          <div style={hint}>未配置密钥时，术语/翻译插件将只做本地检测（不产生最终结果）。</div>
        </div>
      )}

      {/* 底部操作 */}
      <div
        style={{
          marginTop: 14,
          borderTop: "1px solid var(--border)",
          paddingTop: 10,
          display: "flex",
          gap: 8,
          alignItems: "center",
          flexWrap: "wrap",
        }}
      >
        <button onClick={handleSave} disabled={saving} style={{ fontSize: 12 }}>
          {saving ? "保存中…" : "保存设置"}
        </button>
        {tab === "quality" && (
          <button onClick={handleResetQuality} disabled={saving} style={{ fontSize: 12 }}>
            恢复噪音阈值默认
          </button>
        )}
        {message && (
          <span style={{ fontSize: 11, color: message.startsWith("失败") ? "var(--danger)" : "var(--live)" }}>{message}</span>
        )}
      </div>
    </div>
  );
}
