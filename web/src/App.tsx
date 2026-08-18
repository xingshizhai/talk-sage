import { useCallback, useEffect, useRef, useState } from "react";
import { getApi } from "./lib/transport";
import type { AppConfig, DomainEvent } from "./lib/api";
import { TranscriptAccumulator, type TranscriptLine } from "./lib/transcript";
import TranscriptSection from "./sections/TranscriptSection";
import TermsSection, { type TermItem } from "./sections/TermsSection";
import TranslationSection, { type TranslationItem } from "./sections/TranslationSection";
import BriefSection, { type BriefItem } from "./sections/BriefSection";

const api = getApi();

export default function App() {
  const [version, setVersion] = useState<string>("—");
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [listening, setListening] = useState(false);
  const [status, setStatus] = useState<string>("待机");
  const [lines, setLines] = useState<TranscriptLine[]>([]);
  const [terms, setTerms] = useState<TermItem[]>([]);
  const [translations, setTranslations] = useState<TranslationItem[]>([]);
  const [briefs, setBriefs] = useState<BriefItem[]>([]);
  const [rawEvents, setRawEvents] = useState<string[]>([]);
  const [pong, setPong] = useState<string>("");
  const accumulatorRef = useRef(new TranscriptAccumulator());

  useEffect(() => {
    api.getVersion().then(setVersion).catch(console.error);
    api.getConfig().then(setConfig).catch(console.error);
    const off = api.onEvent((ev: DomainEvent) => {
      // 状态事件
      if (ev.type === "status") {
        setStatus(ev.message);
        if (ev.stage === "recording") setListening(true);
        if (ev.stage === "idle" || ev.stage === "asr_ready") setListening(false);
      }
      // 转写事件 → 聚合行
      if (ev.type === "segment") {
        const acc = accumulatorRef.current;
        acc.push(ev);
        setLines([...acc.getLines()]);
      }
      // 术语：骨架插入，final 按 result_id 原地更新
      if (ev.type === "term") {
        setTerms((prev) => {
          const idx = prev.findIndex((t) => t.resultId === ev.result_id);
          if (idx >= 0) {
            const next = [...prev];
            next[idx] = { resultId: ev.result_id, content: ev.content, isFinal: ev.status === "final" };
            return next;
          }
          return [...prev, { resultId: ev.result_id, content: ev.content, isFinal: ev.status === "final" }];
        });
      }
      // 翻译
      if (ev.type === "translation") {
        setTranslations((prev) => [
          ...prev,
          { resultId: ev.result_id, direction: ev.direction, content: ev.content },
        ]);
      }
      // 简报
      if (ev.type === "brief") {
        setBriefs((prev) => [...prev, { source: ev.source, text: ev.text }]);
      }
      // 调试事件流
      setRawEvents((prev) => [...prev.slice(-19), `${ev.type}: ${JSON.stringify(ev).slice(0, 100)}`]);
    });
    return off;
  }, []);

  const handleListen = useCallback(async () => {
    try {
      if (listening) {
        await api.stopListen();
        setListening(false);
        setStatus("已停止");
      } else {
        setStatus("启动中…");
        await api.startListen();
        // 状态由事件流更新（asr_loading → asr_ready → recording）
      }
    } catch (e) {
      setStatus(`错误: ${e}`);
    }
  }, [listening]);

  async function handlePing() {
    try {
      await api.ping();
      setPong("已发送 ping（Rust 侧应推送事件）");
    } catch (e) {
      setPong(`ping 失败: ${e}`);
    }
  }

  return (
    <main style={{ padding: 16, fontFamily: "system-ui, sans-serif", maxWidth: 460 }}>
      <h1 style={{ fontSize: 18 }}>TalkSage v2 — M1</h1>
      <p style={{ color: "#666" }}>
        载体: <b>{api.transport}</b> · 版本: <b>{version}</b> · 状态: <b>{status}</b>
      </p>

      <section style={{ marginTop: 12, display: "flex", gap: 8 }}>
        <button
          onClick={handleListen}
          style={{
            flex: 1,
            padding: "10px 0",
            background: listening ? "#ef4444" : "#10b981",
            color: "#fff",
            border: "none",
            borderRadius: 8,
            fontWeight: 600,
            cursor: "pointer",
          }}
        >
          {listening ? "⏹ 停止监听" : "▶ 开始监听"}
        </button>
        <button onClick={handlePing}>ping</button>
        <span style={{ fontSize: 11, color: "#64748b", alignSelf: "center" }}>{pong}</span>
      </section>

      <section style={{ marginTop: 12 }}>
        <h2 style={{ fontSize: 13, margin: "0 0 6px" }}>实时转写</h2>
        <TranscriptSection lines={lines} />
      </section>

      <section style={{ marginTop: 12 }}>
        <h2 style={{ fontSize: 13, margin: "0 0 6px" }}>术语</h2>
        <TermsSection items={terms} />
      </section>

      <section style={{ marginTop: 12 }}>
        <h2 style={{ fontSize: 13, margin: "0 0 6px" }}>实时翻译</h2>
        <TranslationSection items={translations} />
      </section>

      <section style={{ marginTop: 12 }}>
        <h2 style={{ fontSize: 13, margin: "0 0 6px" }}>简报</h2>
        <BriefSection items={briefs} />
      </section>

      <section style={{ marginTop: 12 }}>
        <h2 style={{ fontSize: 13, margin: "0 0 6px" }}>配置快照</h2>
        <pre style={{ background: "#f5f5f5", padding: 10, borderRadius: 6, fontSize: 11, overflow: "auto", maxHeight: 120 }}>
          {config ? JSON.stringify(config, null, 2) : "加载中…"}
        </pre>
      </section>

      <section style={{ marginTop: 12 }}>
        <h2 style={{ fontSize: 13, margin: "0 0 6px" }}>事件流（调试）</h2>
        <ul style={{ fontSize: 11, maxHeight: 140, overflow: "auto", paddingLeft: 20 }}>
          {rawEvents.length === 0 ? <li style={{ color: "#999" }}>暂无事件</li> : rawEvents.map((e, i) => <li key={i}>{e}</li>)}
        </ul>
      </section>
    </main>
  );
}
