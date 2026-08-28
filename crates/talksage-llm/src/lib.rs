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

/// 聊天（AI 助手）用的超时：与插件不同，用户在等一个完整回答，
/// 且流式下"读"是按 chunk 计的，卡住才该超时。
const CHAT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 两个 chunk 之间的最长间隔（首个 token 也受它约束）。
const CHAT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 一条聊天消息（多轮上下文）。
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct ChatMessage {
    /// system | user | assistant
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: content.into() }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
}

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

    /// 聊天用 agent：读超时按「两个 chunk 之间的最长间隔」设，不能沿用插件那套
    /// 10s —— 长回答会被拦腰截断。
    fn build_chat_agent(&self) -> ureq::Agent {
        let mut builder = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .timeout_connect(CHAT_CONNECT_TIMEOUT)
            .timeout_read(CHAT_STREAM_IDLE_TIMEOUT)
            .timeout_write(LLM_WRITE_TIMEOUT);
        if let Some(p) = &self.proxy {
            match ureq::Proxy::new(p) {
                Ok(proxy_cfg) => { builder = builder.proxy(proxy_cfg); }
                Err(e) => { log::warn!("LLM 代理地址无效，将直连: proxy={p} error={e}"); }
            }
        }
        builder.build()
    }

    /// 多轮流式补全：每收到一段增量就回调 `on_delta`，返回拼好的完整回答。
    ///
    /// `cancelled` 每读一行检查一次：返回 true 时立即收尾（用户点了停止），
    /// 已经产生的部分照常返回，不算错误。
    pub fn stream_chat(
        &self,
        messages: &[ChatMessage],
        on_delta: &mut dyn FnMut(&str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String> {
        use std::io::BufRead;

        let url = format!("{}/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.6,
            "stream": true,
        });
        let agent = self.build_chat_agent();
        let mut req = agent.post(&url).set("Content-Type", "application/json");
        if !self.api_key.is_empty() && self.api_key != "ollama" {
            req = req.set("Authorization", &format!("Bearer {}", self.api_key));
        }
        let resp = match req.send_json(&body) {
            Ok(r) => r,
            Err(ureq::Error::Status(status, _)) => {
                return Err(anyhow!("HTTP {status}：{}", friendly_status(status)))
            }
            Err(e) => return Err(anyhow!("LLM 请求失败: {e}")),
        };

        let mut reader = std::io::BufReader::new(resp.into_reader());
        let mut full = String::new();
        let mut line = String::new();
        loop {
            if cancelled() {
                break;
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // 端点直接关流（有些实现不发 [DONE]）
                Ok(_) => {}
                Err(e) => {
                    // 已经出了字就别把整次回答判死：把读到的部分交回去
                    if full.is_empty() {
                        return Err(anyhow!("LLM 流式读取失败: {e}"));
                    }
                    log::warn!("LLM 流式读取中断，返回已生成部分: {e}");
                    break;
                }
            }
            match parse_sse_line(&line) {
                SseLine::Delta(text) => {
                    full.push_str(&text);
                    on_delta(&text);
                }
                SseLine::Done => break,
                SseLine::Ignore => {}
            }
        }
        if full.trim().is_empty() {
            return Err(anyhow!("LLM 无输出"));
        }
        Ok(full)
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

/// 一行 SSE 的解析结果。
#[derive(Debug, PartialEq, Eq)]
pub enum SseLine {
    /// 一段新增文本。
    Delta(String),
    /// 流正常结束（`data: [DONE]`）。
    Done,
    /// 心跳 / 空行 / 只带 role 的首帧等，忽略即可。
    Ignore,
}

/// 解析流式响应里的一行。
///
/// 各家 OpenAI 兼容端点的差异都收在这里（首帧只给 role、中途插心跳注释、
/// DeepSeek 的 reasoning_content 等），拆成纯函数是为了不起网络也能测。
pub fn parse_sse_line(line: &str) -> SseLine {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(payload) = line.strip_prefix("data:") else {
        return SseLine::Ignore; // 空行、`event:`、`:` 开头的心跳注释
    };
    let payload = payload.trim();
    if payload == "[DONE]" {
        return SseLine::Done;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(payload) else {
        return SseLine::Ignore;
    };
    let delta = &v["choices"][0]["delta"];
    match delta["content"].as_str() {
        Some(text) if !text.is_empty() => SseLine::Delta(text.to_string()),
        // 首帧常常只有 {"role":"assistant"}；推理模型的 reasoning_content 不计入正文
        _ => SseLine::Ignore,
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

    /// SSE 行解析：正文增量、结束标记、以及各种应当忽略的噪音行。
    #[test]
    fn sse_line_parsing_covers_endpoint_quirks() {
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#),
            SseLine::Delta("你好".into())
        );
        assert_eq!(parse_sse_line("data: [DONE]"), SseLine::Done);
        // 首帧只带 role
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#),
            SseLine::Ignore
        );
        // 推理模型的思维链不计入正文
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"reasoning_content":"嗯"}}]}"#),
            SseLine::Ignore
        );
        // 心跳注释 / 空行 / 非 JSON
        assert_eq!(parse_sse_line(": keep-alive"), SseLine::Ignore);
        assert_eq!(parse_sse_line(""), SseLine::Ignore);
        assert_eq!(parse_sse_line("data: not-json"), SseLine::Ignore);
        // 末尾 CRLF 不影响解析
        assert_eq!(
            parse_sse_line("data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\r\n"),
            SseLine::Delta("x".into())
        );
    }

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
                let req = String::from_utf8_lossy(&buf[..received]).into_owned();
                // 请求体也要读干净再回包。Windows 上带着未读数据关闭 socket 会发
                // RST 而不是 FIN，客户端此时正在读状态行，就会撞上 WSAECONNRESET
                // （10054「远程主机强迫关闭了一个现有的连接」）—— 表现为整机满载
                // 跑 cargo test --workspace 时这条偶发失败。
                let header_end = buf[..received]
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|at| at + 4)
                    .unwrap_or(received);
                let content_length = req
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                let mut remaining = content_length.saturating_sub(received.saturating_sub(header_end));
                let mut sink = [0u8; 4096];
                while remaining > 0 {
                    let want = remaining.min(sink.len());
                    match stream.read(&mut sink[..want]) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => remaining -= n,
                    }
                }

                let body = if req.contains("Authorization: Bearer sk-test") {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                } else {
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                };
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
                // 显式半关闭：让对端收到 FIN，而不是靠 drop 的时机。
                let _ = stream.shutdown(std::net::Shutdown::Write);
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
