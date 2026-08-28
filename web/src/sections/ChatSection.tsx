// AI 助手：多轮对话。左侧话题列表，右侧消息流；回答经 ChatDelta 事件逐字到达。

import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import type { ChatMessageRecord, ChatThread, DomainEvent } from "../lib/api";
import { getApi } from "../lib/transport";

const api = getApi();

/** 生成中的回答在库里还是空的，正文只存在于这个内存缓冲里。 */
type Streaming = { messageId: number; text: string };

export default function ChatSection({ onOpenSettings }: { onOpenSettings: () => void }) {
  const [threads, setThreads] = useState<ChatThread[]>([]);
  const [activeId, setActiveId] = useState<number | null>(null);
  const [messages, setMessages] = useState<ChatMessageRecord[]>([]);
  const [draft, setDraft] = useState("");
  const [streaming, setStreaming] = useState<Streaming | null>(null);
  const [sending, setSending] = useState(false);
  const [message, setMessage] = useState("");
  const [renamingId, setRenamingId] = useState<number | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // 事件回调里要读到最新的话题 id，而闭包只在挂载时建立一次
  const activeIdRef = useRef<number | null>(null);
  activeIdRef.current = activeId;

  const refreshThreads = useCallback(async () => {
    try {
      setThreads(await api.listChatThreads());
    } catch (e) {
      console.error("读取话题列表失败:", e);
    }
  }, []);

  const openThread = useCallback(async (id: number) => {
    setActiveId(id);
    setStreaming(null);
    try {
      setMessages(await api.getChatMessages(id));
    } catch (e) {
      console.error("读取消息失败:", e);
      setMessages([]);
    }
  }, []);

  // 首次进入：加载话题列表，并自动打开最近一个
  useEffect(() => {
    (async () => {
      try {
        const list = await api.listChatThreads();
        setThreads(list);
        if (list.length > 0) void openThread(list[0].id);
      } catch (e) {
        console.error("读取话题列表失败:", e);
      }
    })();
  }, [openThread]);

  // 流式增量：只认当前话题，done 时回库取完整正文（生成中库里还是空的）
  useEffect(() => {
    const off = api.onEvent((ev: DomainEvent) => {
      if (ev.type !== "chat_delta") return;
      if (ev.thread_id !== activeIdRef.current) return;
      if (!ev.done) {
        setStreaming((prev) =>
          prev && prev.messageId === ev.message_id
            ? { ...prev, text: prev.text + (ev.delta ?? "") }
            : { messageId: ev.message_id, text: ev.delta ?? "" },
        );
        return;
      }
      setStreaming(null);
      setSending(false);
      if (ev.error) setMessage(`生成失败: ${ev.error}`);
      const tid = ev.thread_id;
      api.getChatMessages(tid).then(setMessages).catch(() => {});
      void refreshThreads(); // 首条提问会自动生成话题名
    });
    return off;
  }, [refreshThreads]);

  // 新内容到达时滚到底
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, streaming]);

  async function handleNewThread() {
    try {
      const id = await api.createChatThread();
      await refreshThreads();
      await openThread(id);
      setMessage("");
    } catch (e) {
      setMessage(`新建话题失败: ${e}`);
    }
  }

  async function handleSend() {
    const text = draft.trim();
    if (!text || sending) return;
    setMessage("");
    let threadId = activeId;
    try {
      // 直接开聊时没有话题：先建一个，省得用户先点"新建"
      if (threadId === null) {
        threadId = await api.createChatThread();
        setActiveId(threadId);
        await refreshThreads();
      }
      setSending(true);
      setDraft("");
      const sent = await api.sendChatMessage(threadId, text);
      setMessages(await api.getChatMessages(threadId));
      // 首批增量可能比这行更早到（事件走 WS，而这里还在等 HTTP 往返）：
      // 已经在累积就别用空串盖掉，否则回答开头会缺字。
      setStreaming((prev) =>
        prev && prev.messageId === sent.assistant_message_id
          ? prev
          : { messageId: sent.assistant_message_id, text: "" },
      );
    } catch (e) {
      setSending(false);
      setDraft(text); // 发送失败别把用户写的东西吞掉
      setMessage(`${e}`.replace(/^Error: /, ""));
    }
  }

  async function handleStop() {
    if (!streaming) return;
    try {
      await api.cancelChatMessage(streaming.messageId);
    } catch (e) {
      console.error("停止生成失败:", e);
    }
  }

  async function handleDeleteThread(id: number) {
    if (!window.confirm("删除这个话题及其全部消息？")) return;
    try {
      await api.deleteChatThread(id);
      const list = await api.listChatThreads();
      setThreads(list);
      if (activeId === id) {
        setMessages([]);
        setActiveId(null);
        if (list.length > 0) void openThread(list[0].id);
      }
    } catch (e) {
      setMessage(`删除失败: ${e}`);
    }
  }

  async function commitRename(id: number, current: string | null) {
    const next = renameDraft.trim();
    setRenamingId(null);
    if (next === (current ?? "").trim()) return;
    try {
      await api.renameChatThread(id, next);
      await refreshThreads();
    } catch (e) {
      setMessage(`重命名失败: ${e}`);
    }
  }

  const threadRow = (t: ChatThread): CSSProperties => ({
    padding: "6px 8px",
    borderRadius: 6,
    marginBottom: 4,
    cursor: "pointer",
    fontSize: 12,
    background: t.id === activeId ? "var(--me-soft)" : "var(--surface-2)",
    display: "flex",
    alignItems: "center",
    gap: 6,
  });

  const bubble = (role: string): CSSProperties => ({
    maxWidth: "82%",
    alignSelf: role === "user" ? "flex-end" : "flex-start",
    background: role === "user" ? "var(--me-soft)" : "var(--surface-2)",
    border: "1px solid var(--border)",
    borderRadius: 10,
    padding: "7px 10px",
    fontSize: 12.5,
    lineHeight: 1.7,
    whiteSpace: "pre-wrap",
    wordBreak: "break-word",
  });

  return (
    <div style={{ display: "flex", gap: 10, height: "100%", minHeight: 0, fontSize: 12 }}>
      {/* 话题列表 */}
      <div
        style={{
          width: 190,
          flexShrink: 0,
          display: "flex",
          flexDirection: "column",
          minHeight: 0,
          border: "1px solid var(--border)",
          borderRadius: 8,
          padding: 8,
        }}
      >
        <button onClick={handleNewThread} style={{ fontSize: 12, marginBottom: 8, flexShrink: 0 }}>
          ＋ 新话题
        </button>
        <div style={{ flex: 1, minHeight: 0, overflowY: "auto" }}>
          {threads.length === 0 && <div style={{ color: "var(--muted)", fontSize: 11 }}>还没有对话</div>}
          {threads.map((t) => (
            <div key={t.id} style={threadRow(t)} onClick={() => openThread(t.id)}>
              {renamingId === t.id ? (
                <input
                  autoFocus
                  value={renameDraft}
                  maxLength={60}
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => setRenameDraft(e.target.value)}
                  onBlur={() => commitRename(t.id, t.title)}
                  onKeyDown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") {
                      e.preventDefault();
                      void commitRename(t.id, t.title);
                    } else if (e.key === "Escape") {
                      e.preventDefault();
                      setRenamingId(null);
                    }
                  }}
                  style={{
                    flex: 1,
                    minWidth: 0,
                    fontSize: 12,
                    padding: "1px 5px",
                    borderRadius: 4,
                    border: "1px solid var(--me)",
                    background: "var(--surface-2)",
                    color: "var(--text)",
                  }}
                />
              ) : (
                <>
                  <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {t.title || "新对话"}
                  </span>
                  <button
                    title="重命名"
                    onClick={(e) => {
                      e.stopPropagation();
                      setRenamingId(t.id);
                      setRenameDraft(t.title ?? "");
                    }}
                    style={iconBtn}
                  >
                    改名
                  </button>
                  <button
                    title="删除话题"
                    onClick={(e) => {
                      e.stopPropagation();
                      void handleDeleteThread(t.id);
                    }}
                    style={{ ...iconBtn, color: "var(--danger)" }}
                  >
                    删
                  </button>
                </>
              )}
            </div>
          ))}
        </div>
      </div>

      {/* 对话区 */}
      <div
        style={{
          flex: 1,
          minWidth: 0,
          display: "flex",
          flexDirection: "column",
          minHeight: 0,
          border: "1px solid var(--border)",
          borderRadius: 8,
          padding: 10,
        }}
      >
        <div ref={scrollRef} style={{ flex: 1, minHeight: 0, overflowY: "auto", display: "flex", flexDirection: "column", gap: 8 }}>
          {messages.length === 0 && !streaming && (
            <div style={{ color: "var(--muted)", margin: "auto", textAlign: "center", lineHeight: 1.8 }}>
              问点什么吧 —— 整理思路、起草邮件、拆解问题都行。
              <br />
              <span style={{ fontSize: 11 }}>回答由「设置 → LLM」里配置的模型生成。</span>
            </div>
          )}
          {messages.map((m) => {
            // 生成中的那条回答正文在 streaming 缓冲里，库里还是空的
            if (streaming && m.id === streaming.messageId) return null;
            return (
              <div key={m.id} style={bubble(m.role)}>
                {m.content}
              </div>
            );
          })}
          {streaming && (
            <div style={bubble("assistant")}>
              {streaming.text}
              <span style={{ color: "var(--muted)" }}>{streaming.text ? " ▍" : "思考中…"}</span>
            </div>
          )}
        </div>

        {message && (
          <div style={{ fontSize: 11, color: "var(--danger)", marginTop: 6, display: "flex", gap: 8, alignItems: "center" }}>
            <span style={{ wordBreak: "break-all" }}>{message}</span>
            {message.includes("LLM") && (
              <button onClick={onOpenSettings} style={{ fontSize: 11, flexShrink: 0 }}>
                去设置
              </button>
            )}
          </div>
        )}

        <div style={{ display: "flex", gap: 8, marginTop: 8, flexShrink: 0, alignItems: "flex-end" }}>
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              // Enter 发送、Shift+Enter 换行；输入法组词期间的 Enter 不能当发送
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void handleSend();
              }
            }}
            placeholder="输入问题，Enter 发送，Shift+Enter 换行"
            rows={2}
            style={{
              flex: 1,
              minWidth: 0,
              resize: "vertical",
              padding: "6px 8px",
              fontSize: 12.5,
              lineHeight: 1.6,
              borderRadius: 6,
              border: "1px solid var(--border)",
              background: "var(--surface-2)",
              color: "var(--text)",
              fontFamily: "inherit",
            }}
          />
          {streaming ? (
            <button onClick={handleStop} style={{ fontSize: 12, flexShrink: 0 }}>
              停止
            </button>
          ) : (
            <button onClick={handleSend} disabled={sending || !draft.trim()} style={{ fontSize: 12, flexShrink: 0 }}>
              {sending ? "发送中…" : "发送"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

const iconBtn: CSSProperties = {
  fontSize: 10,
  padding: "1px 5px",
  borderRadius: 5,
  cursor: "pointer",
  border: "1px solid var(--border)",
  background: "var(--surface-2)",
  color: "var(--muted)",
  flexShrink: 0,
};
