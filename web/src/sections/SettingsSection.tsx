// 设置面板：按 Tab 归类（ASR 转写 / 插件分析 / 会议录音 / 噪音检测 / 声音标识 / LLM）。
// 保存写入 talksage.toml。

import { useEffect, useState } from "react";
import type { AppConfig, SceneParams } from "../lib/api";
import { getApi } from "../lib/transport";

const api = getApi();
const PROVIDERS = ["deepseek", "kimi", "minimax", "groq", "ollama", "claude"];

type SettingsTab = "scene" | "asr" | "plugins" | "recording" | "quality" | "voice" | "llm" | "webhooks";

const TABS: { key: SettingsTab; label: string; desc: string }[] = [
  { key: "scene", label: "场景模式", desc: "生活 / 会议 / 会谈 / 自定义" },
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
  const [sceneMode, setSceneMode] = useState<"life" | "meeting" | "talk" | "custom">(config?.scene?.mode ?? "meeting");
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
    term_enabled: config?.scene?.custom?.term_enabled ?? true,
    translation_enabled: config?.scene?.custom?.translation_enabled ?? true,
    brief_enabled: config?.scene?.custom?.brief_enabled ?? true,
    speaker_enabled: config?.scene?.custom?.speaker_enabled ?? false,
    noise_auto_detect: config?.scene?.custom?.noise_auto_detect ?? true,
  }));
  const [defaultProvider, setDefaultProvider] = useState(config?.llm?.default ?? "deepseek");
  const [apiKey, setApiKey] = useState<string>("");
  const [termEnabled, setTermEnabled] = useState(config?.plugins?.term_explainer?.enabled ?? true);
  const [transEnabled, setTransEnabled] = useState(config?.plugins?.translator?.enabled ?? true);
  const [briefEnabled, setBriefEnabled] = useState(config?.plugins?.brief_retriever?.enabled ?? true);
  const [kbFolder, setKbFolder] = useState<string>("");
  const [kbEnabled, setKbEnabled] = useState(false);
  const [clientEngine, setClientEngine] = useState(config?.asr?.client_engine ?? "zipformer-en");
  const [userEngine, setUserEngine] = useState(config?.asr?.user_engine ?? "paraformer-zh");
  const [vadPreset, setVadPreset] = useState<string>(config?.audio?.vad?.preset ?? "standard");
  const [denoiseEnabled, setDenoiseEnabled] = useState(config?.audio?.denoise?.enabled ?? false);
  const [highpass, setHighpass] = useState(config?.audio?.denoise?.highpass ?? true);
  const [minSegmentMs, setMinSegmentMs] = useState<number>(config?.audio?.min_segment_ms ?? 0);
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
  // 声音标识
  const [voiceStatus, setVoiceStatus] = useState<{ model_available: boolean; enrolled: boolean } | null>(null);
  const [enrolling, setEnrolling] = useState(false);
  const [enrollCount, setEnrollCount] = useState(0);

  // 加载声纹状态
  useEffect(() => {
    (async () => {
      try {
        setVoiceStatus(await api.getVoiceprintStatus());
      } catch (e) {
        console.error("读取声纹状态失败:", e);
      }
    })();
  }, []);

  /** 录制主人声音（countdown 秒）并保存声纹。 */
  async function handleEnroll() {
    const seconds = 6;
    setEnrolling(true);
    setMessage("");
    setEnrollCount(seconds);
    // 倒计时 UI
    const timer = setInterval(() => setEnrollCount((c) => c - 1), 1000);
    try {
      const r = await api.enrollVoice(seconds);
      setVoiceStatus({ model_available: true, enrolled: true });
      setMessage(`声音标识已保存（声纹维度 ${r.dim}）。监听时将优先识别为「我」。`);
    } catch (e) {
      setMessage(`声音标识失败: ${e}`);
    } finally {
      clearInterval(timer);
      setEnrollCount(0);
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

  async function handleSave() {
    setSaving(true);
    setMessage("");
    try {
      const updates: Record<string, unknown> = {
        llm: {
          default: defaultProvider,
          providers: {
            [defaultProvider]: {
              api_key: apiKey.trim(),
            },
          },
        },
        plugins: {
          term_explainer: { enabled: termEnabled },
          translator: { enabled: transEnabled },
          brief_retriever: { enabled: briefEnabled },
        },
        knowledge_base: {
          enabled: kbEnabled,
          folder: kbFolder.trim(),
        },
        asr: {
          client_engine: clientEngine,
          user_engine: userEngine,
        },
        audio: {
          vad: { preset: vadPreset },
          denoise: {
            enabled: denoiseEnabled,
            highpass,
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
                { key: "life", label: "生活", desc: "日常对话：灵敏 VAD 抓短句弱语音，单流，关分析插件" },
                { key: "meeting", label: "会议", desc: "正式会议：双流（我+客户），分析插件全开（默认）" },
                { key: "talk", label: "会谈", desc: "商务洽谈：双流，术语/翻译/简报全开，300ms 短段提交" },
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
              {sceneMode === "life" && (
                <>
                  <div>· VAD：灵敏（threshold 0.35 / 最小语音 150ms / 段尾静音 300ms）——抓日常短句与弱语音</div>
                  <div>· 降噪：关闭（弱信号优先）</div>
                  <div>· 最短提交：不限制（生活短句不丢）</div>
                  <div>· 单流（仅中文 paraformer）；分析插件关闭（省资源）</div>
                  <div>· 说话人识别：默认关闭（可在「自定义」开启）</div>
                </>
              )}
              {sceneMode === "meeting" && (
                <>
                  <div>· VAD：标准（threshold 0.5 / 最小语音 250ms / 段尾静音 500ms）</div>
                  <div>· 降噪：关闭；最短提交：不限制</div>
                  <div>· 双流：我（中文 paraformer）+ 客户（英文 zipformer，系统回环）</div>
                  <div>· 术语解释 / 实时翻译 / 简报检索全开</div>
                  <div>· 说话人识别：默认关闭（可在「自定义」开启）</div>
                </>
              )}
              {sceneMode === "talk" && (
                <>
                  <div>· VAD：标准（threshold 0.5 / 最小语音 250ms / 段尾静音 500ms）</div>
                  <div>· 降噪：关闭；最短提交：300ms（滤掉谈判中的无效短音）</div>
                  <div>· 双流：我 + 客户；术语 / 翻译 / 简报全开</div>
                  <div>· 说话人识别：默认关闭（可在「自定义」开启）</div>
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
                  <input type="number" min={1000} step={1000} value={sceneCustom.vad_max_speech_ms ?? 10000} onChange={(e) => setSceneCustom({ ...sceneCustom, vad_max_speech_ms: Number(e.target.value) || null })} style={numStyle} />
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
                  <option value="paraformer-zh">中文 paraformer-zh（流式，含 fp32 更准）</option>
                  <option value="whisper-base">Whisper base（离线段级，多语言更准）</option>
                  <option value="whisper-small">Whisper small（离线段级，更准更慢）</option>
                  <option value="qwen3-asr">Qwen3-ASR（离线段级，多语言）</option>
                  <option value="zipformer-en">英文 zipformer-en（流式）</option>
                </select>
              </label>
              <label style={labelBlock}>
                <input type="checkbox" checked={sceneCustom.client_enabled} onChange={(e) => setSceneCustom({ ...sceneCustom, client_enabled: e.target.checked })} /> 启用客户流（双流：系统回环 + 客户引擎）
              </label>
              <label style={labelBlock}>
                客户引擎：
                <select value={sceneCustom.client_engine} onChange={(e) => setSceneCustom({ ...sceneCustom, client_engine: e.target.value })} style={inputStyle}>
                  <option value="zipformer-en">英文 zipformer-en（流式）</option>
                  <option value="whisper-base">Whisper base（离线段级，多语言）</option>
                  <option value="paraformer-zh">中文 paraformer-zh（流式）</option>
                </select>
              </label>
              <div style={hint}>「离线段级」模型在 VAD 段结束后对整段识别——准确率更高（尤其中英夹杂），但没有逐字增量（partial）；「流式」模型实时增量、延迟低。</div>
              <label style={labelBlock}>
                <input type="checkbox" checked={sceneCustom.term_enabled} onChange={(e) => setSceneCustom({ ...sceneCustom, term_enabled: e.target.checked })} /> 术语解释
                <span style={{ marginLeft: 12 }}>
                  <input type="checkbox" checked={sceneCustom.translation_enabled} onChange={(e) => setSceneCustom({ ...sceneCustom, translation_enabled: e.target.checked })} /> 实时翻译
                </span>
                <span style={{ marginLeft: 12 }}>
                  <input type="checkbox" checked={sceneCustom.brief_enabled} onChange={(e) => setSceneCustom({ ...sceneCustom, brief_enabled: e.target.checked })} /> 简报检索
                </span>
              </label>
              <label style={labelBlock}>
                <input type="checkbox" checked={sceneCustom.speaker_enabled} onChange={(e) => setSceneCustom({ ...sceneCustom, speaker_enabled: e.target.checked })} /> 说话人识别
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
              <option value="zipformer-en">客户（英文）zipformer-en</option>
              <option value="paraformer-zh">客户（英文）paraformer-zh</option>
            </select>
            <select value={userEngine} onChange={(e) => setUserEngine(e.target.value)} style={inputStyle}>
              <option value="paraformer-zh">我（中文）paraformer-zh</option>
              <option value="zipformer-en">我（中文）zipformer-en</option>
            </select>
          </div>

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
          <label style={labelBlock}>
            <input type="checkbox" checked={termEnabled} onChange={(e) => setTermEnabled(e.target.checked)} /> 术语解释
          </label>
          <label style={labelBlock}>
            <input type="checkbox" checked={transEnabled} onChange={(e) => setTransEnabled(e.target.checked)} /> 实时翻译
          </label>
          <label style={labelBlock}>
            <input type="checkbox" checked={briefEnabled} onChange={(e) => setBriefEnabled(e.target.checked)} /> 简报检索
          </label>

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
          <h3 style={groupTitle}>说话人识别（先识别主人，再区分其他人）</h3>
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
                <span style={{ color: "var(--live)" }}> 已注册 ✓ 监听时您的发言将标记为「我」</span>
              ) : (
                <span style={{ color: "var(--brief)" }}> 未注册</span>
              )}
            </div>
          </div>
          <div style={hint}>
            点击「录制我的声音」，对着麦克风正常说话 6 秒（保持环境安静）。之后监听时：
            您的发言标记为「我」，其他说话人自动区分为「客户1」「客户2」…
          </div>
          <div style={{ display: "flex", gap: 8, marginTop: 8, alignItems: "center", flexWrap: "wrap" }}>
            <button
              onClick={handleEnroll}
              disabled={enrolling || !voiceStatus?.model_available}
              style={{ fontSize: 12 }}
            >
              {enrolling ? `录制中… ${enrollCount}s` : voiceStatus?.enrolled ? "重新录制我的声音" : "录制我的声音"}
            </button>
            {voiceStatus?.enrolled && (
              <button onClick={handleRemoveVoice} disabled={enrolling} style={{ fontSize: 12 }}>
                删除声音标识
              </button>
            )}
          </div>
          <div style={hint}>未注册声音时保持原双流标签（我 / 客户）；录音仍可用于测试闭环。</div>
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
