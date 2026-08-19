// 设置面板：LLM / 插件 / 知识库 / ASR 配置（保存写入 talksage.toml）。

import { useState } from "react";
import type { AppConfig } from "../lib/api";

const PROVIDERS = ["deepseek", "kimi", "minimax", "groq", "ollama", "claude"];

export default function SettingsSection({
  config,
  onSave,
}: {
  config: AppConfig | null;
  onSave: (updates: Record<string, unknown>) => Promise<void>;
}) {
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
  const [recEnabled, setRecEnabled] = useState(config?.recording?.enabled ?? true);
  const [recDir, setRecDir] = useState<string>(config?.recording?.dir ?? "");
  const [qAutoDetect, setQAutoDetect] = useState(config?.quality?.auto_detect ?? true);
  const [qTextNoise, setQTextNoise] = useState(config?.quality?.text_noise_threshold ?? 0.45);
  const [qMinRatio, setQMinRatio] = useState(config?.quality?.min_speech_ratio ?? 0.15);
  const [qMaxRatio, setQMaxRatio] = useState(config?.quality?.max_speech_ratio ?? 0.85);
  const [qSilenceRms, setQSilenceRms] = useState(config?.quality?.silence_rms ?? 0.01);
  const [qHighRms, setQHighRms] = useState(config?.quality?.high_rms ?? 0.5);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");

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
    <div
      style={{
        border: "1px solid var(--border)",
        borderRadius: 8,
        padding: 10,
        fontSize: 12,
      }}
    >
      <h3 style={{ margin: "0 0 8px", fontSize: 13 }}>LLM</h3>
      <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
        <select
          value={defaultProvider}
          onChange={(e) => setDefaultProvider(e.target.value)}
          style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--border)" }}
        >
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
          style={{ flex: 1, padding: "4px 8px", fontSize: 12, borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}
        />
      </div>

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>插件</h3>
      <label style={{ display: "block", marginBottom: 4 }}>
        <input type="checkbox" checked={termEnabled} onChange={(e) => setTermEnabled(e.target.checked)} /> 术语解释
      </label>
      <label style={{ display: "block", marginBottom: 4 }}>
        <input type="checkbox" checked={transEnabled} onChange={(e) => setTransEnabled(e.target.checked)} /> 实时翻译
      </label>
      <label style={{ display: "block", marginBottom: 4 }}>
        <input type="checkbox" checked={briefEnabled} onChange={(e) => setBriefEnabled(e.target.checked)} /> 简报检索
      </label>

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>知识库（客户简报）</h3>
      <label style={{ display: "block", marginBottom: 4 }}>
        <input type="checkbox" checked={kbEnabled} onChange={(e) => setKbEnabled(e.target.checked)} /> 启用
      </label>
      <input
        value={kbFolder}
        onChange={(e) => setKbFolder(e.target.value)}
        placeholder="简报 .md/.txt 文件夹路径"
        style={{ width: "100%", padding: "4px 8px", fontSize: 12, borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)", boxSizing: "border-box" }}
      />

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>ASR</h3>
      <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
        <select value={clientEngine} onChange={(e) => setClientEngine(e.target.value)} style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--border)" }}>
          <option value="zipformer-en">客户（英文）zipformer-en</option>
          <option value="paraformer-zh">客户（英文）paraformer-zh</option>
        </select>
        <select value={userEngine} onChange={(e) => setUserEngine(e.target.value)} style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--border)" }}>
          <option value="paraformer-zh">我（中文）paraformer-zh</option>
          <option value="zipformer-en">我（中文）zipformer-en</option>
        </select>
      </div>

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>识别灵敏度（VAD）</h3>
      <select
        value={vadPreset}
        onChange={(e) => setVadPreset(e.target.value)}
        style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "var(--surface-2)", color: "var(--text)", border: "1px solid var(--border)", width: "100%", boxSizing: "border-box", marginBottom: 4 }}
      >
        <option value="standard">标准（平衡灵敏度与抗噪）</option>
        <option value="sensitive">灵敏（弱语音/快速问答，会议室轻声）</option>
        <option value="strict">严格（抗背景噪音，长句稳定）</option>
      </select>

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>背景噪音处理</h3>
      <label style={{ display: "block", marginBottom: 4 }}>
        <input type="checkbox" checked={denoiseEnabled} onChange={(e) => setDenoiseEnabled(e.target.checked)} /> 启用降噪（噪声门 + 高通）
      </label>
      <label style={{ display: "block", marginBottom: 4, opacity: denoiseEnabled ? 1 : 0.5 }}>
        <input type="checkbox" checked={highpass} disabled={!denoiseEnabled} onChange={(e) => setHighpass(e.target.checked)} /> 高通滤波（去低频轰鸣/空调声）
      </label>

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>会议录音</h3>
      <label style={{ display: "block", marginBottom: 4 }}>
        <input type="checkbox" checked={recEnabled} onChange={(e) => setRecEnabled(e.target.checked)} /> 监听时保存录音（用户流 + 客户流）
      </label>
      <input
        value={recDir}
        onChange={(e) => setRecDir(e.target.value)}
        placeholder="录音目录（留空 = 数据目录/recordings）"
        style={{ width: "100%", padding: "4px 8px", fontSize: 12, borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)", boxSizing: "border-box" }}
      />
      <div style={{ marginTop: 4, color: "var(--muted)", fontSize: 11, lineHeight: 1.6 }}>
        录音用于测试闭环：<code style={{ color: "var(--term)" }}>talksage trim &lt;录音.wav&gt;</code> 去掉静音后，再回放验证转写。
      </div>

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>噪音检测（会话质量评估）</h3>
      <label style={{ display: "block", marginBottom: 6 }}>
        <input type="checkbox" checked={qAutoDetect} onChange={(e) => setQAutoDetect(e.target.checked)} /> 自动检测背景噪音并自动设置阈值
      </label>
      {!qAutoDetect && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, marginBottom: 6, fontSize: 12 }}>
          <label>
            文本噪音阈值（0~1，默认 0.45）：
            <input
              type="number"
              min={0.05}
              max={0.95}
              step={0.05}
              value={qTextNoise}
              onChange={(e) => setQTextNoise(Number(e.target.value))}
              style={{ width: 90, marginLeft: 8, padding: "3px 6px", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}
            />
          </label>
          <label>
            静音语音占比下限（默认 0.15）：
            <input
              type="number"
              min={0.05}
              max={0.5}
              step={0.05}
              value={qMinRatio}
              onChange={(e) => setQMinRatio(Number(e.target.value))}
              style={{ width: 90, marginLeft: 8, padding: "3px 6px", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}
            />
          </label>
          <label>
            持续噪音语音占比上限（默认 0.85）：
            <input
              type="number"
              min={0.5}
              max={0.98}
              step={0.05}
              value={qMaxRatio}
              onChange={(e) => setQMaxRatio(Number(e.target.value))}
              style={{ width: 90, marginLeft: 8, padding: "3px 6px", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}
            />
          </label>
          <label>
            静音能量阈值（RMS，默认 0.01）：
            <input
              type="number"
              min={0.001}
              max={0.1}
              step={0.005}
              value={qSilenceRms}
              onChange={(e) => setQSilenceRms(Number(e.target.value))}
              style={{ width: 90, marginLeft: 8, padding: "3px 6px", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}
            />
          </label>
          <label>
            高能量噪音阈值（RMS，默认 0.5）：
            <input
              type="number"
              min={0.1}
              max={1}
              step={0.05}
              value={qHighRms}
              onChange={(e) => setQHighRms(Number(e.target.value))}
              style={{ width: 90, marginLeft: 8, padding: "3px 6px", borderRadius: 4, border: "1px solid var(--border)", background: "var(--surface-2)", color: "var(--text)" }}
            />
          </label>
        </div>
      )}
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 2 }}>
        <button onClick={handleSave} disabled={saving} style={{ fontSize: 12 }}>
          {saving ? "保存中…" : "保存设置"}
        </button>
        <button onClick={handleResetQuality} disabled={saving} style={{ fontSize: 12 }}>
          恢复噪音阈值默认
        </button>
      </div>
      {message && <div style={{ marginTop: 6, color: message.startsWith("失败") ? "var(--danger)" : "var(--live)" }}>{message}</div>}
    </div>
  );
}
