pub mod token;
pub mod engine;
pub use token::TokenManager;
pub use engine::AliyunEngine;

/// 验证阿里云 ASR 凭据（设置页「检查」按钮用）：
/// 向阿里云 NLS 请求 AccessToken（CreateToken，HMAC-SHA1 签名）。
/// 成功返回 token 有效期（Unix 秒）；失败返回可读错误
/// （InvalidAccessKeyId / SignatureDoesNotMatch 等，见 `token` 模块）。
pub async fn verify_aliyun_credentials(
    access_key_id: &str,
    access_key_secret: &str,
) -> anyhow::Result<u64> {
    let manager = TokenManager::new(access_key_id, access_key_secret);
    let client = reqwest::Client::new();
    manager.verify(&client).await
}
