//! AI 助手：多轮对话的编排。
//!
//! 职责是把「话题历史 → LLM 流式请求 → 增量事件 + 落库」串起来，不关心传输：
//! 增量通过 `emit` 回调交给宿主（Tauri 走 `emit_all`，headless 走事件广播）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use talksage_config::ConfigManager;
use talksage_core::DomainEvent;
use talksage_llm::{ChatMessage, OpenAICompatProvider};
use talksage_session::SessionStore;

/// 增量事件出口。两个宿主各自实现（Tauri emit / broadcast 广播）。
pub type ChatEmit = Arc<dyn Fn(DomainEvent) + Send + Sync>;

/// 送进上下文的最近消息条数。对话越长 prompt 越贵，这里截断保底。
const CONTEXT_MESSAGES: usize = 20;
/// 流式生成期间的落库节流：崩溃最多丢这么久的内容。
const PERSIST_INTERVAL: Duration = Duration::from_secs(1);

const SYSTEM_PROMPT: &str = "你是 TalkSage（拓思者）内置的 AI 助手，帮用户思考与处理工作问题。\
回答简洁、直接、可执行；不确定就说不确定，不要编造。用户用什么语言提问就用什么语言回答。";

/// [`ChatService::send`] 的返回：两条消息都已入库，回答正文随后由事件补齐。
#[derive(Debug, Clone, Copy)]
pub struct ChatSendResult {
    /// 用户提问的消息 id。
    pub user_message_id: i64,
    /// 回答的消息 id —— 前端按它把 [`DomainEvent::ChatDelta`] 拼起来。
    pub assistant_message_id: i64,
}

/// AI 助手服务：话题存取 + 流式生成的调度。
pub struct ChatService {
    config: Arc<ConfigManager>,
    sessions: Arc<SessionStore>,
    /// 正在生成的回答 id → 取消开关。
    running: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

impl ChatService {
    pub fn new(config: Arc<ConfigManager>, sessions: Arc<SessionStore>) -> Self {
        Self {
            config,
            sessions,
            running: Mutex::new(HashMap::new()),
        }
    }

    /// 按配置构建聊天用的 provider。
    ///
    /// 与插件用的 [`crate::service::TalkSageService::build_llm`] 同源，但这里要的是
    /// 具体类型（`stream_chat` 是 `OpenAICompatProvider` 的固有方法，不在 trait 上）。
    fn build_provider(&self) -> Result<OpenAICompatProvider> {
        crate::service::TalkSageService::build_chat_provider(&self.config)
            .ok_or_else(|| anyhow!("尚未配置 LLM：请到「设置 → LLM」填写 API Key"))
    }

    /// 提交一条提问：落库 → 建空回答占位 → 后台线程流式生成。
    ///
    /// 立即返回两条消息的 id，界面先把提问和"生成中"的占位显示出来；正文由
    /// [`DomainEvent::ChatDelta`] 逐段补齐，结束时（done=true）再从库里读完整版。
    pub fn send(&self, thread_id: i64, text: &str, emit: ChatEmit) -> Result<ChatSendResult> {
        let text = text.trim();
        if text.is_empty() {
            return Err(anyhow!("提问不能为空"));
        }
        // 先验证 LLM 可用再落库：否则会留下一条永远等不到回答的空消息
        let provider = self.build_provider()?;

        let now_ms = now_ms();
        let user_message_id = self.sessions.add_chat_message(thread_id, "user", text, now_ms)?;
        let assistant_message_id = self.sessions.add_chat_message(thread_id, "assistant", "", now_ms + 1)?;

        // 上下文取最近 N 条（含刚落库的提问），system 单独放最前
        let history = self.sessions.get_chat_messages(thread_id)?;
        let mut messages = vec![ChatMessage::system(SYSTEM_PROMPT)];
        let start = history.len().saturating_sub(CONTEXT_MESSAGES + 1);
        messages.extend(
            history[start..]
                .iter()
                // 占位的空回答不能进上下文：有些端点会拒绝空 content
                .filter(|m| !(m.id == assistant_message_id || m.content.trim().is_empty()))
                .map(|m| ChatMessage {
                    role: m.role.clone(),
                    content: m.content.clone(),
                }),
        );

        let cancel = Arc::new(AtomicBool::new(false));
        self.running.lock().unwrap().insert(assistant_message_id, cancel.clone());

        let sessions = self.sessions.clone();
        std::thread::spawn(move || {
            let mut acc = String::new();
            let mut last_persist = Instant::now();
            let cancel_flag = cancel.clone();
            let mut on_delta = |delta: &str| {
                acc.push_str(delta);
                emit(DomainEvent::ChatDelta {
                    thread_id,
                    message_id: assistant_message_id,
                    delta: delta.to_string(),
                    done: false,
                    error: String::new(),
                });
                if last_persist.elapsed() >= PERSIST_INTERVAL {
                    last_persist = Instant::now();
                    if let Err(e) = sessions.update_chat_message(assistant_message_id, &acc) {
                        log::warn!("AI 助手回答落库失败（生成中）: {e}");
                    }
                }
            };
            let result = provider.stream_chat(
                &messages,
                &mut on_delta,
                &|| cancel_flag.load(Ordering::Relaxed),
            );

            let (final_text, error) = match result {
                Ok(text) => (text, String::new()),
                // 已经出了一部分字就保留：用户至少看得到生成到哪儿了
                Err(e) if !acc.is_empty() => (acc.clone(), e.to_string()),
                Err(e) => (String::new(), e.to_string()),
            };
            if final_text.trim().is_empty() {
                // 一个字都没有的占位留在库里只会变成一条空气泡
                if let Err(e) = sessions.delete_chat_message(assistant_message_id) {
                    log::warn!("清理空回答失败: {e}");
                }
            } else if let Err(e) = sessions.update_chat_message(assistant_message_id, &final_text) {
                log::warn!("AI 助手回答落库失败: {e}");
            }
            if !error.is_empty() {
                log::warn!("AI 助手回答异常: {error}");
            }
            emit(DomainEvent::ChatDelta {
                thread_id,
                message_id: assistant_message_id,
                delta: String::new(),
                done: true,
                error,
            });
        });

        Ok(ChatSendResult {
            user_message_id,
            assistant_message_id,
        })
    }

    /// 停止某条正在生成的回答；已生成的部分会保留。
    pub fn cancel(&self, message_id: i64) {
        if let Some(flag) = self.running.lock().unwrap().remove(&message_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// 生成结束后清掉取消开关（宿主在收到 done 时调用，避免 map 无限增长）。
    pub fn finish(&self, message_id: i64) {
        self.running.lock().unwrap().remove(&message_id);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> ChatService {
        let config = Arc::new(ConfigManager::from_config(
            talksage_config::Config::default(),
            std::env::temp_dir().join("talksage-chat-test"),
        ));
        let sessions = Arc::new(SessionStore::open(":memory:").unwrap());
        ChatService::new(config, sessions)
    }

    /// 没配 LLM key 时不该在库里留下"提问 + 永远空着的回答"，
    /// 而是直接把可操作的原因返回给界面。
    #[test]
    fn send_without_llm_key_reports_error_and_writes_nothing() {
        let svc = service();
        let thread = svc.sessions.create_chat_thread(1).unwrap();
        let emit: ChatEmit = Arc::new(|_| {});

        let err = svc.send(thread, "在吗", emit).unwrap_err().to_string();
        assert!(err.contains("LLM"), "错误应指向 LLM 配置: {err}");
        assert!(
            svc.sessions.get_chat_messages(thread).unwrap().is_empty(),
            "失败时不应留下半条对话"
        );
    }

    #[test]
    fn send_rejects_empty_question() {
        let svc = service();
        let thread = svc.sessions.create_chat_thread(1).unwrap();
        let emit: ChatEmit = Arc::new(|_| {});
        assert!(svc.send(thread, "   ", emit).is_err());
    }

    /// cancel 只对正在生成的回答有效，重复调用不应 panic。
    #[test]
    fn cancel_is_idempotent() {
        let svc = service();
        svc.cancel(42);
        svc.cancel(42);
        svc.finish(42);
    }
}
