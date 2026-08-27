//! TalkSage v2 LLM Provider 抽象（OpenAI 兼容端点：DeepSeek/Kimi/Groq/Ollama…）。

mod prompt;

pub use prompt::render_prompt;

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
    /// 可选代理 URL（如 `http://127.0.0.1:7890`）；`None` 时直连，不读 env var。
    proxy: Option<String>,
}

impl OpenAICompatProvider {
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            proxy: None,
        }
    }

    /// 设置 HTTP 代理 URL（如 `http://127.0.0.1:7890`）。
    /// 无效 URL 会被忽略并记录警告。
    pub fn with_proxy(mut self, proxy: Option<impl Into<String>>) -> Self {
        self.proxy = proxy.map(Into::into);
        self
    }

    fn build_agent(&self) -> ureq::Agent {
        let mut builder = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout_connect(LLM_CONNECT_TIMEOUT)
            .timeout_read(LLM_READ_TIMEOUT)
            .timeout_write(LLM_WRITE_TIMEOUT);
        if let Some(p) = &self.proxy {
            match ureq::Proxy::new(p) {
                Ok(proxy_cfg) => { builder = builder.proxy(proxy_cfg); }
                Err(e) => { log::warn!("LLM 代理地址无效，将直连: proxy={p} error={e}"); }
            }
        }
        builder.build()
    }

    /// 最小化连通性测试：向配置的端点发一个 max_tokens=1 的请求，
    /// 验证 key / base_url / model 是否可用。不依赖 [`Self::complete`]
    /// 的完整响应解析（有些端点可能因超长 prompt 拒绝）。
    pub fn test_connection(&self) -> Result<()> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "messages": [ { "role": "user", "content": "ping" } ],
            "max_tokens": 1,
            "temperature": 0.0,
        });
        let agent = self.build_agent();
        let mut req = agent
            .post(&url)
            .timeout(LLM_OVERALL_TIMEOUT)
            .set("Content-Type", "application/json");
        if !self.api_key.is_empty() && self.api_key != "ollama" {
            req = req.set("Authorization", &format!("Bearer {}", self.api_key));
        }
        match req.send_json(&body) {
            Ok(resp) if resp.status() == 200 => Ok(()),
            Ok(_) => Err(anyhow!("HTTP 非 2xx，连接未通过验证")),
            // ureq 对非 2xx 返回 Err（Error::Status）；从这里提取状态码给出可读提示
            Err(ureq::Error::Status(status, _resp)) => Err(anyhow!(
                "HTTP {status}：{}",
                friendly_status(status)
            )),
            Err(e) => Err(anyhow!("请求失败: {e}")),
        }
    }
}

/// 常见 LLM 端点错误的人类可读说明（供「检查连接」按钮展示）。
fn friendly_status(status: u16) -> &'static str {
    match status {
        401 => "API Key 无效或未授权（请核对 key 是否复制完整、是否为当前服务商的 key）",
        403 => "无权限访问该模型（key 有效但模型不可用，或套餐未开通）",
        404 => "端点或模型不存在（请核对 base_url / model 名称）",
        429 => "请求过于频繁或余额不足（限流 / 配额用尽）",
        _ => "请求被拒绝",
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

        let agent = self.build_agent();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_status_maps_common_errors() {
        assert!(friendly_status(401).contains("API Key"));
        assert!(friendly_status(403).contains("无权限"));
        assert!(friendly_status(404).contains("不存在"));
        assert!(friendly_status(429).contains("限流"));
        assert!(!friendly_status(500).is_empty());
    }

    /// test_connection：本地端点返回 200 时成功，401 时报出可读错误。
    #[test]
    fn test_connection_hits_local_endpoint() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            // 处理两次请求（合法 key / 非法 key 各一次）
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                // 读完整请求头（直到空行）
                let mut buf = [0u8; 8192];
                let mut received = 0usize;
                while received < buf.len() {
                    match stream.read(&mut buf[received..]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            received += n;
                            if buf[..received].windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let req = String::from_utf8_lossy(&buf[..received]);
                let body = if req.contains("Authorization: Bearer sk-test") {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                } else {
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                };
                let _ = stream.write_all(body.as_bytes());
            }
        });

        let provider = OpenAICompatProvider::new(
            "sk-test",
            "mock-model",
            format!("http://{addr}/v1"),
        );
        assert!(provider.test_connection().is_ok(), "合法 key 应连接成功");

        let bad = OpenAICompatProvider::new("bad-key", "mock-model", format!("http://{addr}/v1"));
        let err = bad.test_connection().unwrap_err().to_string();
        assert!(err.contains("401"), "无效 key 应报 401，实际: {err}");
        assert!(err.contains("API Key"), "401 错误应含可读提示，实际: {err}");

        server.join().unwrap();
    }
}
