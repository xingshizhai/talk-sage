//! TalkSage v2 LLM Provider 抽象（OpenAI 兼容端点：DeepSeek/Kimi/Groq/Ollama…）。

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::Deserialize;

/// 必须短于 pipeline 的 15s 插件 deadline，为结果处理和线程调度留出余量。
const LLM_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const LLM_READ_TIMEOUT: Duration = Duration::from_secs(10);
const LLM_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const LLM_OVERALL_TIMEOUT: Duration = Duration::from_secs(12);

/// LLM Provider 抽象。
pub trait LLMProvider: Send + Sync {
    /// 一次完整补全。
    fn complete(&self, prompt: &str, system: &str) -> Result<String>;
}

/// OpenAI 兼容 Provider（`POST {base_url}/chat/completions`）。
pub struct OpenAICompatProvider {
    api_key: String,
    model: String,
    base_url: String,
}

impl OpenAICompatProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: String,
}

impl LLMProvider for OpenAICompatProvider {
    fn complete(&self, prompt: &str, system: &str) -> Result<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": prompt },
            ],
            "temperature": 0.3,
        });

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(LLM_CONNECT_TIMEOUT)
            .timeout_read(LLM_READ_TIMEOUT)
            .timeout_write(LLM_WRITE_TIMEOUT)
            .build();
        let mut req = agent
            .post(&url)
            .timeout(LLM_OVERALL_TIMEOUT)
            .set("Content-Type", "application/json");
        if !self.api_key.is_empty() && self.api_key != "ollama" {
            req = req.set("Authorization", &format!("Bearer {}", self.api_key));
        }
        let resp = req
            .send_json(&body)
            .map_err(|e| anyhow!("LLM 请求失败: {e}"))?;
        let parsed: ChatResponse = resp
            .into_json()
            .map_err(|e| anyhow!("LLM 响应解析失败: {e}"))?;
        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .ok_or_else(|| anyhow!("LLM 无输出"))
    }
}

/// 便于测试的 mock Provider。
pub struct MockProvider {
    pub response: String,
}

impl LLMProvider for MockProvider {
    fn complete(&self, _prompt: &str, _system: &str) -> Result<String> {
        Ok(self.response.clone())
    }
}

/// 顺序应答的 mock Provider（多轮/并行场景按调用次序返回不同响应）。
pub struct MockSeqProvider {
    responses: std::sync::Mutex<std::collections::VecDeque<String>>,
}

impl MockSeqProvider {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses.into()),
        }
    }
}

impl LLMProvider for MockSeqProvider {
    fn complete(&self, _prompt: &str, _system: &str) -> Result<String> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("MockSeqProvider 应答耗尽"))
    }
}
