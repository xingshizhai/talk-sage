//! 阿里云 NLS Token 获取与缓存。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use sha1::Sha1;

const TOKEN_ENDPOINT: &str = "http://nls-meta.cn-shanghai.aliyuncs.com/";
const REFRESH_BEFORE_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub id: String,
    pub expire_time: u64,
}

impl TokenInfo {
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now + REFRESH_BEFORE_SECS >= self.expire_time
    }
}

pub struct TokenManager {
    access_key_id: String,
    access_key_secret: String,
    cached: Mutex<Option<TokenInfo>>,
}

impl TokenManager {
    pub fn new(access_key_id: impl Into<String>, access_key_secret: impl Into<String>) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            access_key_secret: access_key_secret.into(),
            cached: Mutex::new(None),
        }
    }

    pub async fn get(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        {
            let guard = self.cached.lock().unwrap();
            if let Some(ref t) = *guard {
                if !t.is_expired() {
                    return Ok(t.id.clone());
                }
            }
        }
        let info = self.fetch(client).await?;
        let id = info.id.clone();
        *self.cached.lock().unwrap() = Some(info);
        Ok(id)
    }

    /// 验证 AccessKey 凭据是否有效（设置页「检查」按钮）：直接向阿里云
    /// 请求一个 token，成功返回 token 有效期（Unix 秒），失败返回可读错误。
    pub async fn verify(&self, client: &reqwest::Client) -> anyhow::Result<u64> {
        let info = self.fetch(client).await?;
        Ok(info.expire_time)
    }

    async fn fetch(&self, client: &reqwest::Client) -> anyhow::Result<TokenInfo> {
        let timestamp = {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
            time_to_iso8601(now.as_secs())
        };
        let nonce = uuid::Uuid::new_v4().to_string();

        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("AccessKeyId", self.access_key_id.clone());
        params.insert("Action", "CreateToken".into());
        params.insert("Format", "JSON".into());
        params.insert("RegionId", "cn-shanghai".into());
        params.insert("SignatureMethod", "HMAC-SHA1".into());
        params.insert("SignatureNonce", nonce);
        params.insert("SignatureVersion", "1.0".into());
        params.insert("Timestamp", timestamp);
        params.insert("Version", "2019-02-28".into());

        let canonical = build_canonical_query(&params);
        let string_to_sign = format!("GET&{}&{}", percent_encode("/"), percent_encode(&canonical));
        let key = format!("{}&", self.access_key_secret);
        let sig = sign_hmac_sha1(&key, &string_to_sign);

        params.insert("Signature", sig);

        let qs: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let url = format!("{}?{}", TOKEN_ENDPOINT, qs);
        let resp = client.get(&url).send().await?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(parse_error_response(status, &body));
        }
        let token = &body["Token"];
        Ok(TokenInfo {
            id: token["Id"].as_str().unwrap_or("").to_string(),
            expire_time: token["ExpireTime"].as_u64().unwrap_or(0),
        })
    }
}

/// 把阿里云错误响应（Code/Message）解析成可读错误。
fn parse_error_response(status: reqwest::StatusCode, body: &serde_json::Value) -> anyhow::Error {
    let code = body["Code"].as_str().unwrap_or("");
    let message = body["Message"].as_str().unwrap_or("");
    let readable = match code {
        "InvalidAccessKeyId" => "AccessKey ID 无效（请核对是否填错或已停用）",
        "SignatureDoesNotMatch" => "AccessKey Secret 与 ID 不匹配（签名校验失败）",
        "Forbidden" => "无权限访问该资源（子账号可能未授权 NLS 服务）",
        "InvalidSecurityToken.Expired" => "临时凭证已过期",
        _ => "请求被阿里云拒绝",
    };
    if message.is_empty() {
        anyhow::anyhow!("阿里云 Token 请求失败 {status}: {readable}")
    } else {
        anyhow::anyhow!("阿里云 Token 请求失败 {status}: {readable}（{message}）")
    }
}

pub fn build_canonical_query(params: &BTreeMap<&str, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn sign_hmac_sha1(key: &str, data: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(data.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn time_to_iso8601(unix_secs: u64) -> String {
    let s = unix_secs;
    let sec = s % 60;
    let min = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let days = s / 86400;
    let (year, month, day) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, month, day, hour, min, sec)
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let yd = if leap { 366 } else { 365 };
        if days < yd { break; }
        days -= yd;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let month_days = [31u64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u64;
    for &md in &month_days {
        if days < md { break; }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_canonical_query_sorts_and_encodes() {
        let mut params = BTreeMap::new();
        params.insert("Action", "CreateToken".to_string());
        params.insert("Format", "JSON".to_string());
        params.insert("Version", "2019-02-28".to_string());
        let s = build_canonical_query(&params);
        assert!(s.starts_with("Action=CreateToken"));
        assert!(s.contains("&Format=JSON"));
        assert!(s.contains("&Version=2019-02-28"));
    }

    #[test]
    fn sign_produces_non_empty_base64() {
        let sig = sign_hmac_sha1("key", "string-to-sign");
        assert!(!sig.is_empty());
        assert!(sig.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn token_info_expired() {
        let t = TokenInfo { id: "tok".into(), expire_time: 0 };
        assert!(t.is_expired());
    }

    #[test]
    fn token_info_not_expired() {
        let t = TokenInfo { id: "tok".into(), expire_time: u64::MAX };
        assert!(!t.is_expired());
    }

    #[test]
    fn error_response_maps_common_codes() {
        let bad_id = parse_error_response(
            reqwest::StatusCode::FORBIDDEN,
            &serde_json::json!({ "Code": "InvalidAccessKeyId", "Message": "specified access key is not found" }),
        )
        .to_string();
        assert!(bad_id.contains("AccessKey ID 无效"), "实际: {bad_id}");
        assert!(bad_id.contains("specified access key"), "应附原始消息: {bad_id}");

        let sig = parse_error_response(
            reqwest::StatusCode::BAD_REQUEST,
            &serde_json::json!({ "Code": "SignatureDoesNotMatch", "Message": "" }),
        )
        .to_string();
        assert!(sig.contains("Secret 与 ID 不匹配"), "实际: {sig}");

        let unknown = parse_error_response(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            &serde_json::json!({ "Code": "ServiceUnavailable", "Message": "busy" }),
        )
        .to_string();
        assert!(unknown.contains("请求被阿里云拒绝"), "实际: {unknown}");
    }
}
