// 会议文字通道输入框（Fay 借鉴：文字 = 一等输入流）。
//
// 监听/导入期间把打字文本作为一条 final 段注入当前会话：与语音段同时间轴
// 实时广播（source=typed_text，前端显示为「文字」标签）并随会话落库。
// 供远程参会者提问/补充——不依赖拾音，也不受口音/ASR 误差影响。

import { useState } from "react";

export default function TypedInput({
  disabled,
  onSend,
  onError,
}: {
  disabled: boolean;
  onSend: (text: string) => Promise<void>;
  onError?: (msg: string) => void;
}) {
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);

  const submit = async () => {
    const t = text.trim();
    if (!t || sending) return;
    setSending(true);
    try {
      await onSend(t);
      setText("");
    } catch (error) {
      onError?.(`文字发送失败: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setSending(false);
    }
  };

  return (
    <div
      style={{
        display: "flex",
        gap: 8,
        padding: "7px 14px",
        background: "var(--surface-2)",
        border: "var(--card-border)",
        borderBottom: "none",
        borderRadius: "var(--card-radius) var(--card-radius) 0 0",
        alignItems: "center",
      }}
    >
      <span
        title="把打字文本作为一段并入当前会话转写（与语音段同时间轴、随会话落库）；适合远程参会者提问/补充"
        style={{ fontSize: 11, color: "var(--muted)", whiteSpace: "nowrap" }}
      >
        ⌨ 文字通道
      </span>
      <input
        value={text}
        disabled={disabled || sending}
        placeholder={
          disabled
            ? "开始监听后可在会中输入文字，实时并入会议记录…"
            : "输入文字，Enter 发送并实时并入转写流…"
        }
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // 输入法组词期间的 Enter 不能当发送
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            void submit();
          }
        }}
        style={{
          flex: 1,
          minWidth: 0,
          background: "var(--surface)",
          color: "var(--text)",
          border: "1px solid var(--border)",
          borderRadius: 7,
          padding: "6px 10px",
          font: "inherit",
          fontSize: 12,
        }}
      />
      <button
        onClick={() => void submit()}
        disabled={disabled || sending || !text.trim()}
        style={{
          padding: "5px 14px",
          borderRadius: 7,
          cursor: "pointer",
          border: "none",
          background: "var(--live)",
          color: "#fff",
          fontWeight: 600,
          fontSize: 12,
          opacity: disabled || !text.trim() ? 0.45 : 1,
        }}
      >
        {sending ? "发送中…" : "发送"}
      </button>
    </div>
  );
}
