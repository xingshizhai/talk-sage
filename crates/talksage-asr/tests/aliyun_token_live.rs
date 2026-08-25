/// 阿里云 Token 真实请求调试测试
/// 运行：cargo test -p talksage-asr --test aliyun_token_live -- --nocapture

use talksage_asr::aliyun::token::{build_canonical_query, sign_hmac_sha1};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn fetch_token_from_aliyun() {
    let key_id = std::env::var("ALIYUN_ACCESS_ID").expect("ALIYUN_ACCESS_ID not set");
    let key_secret = std::env::var("ALIYUN_ACCESS_SECRET").expect("ALIYUN_ACCESS_SECRET not set");

    // 手动构建请求，打印完整响应
    let client = reqwest::Client::new();

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let timestamp = format_iso8601(now);
    let nonce = uuid::Uuid::new_v4().to_string();

    let mut params: BTreeMap<&str, String> = BTreeMap::new();
    params.insert("AccessKeyId", key_id.clone());
    params.insert("Action", "CreateToken".into());
    params.insert("Format", "JSON".into());
    params.insert("RegionId", "cn-shanghai".into());
    params.insert("SignatureMethod", "HMAC-SHA1".into());
    params.insert("SignatureNonce", nonce);
    params.insert("SignatureVersion", "1.0".into());
    params.insert("Timestamp", timestamp.clone());
    params.insert("Version", "2019-02-28".into());

    let canonical = build_canonical_query(&params);
    let string_to_sign = format!("GET&{}&{}", percent_encode("/"), percent_encode(&canonical));
    let key = format!("{}&", key_secret);
    let sig = sign_hmac_sha1(&key, &string_to_sign);

    println!("StringToSign: {string_to_sign}");
    println!("Signature: {sig}");

    params.insert("Signature", sig);
    let qs: String = params.iter()
        .map(|(k, v)| format!("{}={}", k, percent_encode(v)))
        .collect::<Vec<_>>().join("&");
    let url = format!("http://nls-meta.cn-shanghai.aliyuncs.com/?{qs}");

    let resp = client.get(&url).send().await.expect("request failed");
    let status = resp.status();
    let body = resp.text().await.expect("body read failed");
    println!("Status: {status}");
    println!("Body: {body}");

    let v: serde_json::Value = serde_json::from_str(&body).expect("json parse failed");
    let token_id = v["Token"]["Id"].as_str().unwrap_or("");
    println!("Token Id: {token_id}");
    assert!(!token_id.is_empty(), "Token.Id should not be empty");
}

fn format_iso8601(unix_secs: u64) -> String {
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
