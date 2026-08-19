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
      };
      await onSave(updates);
      setMessage("已保存（部分设置重启后生效）");
    } catch (e) {
      setMessage(`保存失败: ${e}`);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      style={{
        border: "1px solid rgba(255,255,255,0.08)",
        borderRadius: 8,
        padding: 10,
        fontSize: 12,
        maxHeight: 420,
        overflowY: "auto",
      }}
    >
      <h3 style={{ margin: "0 0 8px", fontSize: 13 }}>LLM</h3>
      <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
        <select
          value={defaultProvider}
          onChange={(e) => setDefaultProvider(e.target.value)}
          style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "#0f172a", color: "#e2e8f0", border: "1px solid #334155" }}
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
          style={{ flex: 1, padding: "4px 8px", fontSize: 12, borderRadius: 4, border: "1px solid #334155", background: "#0f172a", color: "#e2e8f0" }}
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
        style={{ width: "100%", padding: "4px 8px", fontSize: 12, borderRadius: 4, border: "1px solid #334155", background: "#0f172a", color: "#e2e8f0", boxSizing: "border-box" }}
      />

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>ASR</h3>
      <div style={{ display: "flex", gap: 6, marginBottom: 6 }}>
        <select value={clientEngine} onChange={(e) => setClientEngine(e.target.value)} style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "#0f172a", color: "#e2e8f0", border: "1px solid #334155" }}>
          <option value="zipformer-en">客户（英文）zipformer-en</option>
          <option value="paraformer-zh">客户（英文）paraformer-zh</option>
        </select>
        <select value={userEngine} onChange={(e) => setUserEngine(e.target.value)} style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "#0f172a", color: "#e2e8f0", border: "1px solid #334155" }}>
          <option value="paraformer-zh">我（中文）paraformer-zh</option>
          <option value="zipformer-en">我（中文）zipformer-en</option>
        </select>
      </div>

      <h3 style={{ margin: "10px 0 6px", fontSize: 13 }}>识别灵敏度（VAD）</h3>
      <select
        value={vadPreset}
        onChange={(e) => setVadPreset(e.target.value)}
        style={{ fontSize: 12, padding: "3px 6px", borderRadius: 4, background: "#0f172a", color: "#e2e8f0", border: "1px solid #334155", width: "100%", boxSizing: "border-box", marginBottom: 4 }}
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

      <button onClick={handleSave} disabled={saving} style={{ fontSize: 12, marginTop: 4 }}>
        {saving ? "保存中…" : "保存设置"}
      </button>
      {message && <div style={{ marginTop: 6, color: message.startsWith("失败") ? "#f87171" : "#34d399" }}>{message}</div>}
    </div>
  );
}
