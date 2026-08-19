// 调试窗口（模态）：事件流 + 日志 两个 tab。

import { useEffect, useRef, useState } from "react";
import type { DomainEvent } from "../lib/api";

export default function DebugWindow({
  events,
  readLogs,
  onClose,
}: {
  events: DomainEvent[];
  readLogs: (lines?: number) => Promise<string>;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<"events" | "logs">("events");
  const [logs, setLogs] = useState<string>("（加载中…）");
  const [autoRefresh, setAutoRefresh] = useState(true);
  const eventsRef = useRef<HTMLDivElement>(null);
  const logsRef = useRef<HTMLDivElement>(null);

  // 日志加载 + 自动刷新（3s）
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | undefined;
    const load = () => readLogs(300).then(setLogs).catch((e) => setLogs(`读取失败: ${e}`));
    load();
    if (autoRefresh) {
      timer = setInterval(load, 3000);
    }
    return () => {
      if (timer) clearInterval(timer);
    };
  }, [readLogs, autoRefresh, tab]);

  // 自动滚动到底部
  useEffect(() => {
    const el = tab === "events" ? eventsRef.current : logsRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [events, logs, tab]);

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.6)",
        zIndex: 1000,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
      }}
      onClick={onClose}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: "80%",
          height: "80%",
          maxWidth: 900,
          background: "#0b1120",
          border: "1px solid rgba(255,255,255,0.15)",
          borderRadius: 12,
          display: "flex",
          flexDirection: "column",
          padding: 12,
          boxSizing: "border-box",
        }}
      >
        {/* 标题栏 */}
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
          <div style={{ display: "flex", gap: 6 }}>
            <button
              onClick={() => setTab("events")}
              style={{ fontSize: 12, padding: "4px 12px", borderRadius: 6, cursor: "pointer", background: tab === "events" ? "#2563eb" : "#1e293b", color: "#e2e8f0", border: "none" }}
            >
              事件流
            </button>
            <button
              onClick={() => setTab("logs")}
              style={{ fontSize: 12, padding: "4px 12px", borderRadius: 6, cursor: "pointer", background: tab === "logs" ? "#2563eb" : "#1e293b", color: "#e2e8f0", border: "none" }}
            >
              日志
            </button>
          </div>
          <div style={{ display: "flex", gap: 10, alignItems: "center" }}>
            {tab === "logs" && (
              <label style={{ fontSize: 11, color: "#94a3b8" }}>
                <input type="checkbox" checked={autoRefresh} onChange={(e) => setAutoRefresh(e.target.checked)} /> 自动刷新(3s)
              </label>
            )}
            <button onClick={onClose} style={{ fontSize: 12, padding: "4px 10px", borderRadius: 6, cursor: "pointer", background: "#ef4444", color: "#fff", border: "none" }}>
              关闭
            </button>
          </div>
        </div>

        {/* 内容 */}
        <div
          ref={tab === "events" ? eventsRef : logsRef}
          style={{
            flex: 1,
            overflowY: "auto",
            fontFamily: "monospace",
            fontSize: 11,
            lineHeight: 1.6,
            background: "rgba(255,255,255,0.03)",
            borderRadius: 8,
            padding: 10,
            whiteSpace: "pre-wrap",
            wordBreak: "break-all",
          }}
        >
          {tab === "events" ? (
            events.length === 0 ? (
              <span style={{ color: "#64748b" }}>暂无事件</span>
            ) : (
              events.map((ev, i) => (
                <div key={i} style={{ color: "#cbd5e1", marginBottom: 2 }}>
                  {JSON.stringify(ev)}
                </div>
              ))
            )
          ) : (
            <span style={{ color: "#cbd5e1" }}>{logs}</span>
          )}
        </div>
      </div>
    </div>
  );
}
