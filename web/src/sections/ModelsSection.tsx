// 模型管理：下载 / 删除 ASR 引擎，与「设置」里的转写参数分开。

import { useEffect, useState, type CSSProperties } from "react";
import type { AsrModelInfo, DomainEvent } from "../lib/api";
import { getApi } from "../lib/transport";

const api = getApi();

export default function ModelsSection({ listening }: { listening: boolean }) {
  const [asrModels, setAsrModels] = useState<AsrModelInfo[]>([]);
  const [modelProgress, setModelProgress] = useState<Record<string, DomainEvent & { type: "model_progress" }>>({});
  const [modelBusy, setModelBusy] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState<string | null>(null);
  const [message, setMessage] = useState("");

  useEffect(() => {
    api.listAsrModels().then(setAsrModels).catch((e) => {
      console.error("读取 ASR 模型列表失败:", e);
      setMessage(`读取模型列表失败: ${e}`);
    });
  }, []);

  useEffect(() => {
    const off = api.onEvent((ev: DomainEvent) => {
      if (ev.type !== "model_progress") return;
      setModelProgress((prev) => ({ ...prev, [ev.engine]: ev }));
      if (ev.stage === "done" || ev.stage === "error" || ev.stage === "cancelled") {
        setCancelling((cur) => (cur === ev.engine ? null : cur));
        api.listAsrModels().then(setAsrModels).catch(() => {});
      }
    });
    return off;
  }, []);

  async function handleDownloadModel(id: string) {
    setModelBusy(id);
    setMessage("");
    try {
      await api.downloadModel(id);
      setAsrModels(await api.listAsrModels());
    } catch (e) {
      setMessage(`模型安装失败: ${e}`);
    } finally {
      setModelBusy(null);
    }
  }

  async function handleRemoveModel(id: string) {
    if (!window.confirm(`确定删除「${id}」模型文件吗？删除后需重新下载。`)) return;
    setModelBusy(id);
    setMessage("");
    try {
      await api.removeModel(id);
      setAsrModels(await api.listAsrModels());
      setMessage(`已删除模型: ${id}`);
    } catch (e) {
      setMessage(`删除失败: ${e}`);
    } finally {
      setModelBusy(null);
    }
  }

  async function handleCancelDownload(id: string) {
    setCancelling(id);
    setMessage("");
    try {
      await api.cancelModelDownload(id);
      setMessage(`已请求取消: ${id}`);
    } catch (e) {
      setCancelling(null);
      setMessage(`取消失败: ${e}`);
    }
  }

  const hint: CSSProperties = { marginTop: 8, color: "var(--muted)", fontSize: 11, lineHeight: 1.6 };
  const busy = listening || modelBusy !== null;

  return (
    <div style={{ border: "1px solid var(--border)", borderRadius: 8, padding: 10, fontSize: 12, display: "flex", flexDirection: "column", height: "100%", minHeight: 0, boxSizing: "border-box" }}>
      <h3 style={{ margin: "0 0 6px", fontSize: 13 }}>转写模型</h3>
      <div style={{ ...hint, marginTop: 0, marginBottom: 8 }}>
        下载在后台进行，可切换页面继续使用。安装或删除前请先停止监听。选用哪个引擎请到「设置 → ASR 转写」。
      </div>
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column", gap: 6 }}>
        {asrModels.length === 0 && <div style={{ fontSize: 12, color: "var(--muted)" }}>加载模型列表…</div>}
        {asrModels.map((m) => {
          const prog = modelProgress[m.id];
          const active = modelBusy === m.id || (prog && (prog.stage === "downloading" || prog.stage === "extracting"));
          return (
            <div key={m.id} style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12, border: "1px solid var(--border)", borderRadius: 8, padding: "6px 10px", background: "var(--surface-2)" }}>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 700 }}>
                  {m.label}
                  {m.selectable === false && m.id !== "punct" && <span style={{ color: "var(--brief)", marginLeft: 6 }}>预下载</span>}
                  {m.installed ? <span style={{ color: "var(--live)", marginLeft: 6 }}>✓ 已安装</span> : <span style={{ color: "var(--brief)", marginLeft: 6 }}>未安装</span>}
                </div>
                <div style={{ color: "var(--muted)", marginTop: 2 }}>
                  {m.description} · 约 {m.download_size_mb ?? "?"} MB
                  {m.size_mb ? ` · 已占用 ${m.size_mb} MB` : ""}
                  {m.downloading && !active ? " · 检测到未完成下载，点击可继续" : ""}
                </div>
                {active && prog && (prog.stage === "downloading" || prog.stage === "extracting") && (
                  <div style={{ marginTop: 4, height: 5, background: "var(--border)", borderRadius: 3, overflow: "hidden" }}>
                    <div style={{ height: "100%", width: `${prog.percent ?? 0}%`, background: "var(--live)", transition: "width 0.4s linear" }} />
                  </div>
                )}
                {prog && prog.stage === "error" && <div style={{ color: "var(--danger)", marginTop: 2 }}>下载失败: {prog.message}</div>}
                {prog && prog.stage === "cancelled" && <div style={{ color: "var(--brief)", marginTop: 2 }}>已取消下载，可重新下载</div>}
                {prog && prog.stage === "done" && <div style={{ color: "var(--live)", marginTop: 2 }}>安装完成</div>}
              </div>
              {active && (prog?.stage === "downloading" || prog?.stage === "extracting") ? (
                <button onClick={() => void handleCancelDownload(m.id)} disabled={cancelling === m.id} style={{ fontSize: 12, padding: "4px 10px", cursor: cancelling === m.id ? "default" : "pointer" }}>
                  {cancelling === m.id ? "取消中…" : "取消"}
                </button>
              ) : m.installed ? (
                <button onClick={() => void handleRemoveModel(m.id)} disabled={busy || !!active} style={{ fontSize: 12, padding: "4px 10px", cursor: "pointer" }}>
                  删除
                </button>
              ) : (
                <button onClick={() => void handleDownloadModel(m.id)} disabled={busy || !!active} style={{ fontSize: 12, padding: "4px 10px", cursor: "pointer" }}>
                  {active ? "下载中…" : "下载"}
                </button>
              )}
            </div>
          );
        })}
      </div>
      {listening && (
        <div style={{ ...hint, color: "var(--brief)" }}>监听中无法安装或删除模型，请先停止监听。</div>
      )}
      <div style={hint}>
        Qwen3-ASR 由官方 GitHub release 提供（HF 仓库为受限私有）。下载、续传、校验和错误会写入应用日志。
      </div>
      {message && (
        <div style={{ marginTop: 8, fontSize: 11, color: message.includes("失败") ? "var(--danger)" : "var(--live)" }}>{message}</div>
      )}
    </div>
  );
}
