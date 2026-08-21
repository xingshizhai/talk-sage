//! 会议结束 Webhook（借鉴 Call.md workflow-webhook + url-guard）。
//!
//! - `validate_webhook_url`：调用前 SSRF 防护——拒绝非 http(s)、内网/回环 IP、
//!   localhost、`.local` 主机名、以及解析到私网地址的主机名。
//! - `post_webhook`：POST JSON payload（不做校验，由 trigger 层把关）。
//! - `trigger_webhooks`：逐条校验 → 发送，返回每条结果（供日志/UI 回溯）。

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use serde::Serialize;

/// 单条 webhook 触发结果。
#[derive(Debug, Clone, Serialize)]
pub struct WebhookResult {
    pub url: String,
    pub ok: bool,
    pub message: String,
}

/// SSRF 防护：校验 webhook URL 是否允许调用。
///
/// 拒绝规则（借鉴 Call.md url-guard）：
/// - 仅允许 http/https
/// - 字面 IP：回环（127/8、::1）、私网（10/8、172.16/12、192.168/16）、链路本地（169.254/16）、未指定（0.0.0.0、::）
/// - 主机名：localhost、`*.local` 后缀
/// - 主机名解析后的任一地址落在私网/回环/链路本地 → 拒绝（防 DNS 重绑定）
pub fn validate_webhook_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(format!("仅支持 http/https: {trimmed}"));
    }
    let rest = trimmed.split_once("://").map(|(_, r)| r).unwrap_or(trimmed);
    let host = rest.split('/').next().unwrap_or(rest);
    let host = host.split(':').next().unwrap_or(host).trim_matches(['[', ']']);
    if host.is_empty() {
        return Err("URL 缺少主机名".into());
    }

    // 字面 IP 检查
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(format!("拒绝内网/回环地址: {ip}"));
        }
        return Ok(());
    }

    // 主机名检查
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".local") {
        return Err(format!("拒绝本地主机名: {host}"));
    }

    // 解析并检查所有地址（防 DNS 重绑定）；解析失败（离线/沙箱）时放行——
    // 本地应用不应因 DNS 暂不可用而拒绝合法 webhook，字面 IP/localhost 检查已覆盖常见 SSRF 向量。
    if let Ok(addrs) = (host, 0).to_socket_addrs() {
        for a in addrs {
            if is_private_ip(&a.ip()) {
                return Err(format!("拒绝解析到内网地址的主机名 {host} -> {}", a.ip()));
            }
        }
    }
    Ok(())
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let oct = v4.octets();
            v4.is_loopback()                       // 127/8
                || v4.is_private()                 // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()              // 169.254/16
                || v4.is_unspecified()             // 0.0.0.0
                || oct[0] == 100 && (oct[1] & 0xc0) == 0x40 // 100.64/10 CGNAT
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || v6.is_unique_local()
        }
    }
}

/// POST JSON payload 到 webhook（不校验 URL；超时 10s；显式禁用环境代理，走直连）。
pub fn post_webhook(url: &str, payload: &serde_json::Value) -> Result<(), String> {
    let agent: ureq::Agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(8))
        .timeout_write(Duration::from_secs(5))
        .build();
    let resp = agent
        .post(url)
        .timeout(Duration::from_secs(10))
        .set("Content-Type", "application/json")
        .set("User-Agent", "talksage/1.0 webhook")
        .send_json(payload)
        .map_err(|e| format!("发送失败: {e}"))?;
    let status = resp.status();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("HTTP {status}"))
    }
}

/// 逐条校验并发送；返回每条结果（不因单条失败中断其余）。
pub fn trigger_webhooks(urls: &[String], payload: &serde_json::Value) -> Vec<WebhookResult> {
    urls.iter()
        .map(|url| match validate_webhook_url(url) {
            Err(e) => WebhookResult { url: url.clone(), ok: false, message: e },
            Ok(()) => match post_webhook(url, payload) {
                Ok(()) => WebhookResult { url: url.clone(), ok: true, message: "ok".into() },
                Err(e) => WebhookResult { url: url.clone(), ok: false, message: e },
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn validate_accepts_public_https() {
        assert!(validate_webhook_url("https://hooks.example.com/x/y").is_ok());
        assert!(validate_webhook_url("http://example.com/hook").is_ok());
    }

    #[test]
    fn validate_rejects_non_http_and_bad() {
        assert!(validate_webhook_url("ftp://x.com/h").is_err());
        assert!(validate_webhook_url("not a url").is_err());
        assert!(validate_webhook_url("https:///nohost").is_err());
    }

    #[test]
    fn validate_rejects_private_and_loopback() {
        assert!(validate_webhook_url("http://127.0.0.1:8080/hook").is_err());
        assert!(validate_webhook_url("http://10.0.0.5/hook").is_err());
        assert!(validate_webhook_url("http://192.168.1.1/hook").is_err());
        assert!(validate_webhook_url("http://172.16.0.1/hook").is_err());
        assert!(validate_webhook_url("http://169.254.169.254/latest/meta-data").is_err(), "云元数据端点必须拒绝");
        assert!(validate_webhook_url("http://localhost:3000/hook").is_err());
        assert!(validate_webhook_url("http://[::1]:8080/hook").is_err());
        assert!(validate_webhook_url("http://myhost.local/hook").is_err());
    }

    /// 端到端：本地 TcpListener 接收 POST（post_webhook 不做校验，直接测试传输）。
    #[test]
    fn post_webhook_delivers_json() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let mut req = String::from_utf8_lossy(&buf[..n]).to_string();
            // 回一个最小 HTTP 200（ureq 需要读取状态行），随后排空剩余 body
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
            let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
            let mut tmp = [0u8; 4096];
            loop {
                match stream.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(m) => req.push_str(&String::from_utf8_lossy(&tmp[..m])),
                }
            }
            req
        });
        std::thread::sleep(Duration::from_millis(50));
        let payload = serde_json::json!({ "meeting": { "id": 1 }, "content": { "summary": "hi" } });
        let r = post_webhook(&format!("http://{addr}/hook"), &payload);
        assert!(r.is_ok(), "post_webhook 失败: {r:?}");
        let req = handle.join().unwrap();
        assert!(req.contains("POST /hook"));
        assert!(req.contains("\"meeting\""));
        assert!(req.contains("\"summary\":\"hi\""));
    }
}
