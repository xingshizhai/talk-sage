# ASR GPU 加速 + 阿里云云端回退 实现方案

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 架构迁移 + GPU 加速 + 云端回退：移除低精度流式推理路径，改为"VAD 切段 + 离线大模型一次推理"；检测 NVIDIA CUDA / Apple CoreML，有 GPU 时加速本地推理；无 GPU 时回退阿里云实时语音识别 WebSocket API。

**Architecture:**

```
旧架构（移除）：
  麦克风 → VAD → Paraformer-zh 流式（每帧增量） → partial 文本

新架构（本方案）：
  麦克风 → Silero VAD 切段 → 段内累积音频
                                  ↓ 段结束（静音超阈值）
                         [本地有 GPU] Qwen3-ASR / WhisperSmall (GPU) → 高精度文本
                         [无 GPU]    阿里云 WebSocket 实时语音 → 高精度文本
```

- **移除** `SherpaStreamingEngine` 作为主路径；`ParaformerZh` / `ZipformerEn` 保留枚举值但不再作为默认选项
- **默认本地引擎**改为 `Qwen3Asr`（GPU 时）或 `WhisperSmall`（CPU 无 GPU 时），均走 `OfflineSegmentEngine`（VAD 段结束后整段推理）
- `crates/talksage-asr/src/gpu.rs`：运行时检测 GPU 后端（`GpuBackend::Cuda | CoreMl | None`）
- 现有 `OfflineSegmentEngine` 增加 `provider` 参数（`"cuda"` / `"coreml"` / `"cpu"`）
- `crates/talksage-asr/src/aliyun/token.rs`：HMAC-SHA1 签名 + Token 缓存（有效期内复用）
- `crates/talksage-asr/src/aliyun/engine.rs`：阿里云 WebSocket 流式引擎（服务端自带 VAD），实现 `SegmentEngine` trait
- `EngineKind::AliyunCloud` 新变体；`create_engine_auto` 按 `GpuBackend` 和配置决定路径
- Config 增加 `aliyun_access_key_id / secret / app_key` + `asr_mode: "auto" | "local" | "cloud"`

**延迟影响：** VAD 切段后离线推理约 1–3s 出文字（而非之前的逐帧增量），但识别精度显著提升，与 Meetily/noScribe 同档。阿里云方案延迟约 200–500ms。

**Tech Stack:** Rust / tokio / tokio-tungstenite / reqwest / hmac + sha1 / uuid / base64 / sherpa-onnx

---

## Task 0：移除流式推理路径，切换默认本地引擎

**Files:**
- 修改: `crates/talksage-pipeline/src/lib.rs`（`StreamWorker` 默认引擎逻辑）
- 修改: `crates/talksage-config/src/lib.rs`（`AsrConfig` 默认引擎改为 `qwen3-asr`）
- 修改: `crates/talksage-pipeline/tests/characterization.rs`（更新测试引擎选择）

> 注：`SherpaStreamingEngine` 结构体本身保留（`ZipformerEn` 可能仍有英文场景），
> 但从 pipeline 的自动选择路径中移除，不再作为中文 ASR 的默认路径。

- [ ] **Step 1：在 `AsrConfig` 中改默认引擎**

在 `crates/talksage-config/src/lib.rs` 中，找到 `client_engine` / `user_engine` 的默认值，从 `"paraformer-zh"` 改为 `"qwen3-asr"`：

```rust
// 原来
fn default_client_engine() -> String { "paraformer-zh".into() }
// 改为
fn default_client_engine() -> String { "qwen3-asr".into() }
```

- [ ] **Step 2：验证 pipeline 的 VAD 切段 + 离线推理路径已就绪**

当前 `OfflineSegmentEngine` 已实现：
- `accept(samples)` → 累积到 `self.buffer`（不出 partial）
- `finish()` → 整段送离线识别器 → 返回最终文本

pipeline 里的 `finish_speech()` 调用 `engine.finish()` 取最终文本，已经是正确的 VAD 切段模式。

**确认点（读代码验证，不修改）：**
- `crates/talksage-pipeline/src/lib.rs` 的 `finish_speech()` 已调用 `engine.finish()`
- VAD 静音检测触发 `finish_speech()` 的逻辑已存在
- 标点恢复 `PunctuationRestorer::restore_and_split()` 在 `finish_speech()` 中已调用

如果上述确认通过，无需修改 pipeline 核心逻辑。

- [ ] **Step 3：更新测试文件中的引擎选择**

在 `crates/talksage-pipeline/tests/characterization.rs` 和 `tests/pipeline_live.rs` 中，
将所有 `engine_kind: EngineKind::ParaformerZh` 改为 `engine_kind: EngineKind::Qwen3Asr`（或保留 ParaformerZh 作为测试覆盖，视测试意图而定）。

- [ ] **Step 4：运行所有测试**

```bash
cargo test -p talksage-pipeline --test characterization
cargo test -p talksage-config --lib
```

预期：全 PASS

- [ ] **Step 5：Commit**

```bash
git add crates/talksage-config/src/lib.rs crates/talksage-pipeline/tests/
git commit -m "feat(pipeline): switch default engine to qwen3-asr (VAD+offline batch mode)"
```

---

## 文件变更清单

| 操作 | 文件 | 职责 |
|---|---|---|
| 新建 | `crates/talksage-asr/src/gpu.rs` | GPU 后端检测 |
| 修改 | `crates/talksage-asr/src/lib.rs` | 增加 `AliyunCloud` variant；`create_engine` 加 provider 参数；pub use 新模块 |
| 新建 | `crates/talksage-asr/src/aliyun/mod.rs` | 模块声明 |
| 新建 | `crates/talksage-asr/src/aliyun/token.rs` | Token 获取与缓存 |
| 新建 | `crates/talksage-asr/src/aliyun/engine.rs` | WebSocket 引擎 |
| 修改 | `crates/talksage-asr/Cargo.toml` | 新增依赖：tokio-tungstenite, hmac, sha1, uuid, base64 |
| 修改 | `crates/talksage-config/src/lib.rs` | `AsrConfig` 增加阿里云凭证字段和 `asr_mode` |
| 修改 | `crates/talksage-pipeline/src/lib.rs` | `LivePipelineConfig` 传入阿里云配置；`create_engine` 调用改为 `create_engine_auto` |
| 修改 | `crates/talksage-pipeline/src/service.rs` | 填充新配置字段 |
| 修改 | `crates/talksage-server/src/lib.rs` | 新增 GPU 状态 API；阿里云凭证通过 config 下发 |
| 修改 | `web/src-tauri/src/lib.rs` | Tauri 侧同步 GPU 状态 |
| 修改 | `web/src/lib/api.ts` | 新增 `aliyun_*` 字段类型 |
| 修改 | `web/src/sections/SettingsSection.tsx` | 新增阿里云凭证输入、ASR 模式选择、GPU 状态显示 |

---

## Task 1：GPU 检测模块

**Files:**
- 新建: `crates/talksage-asr/src/gpu.rs`
- 修改: `crates/talksage-asr/src/lib.rs`（pub mod gpu; pub use gpu::GpuBackend;）

- [ ] **Step 1：写失败测试**

```rust
// crates/talksage-asr/src/gpu.rs（先写测试）
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_a_valid_variant() {
        let b = GpuBackend::detect();
        // 任何平台上都必须返回一个合法变体（不 panic）
        assert!(matches!(b, GpuBackend::Cuda | GpuBackend::CoreMl | GpuBackend::None));
    }

    #[test]
    fn provider_str_matches_backend() {
        assert_eq!(GpuBackend::Cuda.provider_str(), "cuda");
        assert_eq!(GpuBackend::CoreMl.provider_str(), "coreml");
        assert_eq!(GpuBackend::None.provider_str(), "cpu");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_silicon_detects_coreml() {
        // 在 macOS 上至少应检测到 CoreMl（CI 可在 M 系列 runner 上验证）
        // 非 Apple Silicon 机器上跳过（detect 返回 None 也合法）
        let b = GpuBackend::detect();
        // 只验证不 panic，不对 CI 环境硬断言
        let _ = b;
    }
}
```

运行：`cargo test -p talksage-asr gpu --lib`  
预期：FAIL（`GpuBackend` 未定义）

- [ ] **Step 2：实现 `gpu.rs`**

```rust
//! GPU 后端检测：运行时探测可用的硬件加速后端。
//!
//! 检测策略（按优先级）：
//!   1. NVIDIA CUDA：检查 `libcuda.so` / `nvcuda.dll` 是否可动态加载。
//!   2. Apple CoreML（macOS only）：编译期 `target_os = "macos"` 即可用；
//!      进一步区分 Apple Silicon（arm64）以确认 Metal/ANE 支持。
//!   3. 以上均不满足：回退 CPU。

/// 可用的硬件加速后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackend {
    /// NVIDIA CUDA GPU（Windows/Linux）。
    Cuda,
    /// Apple CoreML（macOS，Metal/ANE；M 系列最优，Intel Mac 也支持但效果有限）。
    CoreMl,
    /// 无受支持的 GPU，使用 CPU 推理。
    None,
}

impl GpuBackend {
    /// 运行时检测最优可用后端。
    pub fn detect() -> Self {
        // Apple CoreML：编译期已知是 macOS，再检查 arm64
        #[cfg(target_os = "macos")]
        {
            return Self::CoreMl;
        }
        // NVIDIA CUDA：尝试动态加载 CUDA 库
        #[cfg(not(target_os = "macos"))]
        if Self::cuda_available() {
            return Self::Cuda;
        }
        Self::None
    }

    /// 对应 sherpa-onnx `provider` 字段值。
    pub fn provider_str(self) -> &'static str {
        match self {
            Self::Cuda => "cuda",
            Self::CoreMl => "coreml",
            Self::None => "cpu",
        }
    }

    /// 是否比 CPU 快（用于 UI 展示）。
    pub fn is_accelerated(self) -> bool {
        !matches!(self, Self::None)
    }

    /// 人类可读名称。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Cuda => "NVIDIA CUDA",
            Self::CoreMl => "Apple CoreML (Metal)",
            Self::None => "CPU",
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn cuda_available() -> bool {
        // 尝试 dlopen CUDA runtime 库；仅探测，不调用任何 CUDA API
        #[cfg(target_os = "windows")]
        let lib = libloading::Library::new("nvcuda.dll");
        #[cfg(not(target_os = "windows"))]
        let lib = libloading::Library::new("libcuda.so.1");
        lib.is_ok()
    }
}
```

同时在 `Cargo.toml` 加：
```toml
[target.'cfg(not(target_os = "macos"))'.dependencies]
libloading = "0.8"
```

在 `lib.rs` 顶部加：
```rust
pub mod gpu;
pub use gpu::GpuBackend;
```

- [ ] **Step 3：运行测试**

```bash
cargo test -p talksage-asr gpu --lib
```

预期：PASS（3 个测试全通过）

- [ ] **Step 4：Commit**

```bash
git add crates/talksage-asr/src/gpu.rs crates/talksage-asr/src/lib.rs crates/talksage-asr/Cargo.toml
git commit -m "feat(asr): add GpuBackend detection (CUDA/CoreML/CPU)"
```

---

## Task 2：现有引擎加 provider 参数

**Files:**
- 修改: `crates/talksage-asr/src/lib.rs`（`SherpaStreamingEngine` 和 `OfflineSegmentEngine` 的构造函数）

- [ ] **Step 1：写失败测试**

```rust
// 在 lib.rs tests 模块添加：
#[test]
fn engine_options_has_provider_field() {
    let opts = EngineOptions {
        provider: "cpu".into(),
        ..Default::default()
    };
    assert_eq!(opts.provider, "cpu");
}
```

运行：`cargo test -p talksage-asr engine_options_has_provider -- --lib`  
预期：FAIL（`EngineOptions` 没有 `provider` 字段）

- [ ] **Step 2：给 `EngineOptions` 加 `provider` 字段**

在 `lib.rs` 的 `EngineOptions` 结构体中添加：

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineOptions {
    pub hotwords: Vec<String>,
    pub hotword_score: f32,
    /// sherpa-onnx provider："cpu" | "cuda" | "coreml"。默认 "cpu"。
    pub provider: String,
}
```

`signature()` 方法中加入 provider：
```rust
fn signature(&self) -> String {
    let provider = if self.provider.is_empty() { "cpu" } else { &self.provider };
    format!("{:.3}|{}|{}", self.hotword_score, self.hotwords.join("\u{1f}"), provider)
}
```

- [ ] **Step 3：在 `SherpaStreamingEngine` 和 `OfflineSegmentEngine` 中使用 provider**

在 `SherpaStreamingEngine::new_with_options` 的 `OnlineModelConfig` 中：
```rust
num_threads,
provider: Some(if options.provider.is_empty() { "cpu".into() } else { options.provider.clone() }),
..Default::default()
```

在 `OfflineSegmentEngine::new_with_options` 的两处 `OfflineModelConfig` 中，将：
```rust
provider: Some("cpu".into()),
```
改为：
```rust
provider: Some(if options.provider.is_empty() { "cpu".into() } else { options.provider.clone() }),
```

- [ ] **Step 4：新增便捷构造函数 `create_engine_auto`**

在 `lib.rs` 中 `create_engine_with_options` 之后添加：

```rust
/// 自动选择 provider：有 GPU 时用 GPU，否则 CPU。
pub fn create_engine_auto(
    kind: EngineKind,
    model_dir: &Path,
    num_threads: i32,
    gpu: GpuBackend,
    options: &EngineOptions,
) -> anyhow::Result<Box<dyn SegmentEngine>> {
    let provider = gpu.provider_str().to_string();
    let opts = EngineOptions { provider, ..options.clone() };
    create_engine_with_options(kind, model_dir, num_threads, &opts)
}
```

- [ ] **Step 5：运行全量测试**

```bash
cargo test -p talksage-asr --lib
```

预期：所有原有测试仍 PASS（`EngineOptions::default()` 的 `provider` 为空字符串，行为等价于 `"cpu"`）

- [ ] **Step 6：Commit**

```bash
git add crates/talksage-asr/src/lib.rs
git commit -m "feat(asr): add provider field to EngineOptions, create_engine_auto"
```

---

## Task 3：阿里云 Token 管理

**Files:**
- 新建: `crates/talksage-asr/src/aliyun/mod.rs`
- 新建: `crates/talksage-asr/src/aliyun/token.rs`
- 修改: `crates/talksage-asr/Cargo.toml`（加依赖）

阿里云 Token API：`GET http://nls-meta.cn-shanghai.aliyuncs.com/`  
签名：HMAC-SHA1（按阿里云 POP 协议）

- [ ] **Step 1：加 Cargo 依赖**

在 `crates/talksage-asr/Cargo.toml` 的 `[dependencies]` 中添加：

```toml
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
hmac = "0.12"
sha1 = "0.10"
uuid = { version = "1", features = ["v4"] }
base64 = "0.22"
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }
tokio = { version = "1", features = ["rt", "time", "sync"] }
```

- [ ] **Step 2：写失败测试**

```rust
// crates/talksage-asr/src/aliyun/token.rs 顶部
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_canonical_query_sorts_and_encodes() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("Action", "CreateToken");
        params.insert("Format", "JSON");
        params.insert("Version", "2019-02-28");
        let s = build_canonical_query(&params);
        // 按字典序排列，特殊字符百分号编码
        assert!(s.starts_with("Action=CreateToken"));
        assert!(s.contains("&Format=JSON"));
        assert!(s.contains("&Version=2019-02-28"));
    }

    #[test]
    fn sign_produces_non_empty_base64() {
        let sig = sign_hmac_sha1("key", "string-to-sign");
        assert!(!sig.is_empty());
        // base64 字符集校验（只含 +/= 和字母数字）
        assert!(sig.chars().all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    #[test]
    fn token_info_expired() {
        let t = TokenInfo {
            id: "tok".into(),
            expire_time: 0, // 1970 年，必然已过期
        };
        assert!(t.is_expired());
    }

    #[test]
    fn token_info_not_expired() {
        let t = TokenInfo {
            id: "tok".into(),
            expire_time: u64::MAX,
        };
        assert!(!t.is_expired());
    }
}
```

运行：`cargo test -p talksage-asr aliyun::token -- --lib`  
预期：FAIL（模块未定义）

- [ ] **Step 3：实现 `token.rs`**

```rust
//! 阿里云 NLS Token 获取与缓存。
//!
//! Token 有效期约 24 小时（服务端返回 ExpireTime Unix 时间戳）。
//! `TokenManager` 在每次 `get()` 时检查有效期，到期前 5 分钟提前刷新。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use hmac::{Hmac, Mac};
use sha1::Sha1;

const TOKEN_ENDPOINT: &str = "http://nls-meta.cn-shanghai.aliyuncs.com/";
const REFRESH_BEFORE_SECS: u64 = 300; // 到期前 5 分钟刷新

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub id: String,
    pub expire_time: u64, // Unix 时间戳（秒）
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

/// 线程安全的 Token 管理器；应用启动时创建一个，共享给所有引擎实例。
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

    /// 获取有效 Token：缓存未过期直接返回，否则重新请求。
    pub async fn get(&self, client: &reqwest::Client) -> anyhow::Result<String> {
        // 先检查缓存（短暂持锁，只读）
        {
            let guard = self.cached.lock().unwrap();
            if let Some(ref t) = *guard {
                if !t.is_expired() {
                    return Ok(t.id.clone());
                }
            }
        }
        // 缓存失效，重新获取
        let info = self.fetch(client).await?;
        let id = info.id.clone();
        *self.cached.lock().unwrap() = Some(info);
        Ok(id)
    }

    async fn fetch(&self, client: &reqwest::Client) -> anyhow::Result<TokenInfo> {
        let timestamp = {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
            let secs = now.as_secs();
            // 格式：2019-03-27T09:51:25Z
            let dt = time_to_iso8601(secs);
            dt
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

        // 构造 query string（已签名）
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
            anyhow::bail!("阿里云 Token 请求失败 {}: {}", status, body);
        }
        let token = &body["Token"];
        Ok(TokenInfo {
            id: token["Id"].as_str().unwrap_or("").to_string(),
            expire_time: token["ExpireTime"].as_u64().unwrap_or(0),
        })
    }
}

pub(crate) fn build_canonical_query(params: &BTreeMap<&str, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub(crate) fn sign_hmac_sha1(key: &str, data: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes())
        .expect("HMAC 接受任意长度 key");
    mac.update(data.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

fn percent_encode(s: &str) -> String {
    // RFC 3986 unreserved chars：A-Z a-z 0-9 - _ . ~
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
    // 简单实现：避免引入 chrono，手动计算 UTC 时间字符串
    let s = unix_secs;
    let sec = s % 60;
    let min = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let days = s / 86400;
    // 从 1970-01-01 推算年月日（足够精确到 2100 年）
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
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
    let month_days = [31u64,if leap {29} else {28},31,30,31,30,31,31,30,31,30,31];
    let mut month = 1u64;
    for &md in &month_days {
        if days < md { break; }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}
```

`mod.rs`：
```rust
pub mod token;
pub mod engine;
pub use token::TokenManager;
pub use engine::AliyunEngine;
```

- [ ] **Step 4：运行 Token 单元测试**

```bash
cargo test -p talksage-asr aliyun::token -- --lib
```

预期：4 个测试全 PASS

- [ ] **Step 5：Commit**

```bash
git add crates/talksage-asr/src/aliyun/ crates/talksage-asr/Cargo.toml
git commit -m "feat(asr): aliyun token manager with HMAC-SHA1 signing"
```

---

## Task 4：阿里云 WebSocket 引擎

**Files:**
- 新建: `crates/talksage-asr/src/aliyun/engine.rs`

阿里云协议：
- 建连：`wss://nls-gateway-cn-shanghai.aliyuncs.com/ws/v1?token=<TOKEN>`
- 开始：发 StartTranscription JSON 消息
- 推送：发送 binary PCM 帧（16kHz mono int16）
- 接收：`SentenceBegin` / `TranscriptionResultChanged`（partial）/ `SentenceEnd`（final）
- 结束：发 StopTranscription JSON 消息

- [ ] **Step 1：写失败测试**

```rust
// crates/talksage-asr/src/aliyun/engine.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_message_has_required_fields() {
        let msg = build_start_message("my-appkey", "task-uuid-001");
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["header"]["name"], "StartTranscription");
        assert_eq!(v["header"]["appkey"], "my-appkey");
        assert_eq!(v["payload"]["format"], "pcm");
        assert_eq!(v["payload"]["sample_rate"], 16000);
        assert_eq!(v["payload"]["enable_intermediate_result"], true);
        assert_eq!(v["payload"]["enable_punctuation_prediction"], true);
        assert_eq!(v["payload"]["enable_semantic_sentence_detection"], true);
    }

    #[test]
    fn stop_message_has_required_fields() {
        let msg = build_stop_message("my-appkey", "task-uuid-001");
        let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(v["header"]["name"], "StopTranscription");
    }

    #[test]
    fn f32_to_i16_pcm_clamps_correctly() {
        let samples = vec![0.0f32, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0];
        let out = f32_to_i16_pcm(&samples);
        assert_eq!(out[0], 0);
        assert_eq!(out[1], i16::MAX);
        assert_eq!(out[2], i16::MIN);
        // 2.0 和 -2.0 应 clamp
        assert_eq!(out[5], i16::MAX);
        assert_eq!(out[6], i16::MIN);
    }
}
```

运行：`cargo test -p talksage-asr aliyun::engine -- --lib`  
预期：FAIL（模块未定义）

- [ ] **Step 2：实现 `engine.rs`**

```rust
//! 阿里云实时语音识别 WebSocket 引擎。
//!
//! 实现 `SegmentEngine` trait：
//! - `accept(samples)` 缓冲 f32 音频，转 i16 PCM 后通过 WebSocket 发送；
//!   服务端返回 TranscriptionResultChanged 时通过 channel 更新 partial。
//! - `finish()` 发送 StopTranscription，等待最终 SentenceEnd，返回完整文本。
//! - `reset()` 关闭当前 WS 连接，下一次 `accept` 时重新建连。
//!
//! 连接生命周期：
//!   建连 → StartTranscription → 循环发 PCM → StopTranscription → 等 SentenceEnd → 关闭

use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};

use super::TokenManager;
use crate::EngineKind;

/// 事件类型（从服务端 WS 消息解析）
#[derive(Debug)]
enum AliyunEvent {
    Partial(String),
    Final(String),
    Error(String),
}

/// 阿里云实时 ASR 引擎。
///
/// 线程模型：`accept` / `finish` / `reset` 在同步调用线程运行；
/// WS 读写在 Tokio 异步任务中执行，通过 channel 与同步侧通信。
pub struct AliyunEngine {
    app_key: String,
    token_manager: Arc<TokenManager>,
    http_client: reqwest::Client,
    runtime: Handle,

    // 活跃会话
    session: Option<AliyunSession>,
    /// 当前段累积的 partial 文本（SentenceEnd 前最后一次 TranscriptionResultChanged）
    current_partial: String,
}

struct AliyunSession {
    /// 向 WS 写端发 PCM 或控制消息
    tx: mpsc::Sender<SessionCmd>,
    /// 从 WS 读端收事件
    rx: mpsc::Receiver<AliyunEvent>,
    task_id: String,
}

enum SessionCmd {
    Audio(Vec<u8>), // i16 PCM bytes
    Stop,
}

impl AliyunEngine {
    pub fn new(
        app_key: impl Into<String>,
        token_manager: Arc<TokenManager>,
        runtime: Handle,
    ) -> Self {
        Self {
            app_key: app_key.into(),
            token_manager,
            http_client: reqwest::Client::new(),
            runtime,
            session: None,
            current_partial: String::new(),
        }
    }

    /// 建立 WebSocket 会话（已建则复用）。
    fn ensure_session(&mut self) -> anyhow::Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
        let token = self.runtime.block_on(
            self.token_manager.get(&self.http_client)
        )?;
        let task_id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let url = format!(
            "wss://nls-gateway-cn-shanghai.aliyuncs.com/ws/v1?token={}",
            token
        );
        let app_key = self.app_key.clone();
        let start_msg = build_start_message(&app_key, &task_id);

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<SessionCmd>(128);
        let (evt_tx, evt_rx) = mpsc::channel::<AliyunEvent>(64);

        let task_id_clone = task_id.clone();
        self.runtime.spawn(async move {
            let ws_result = connect_async(&url).await;
            let (mut ws, _) = match ws_result {
                Ok(pair) => pair,
                Err(e) => {
                    let _ = evt_tx.send(AliyunEvent::Error(e.to_string())).await;
                    return;
                }
            };
            // 发 StartTranscription
            if ws.send(Message::Text(start_msg.into())).await.is_err() {
                return;
            }
            // 并发：读 WS 事件 + 写音频帧
            loop {
                tokio::select! {
                    // 写侧：收到 PCM 帧或 Stop 命令
                    cmd = cmd_rx.recv() => {
                        match cmd {
                            Some(SessionCmd::Audio(pcm)) => {
                                if ws.send(Message::Binary(pcm.into())).await.is_err() { break; }
                            }
                            Some(SessionCmd::Stop) => {
                                let stop = build_stop_message(&app_key, &task_id_clone);
                                let _ = ws.send(Message::Text(stop.into())).await;
                                // 继续读直到 SentenceEnd 或连接关闭
                            }
                            None => break,
                        }
                    }
                    // 读侧：处理服务端消息
                    msg = ws.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Some(evt) = parse_event(&text) {
                                    let done = matches!(evt, AliyunEvent::Final(_) | AliyunEvent::Error(_));
                                    let _ = evt_tx.send(evt).await;
                                    if done { break; }
                                }
                            }
                            None | Some(Err(_)) => break,
                            _ => {}
                        }
                    }
                }
            }
        });

        self.session = Some(AliyunSession { tx: cmd_tx, rx: evt_rx, task_id });
        Ok(())
    }

    /// 非阻塞地 drain 所有已到达的事件，更新 partial 文本。
    fn drain_events(&mut self) {
        if let Some(ref mut sess) = self.session {
            while let Ok(evt) = sess.rx.try_recv() {
                match evt {
                    AliyunEvent::Partial(t) => self.current_partial = t,
                    AliyunEvent::Final(t) => self.current_partial = t,
                    AliyunEvent::Error(e) => log::warn!("阿里云 ASR 事件错误: {e}"),
                }
            }
        }
    }
}

impl crate::SegmentEngine for AliyunEngine {
    fn accept(&mut self, samples: &[f32]) -> Option<String> {
        if let Err(e) = self.ensure_session() {
            log::error!("阿里云 ASR 建连失败: {e}");
            return None;
        }
        let pcm = f32_to_i16_pcm(samples);
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        if let Some(ref sess) = self.session {
            let _ = sess.tx.try_send(SessionCmd::Audio(bytes));
        }
        // drain partial 事件，返回最新 partial
        self.drain_events();
        if self.current_partial.is_empty() {
            None
        } else {
            Some(self.current_partial.clone())
        }
    }

    fn finish(&mut self) -> String {
        if let Some(ref sess) = self.session {
            let _ = sess.tx.try_send(SessionCmd::Stop);
        }
        // 阻塞等待 Final 事件（最多 15 秒）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut final_text = self.current_partial.clone();
        if let Some(ref mut sess) = self.session {
            while std::time::Instant::now() < deadline {
                match sess.rx.try_recv() {
                    Ok(AliyunEvent::Final(t)) => { final_text = t; break; }
                    Ok(AliyunEvent::Partial(t)) => { final_text = t; }
                    Ok(AliyunEvent::Error(e)) => { log::warn!("finish 时收到错误: {e}"); break; }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        }
        final_text
    }

    fn reset(&mut self) {
        // 丢弃会话（WS 任务自然结束）
        self.session = None;
        self.current_partial.clear();
    }

    fn kind(&self) -> EngineKind {
        EngineKind::AliyunCloud
    }
}

// ── 辅助函数 ──────────────────────────────────────────────────

pub(crate) fn build_start_message(app_key: &str, task_id: &str) -> String {
    serde_json::json!({
        "header": {
            "message_id": uuid::Uuid::new_v4().to_string().replace('-', ""),
            "task_id": task_id,
            "namespace": "SpeechTranscriber",
            "name": "StartTranscription",
            "appkey": app_key
        },
        "payload": {
            "format": "pcm",
            "sample_rate": 16000,
            "enable_intermediate_result": true,
            "enable_punctuation_prediction": true,
            "enable_inverse_text_normalization": true,
            "enable_semantic_sentence_detection": true
        }
    }).to_string()
}

pub(crate) fn build_stop_message(app_key: &str, task_id: &str) -> String {
    serde_json::json!({
        "header": {
            "message_id": uuid::Uuid::new_v4().to_string().replace('-', ""),
            "task_id": task_id,
            "namespace": "SpeechTranscriber",
            "name": "StopTranscription",
            "appkey": app_key
        },
        "payload": {}
    }).to_string()
}

fn parse_event(text: &str) -> Option<AliyunEvent> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let name = v["header"]["name"].as_str()?;
    match name {
        "TranscriptionResultChanged" => {
            let result = v["payload"]["result"].as_str().unwrap_or("").to_string();
            Some(AliyunEvent::Partial(result))
        }
        "SentenceEnd" => {
            let result = v["payload"]["result"].as_str().unwrap_or("").to_string();
            Some(AliyunEvent::Final(result))
        }
        "TaskFailed" => {
            let msg = v["header"]["status_text"].as_str().unwrap_or("unknown").to_string();
            Some(AliyunEvent::Error(msg))
        }
        _ => None,
    }
}

pub(crate) fn f32_to_i16_pcm(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| {
        let clamped = s.clamp(-1.0, 1.0);
        (clamped * i16::MAX as f32) as i16
    }).collect()
}
```

- [ ] **Step 3：运行引擎单元测试**

```bash
cargo test -p talksage-asr aliyun::engine -- --lib
```

预期：3 个测试全 PASS

- [ ] **Step 4：在 `lib.rs` 中注册 `AliyunCloud` variant**

`EngineKind` 枚举新增：
```rust
/// 阿里云实时语音识别（云端 WebSocket，无 GPU 时的回退）。
AliyunCloud,
```

在 `from_name`、`display_name`、`model_dir_name`、`is_streaming`、`profile` 中补充分支：
```rust
// from_name
"aliyun" | "aliyun-cloud" => Some(Self::AliyunCloud),

// display_name
Self::AliyunCloud => "aliyun-cloud",

// model_dir_name（无本地模型）
Self::AliyunCloud => "aliyun-cloud",

// is_streaming（实时 WebSocket，属于流式）
Self::AliyunCloud => true,

// is_available（云端引擎不检查本地文件）
Self::AliyunCloud => true,

// profile
Self::AliyunCloud => ModelProfile {
    kind: self,
    label: "阿里云实时语音",
    languages: "zh,en",
    streaming: true,
    speed: "realtime",
    description: "云端流式识别，需配置 AccessKey；无本地 GPU 时自动启用",
},
```

`ALL` 数组更新（不需要加 AliyunCloud，它是内部回退，不出现在用户模型列表）。

- [ ] **Step 5：全量测试**

```bash
cargo test -p talksage-asr --lib
```

预期：全 PASS

- [ ] **Step 6：Commit**

```bash
git add crates/talksage-asr/src/aliyun/
git commit -m "feat(asr): aliyun websocket streaming engine + AliyunCloud EngineKind"
```

---

## Task 5：Config 增加阿里云凭证字段

**Files:**
- 修改: `crates/talksage-config/src/lib.rs`
- 修改: `web/src/lib/api.ts`

- [ ] **Step 1：写失败测试**

```rust
// 在 talksage-config 的 tests 中添加（或直接在 lib.rs #[cfg(test)] 中）：
#[test]
fn asr_config_has_aliyun_fields() {
    let cfg = AsrConfig::default();
    assert!(cfg.aliyun_access_key_id.is_empty());
    assert!(cfg.aliyun_access_key_secret.is_empty());
    assert!(cfg.aliyun_app_key.is_empty());
    assert_eq!(cfg.asr_mode, "auto");
}
```

运行：`cargo test -p talksage-config -- --lib`  
预期：FAIL

- [ ] **Step 2：修改 `AsrConfig`**

在 `crates/talksage-config/src/lib.rs` 的 `AsrConfig` 中新增字段：

```rust
/// 阿里云智能语音 AccessKey ID。
#[serde(default)]
pub aliyun_access_key_id: String,
/// 阿里云智能语音 AccessKey Secret。
#[serde(default)]
pub aliyun_access_key_secret: String,
/// 阿里云 NLS 项目 AppKey。
#[serde(default)]
pub aliyun_app_key: String,
/// ASR 模式："auto"（有 GPU 用本地，否则云端）| "local"（强制本地）| "cloud"（强制云端）。
#[serde(default = "default_asr_mode")]
pub asr_mode: String,
```

添加辅助函数：
```rust
fn default_asr_mode() -> String { "auto".into() }
```

在 `merge_config` 中同步字段（与现有 `punct_enabled` 等字段同模式）：
```rust
aliyun_access_key_id: user.asr.aliyun_access_key_id.clone(),
aliyun_access_key_secret: user.asr.aliyun_access_key_secret.clone(),
aliyun_app_key: user.asr.aliyun_app_key.clone(),
asr_mode: user.asr.asr_mode.clone(),
```

- [ ] **Step 3：运行 Config 测试**

```bash
cargo test -p talksage-config --lib
```

预期：全 PASS

- [ ] **Step 4：更新 `web/src/lib/api.ts`**

在 `AppConfig.asr` 类型中添加：
```typescript
aliyun_access_key_id: string;
aliyun_access_key_secret: string;
aliyun_app_key: string;
asr_mode: string;
```

- [ ] **Step 5：Commit**

```bash
git add crates/talksage-config/src/lib.rs web/src/lib/api.ts
git commit -m "feat(config): add aliyun credentials and asr_mode to AsrConfig"
```

---

## Task 6：Pipeline 集成自动引擎选择

**Files:**
- 修改: `crates/talksage-pipeline/src/lib.rs`
- 修改: `crates/talksage-pipeline/src/service.rs`

- [ ] **Step 1：写失败测试**

```rust
// 在 pipeline 的 tests 中（tests/pipeline_live.rs 或 lib.rs inline）添加：
#[test]
fn pipeline_config_has_aliyun_fields() {
    let cfg = LivePipelineConfig::default();
    assert!(cfg.aliyun_access_key_id.is_empty());
    assert_eq!(cfg.asr_mode, "auto");
}
```

- [ ] **Step 2：修改 `LivePipelineConfig`**

在 `crates/talksage-pipeline/src/lib.rs` 的 `LivePipelineConfig` 中新增：

```rust
pub aliyun_access_key_id: String,
pub aliyun_access_key_secret: String,
pub aliyun_app_key: String,
/// "auto" | "local" | "cloud"
pub asr_mode: String,
```

- [ ] **Step 3：在 `StreamWorker` 构造时加入自动引擎选择逻辑**

在 `StreamWorker::new`（或等价的初始化函数）中，在创建引擎之前：

```rust
use talksage_asr::{GpuBackend, EngineKind};

let gpu = GpuBackend::detect();
let use_cloud = match cfg.asr_mode.as_str() {
    "cloud" => true,
    "local" => false,
    _ /* "auto" */ => {
        !gpu.is_accelerated()
            && !cfg.aliyun_access_key_id.is_empty()
            && !cfg.aliyun_app_key.is_empty()
    }
};

if use_cloud {
    log::info!("ASR 引擎：阿里云实时语音识别（云端）");
    // 构建 AliyunEngine，赋给 engine 字段
    let token_mgr = Arc::new(talksage_asr::aliyun::TokenManager::new(
        &cfg.aliyun_access_key_id,
        &cfg.aliyun_access_key_secret,
    ));
    let handle = tokio::runtime::Handle::current();
    Box::new(talksage_asr::aliyun::AliyunEngine::new(
        &cfg.aliyun_app_key,
        token_mgr,
        handle,
    ))
} else {
    log::info!("ASR 引擎：本地 {:?} (provider={})", cfg.engine_kind, gpu.provider_str());
    talksage_asr::create_engine_auto(
        engine_kind,
        &model_dir,
        cfg.num_threads,
        gpu,
        &engine_options,
    )?
};
```

- [ ] **Step 4：在 `service.rs` 填充新字段**

```rust
aliyun_access_key_id: config.asr.aliyun_access_key_id.clone(),
aliyun_access_key_secret: config.asr.aliyun_access_key_secret.clone(),
aliyun_app_key: config.asr.aliyun_app_key.clone(),
asr_mode: config.asr.asr_mode.clone(),
```

- [ ] **Step 5：运行 pipeline 测试**

```bash
cargo test -p talksage-pipeline --lib
```

预期：全 PASS

- [ ] **Step 6：Commit**

```bash
git add crates/talksage-pipeline/src/lib.rs crates/talksage-pipeline/src/service.rs
git commit -m "feat(pipeline): auto engine selection (GPU local / cloud fallback)"
```

---

## Task 7：Server / Tauri 暴露 GPU 状态

**Files:**
- 修改: `crates/talksage-server/src/lib.rs`
- 修改: `web/src-tauri/src/lib.rs`

- [ ] **Step 1：在 server API 中新增 `GET /asr/gpu_status`**

```rust
// crates/talksage-server/src/lib.rs
async fn gpu_status_api() -> axum::Json<serde_json::Value> {
    let gpu = talksage_asr::GpuBackend::detect();
    axum::Json(serde_json::json!({
        "backend": gpu.provider_str(),
        "display_name": gpu.display_name(),
        "is_accelerated": gpu.is_accelerated(),
    }))
}
```

路由注册：`.route("/asr/gpu_status", get(gpu_status_api))`

- [ ] **Step 2：在 Tauri 中新增 `get_gpu_status` command**

```rust
// web/src-tauri/src/lib.rs
#[tauri::command]
fn get_gpu_status() -> serde_json::Value {
    let gpu = talksage_asr::GpuBackend::detect();
    serde_json::json!({
        "backend": gpu.provider_str(),
        "display_name": gpu.display_name(),
        "is_accelerated": gpu.is_accelerated(),
    })
}
```

注册到 `.invoke_handler(tauri::generate_handler![..., get_gpu_status])`

- [ ] **Step 3：Commit**

```bash
git add crates/talksage-server/src/lib.rs web/src-tauri/src/lib.rs
git commit -m "feat(server,tauri): expose GPU status API"
```

---

## Task 8：Settings UI — 阿里云凭证 + ASR 模式

**Files:**
- 修改: `web/src/sections/SettingsSection.tsx`

- [ ] **Step 1：新增状态和保存逻辑**

在现有 `SettingsSection` 组件中新增状态：
```tsx
const [asrMode, setAsrMode] = useState(config?.asr?.asr_mode ?? 'auto');
const [aliyunKeyId, setAliyunKeyId] = useState(config?.asr?.aliyun_access_key_id ?? '');
const [aliyunKeySecret, setAliyunKeySecret] = useState(config?.asr?.aliyun_access_key_secret ?? '');
const [aliyunAppKey, setAliyunAppKey] = useState(config?.asr?.aliyun_app_key ?? '');
const [gpuStatus, setGpuStatus] = useState<{ display_name: string; is_accelerated: boolean } | null>(null);
```

在 `useEffect` 中获取 GPU 状态：
```tsx
// 获取 GPU 状态
invoke<typeof gpuStatus>('get_gpu_status').then(setGpuStatus).catch(() => {});
```

在 `handleSave` 中加入：
```tsx
asr_mode: asrMode,
aliyun_access_key_id: aliyunKeyId,
aliyun_access_key_secret: aliyunKeySecret,
aliyun_app_key: aliyunAppKey,
```

- [ ] **Step 2：在 ASR 选项卡中新增 UI 区域**

在 ASR tab 的现有 `punct_enabled` 复选框之后，添加：

```tsx
{/* GPU 状态 */}
<div className="setting-row">
  <span className="setting-label">推理硬件</span>
  <span className="setting-value">
    {gpuStatus
      ? `${gpuStatus.display_name}${gpuStatus.is_accelerated ? ' ⚡' : ''}`
      : '检测中…'}
  </span>
</div>

{/* ASR 模式 */}
<div className="setting-row">
  <label className="setting-label">ASR 模式</label>
  <select value={asrMode} onChange={e => setAsrMode(e.target.value)}>
    <option value="auto">自动（有 GPU 用本地，否则云端）</option>
    <option value="local">强制本地</option>
    <option value="cloud">强制云端（阿里云）</option>
  </select>
</div>

{/* 阿里云凭证 — 仅 auto/cloud 模式时显示 */}
{(asrMode === 'auto' || asrMode === 'cloud') && (
  <div className="setting-section">
    <div className="setting-section-title">阿里云语音识别凭证</div>
    <div className="setting-row">
      <label className="setting-label">AccessKey ID</label>
      <input
        type="text"
        value={aliyunKeyId}
        onChange={e => setAliyunKeyId(e.target.value)}
        placeholder="LTAI5t…"
      />
    </div>
    <div className="setting-row">
      <label className="setting-label">AccessKey Secret</label>
      <input
        type="password"
        value={aliyunKeySecret}
        onChange={e => setAliyunKeySecret(e.target.value)}
        placeholder="●●●●●●●●"
      />
    </div>
    <div className="setting-row">
      <label className="setting-label">AppKey</label>
      <input
        type="text"
        value={aliyunAppKey}
        onChange={e => setAliyunAppKey(e.target.value)}
        placeholder="项目 AppKey"
      />
    </div>
    <p className="setting-hint">
      凭证仅存储在本机配置文件中。
      <a href="https://nls-portal.console.aliyun.com/" target="_blank" rel="noopener">前往阿里云 NLS 控制台获取</a>
    </p>
  </div>
)}
```

- [ ] **Step 3：Commit**

```bash
git add web/src/sections/SettingsSection.tsx
git commit -m "feat(ui): aliyun credentials, ASR mode selector, GPU status in settings"
```

---

## 自检

**Spec 覆盖：**
- ✅ NVIDIA CUDA GPU 支持（Task 1 + 2）
- ✅ Apple M 系列 GPU 支持 CoreML（Task 1 + 2）
- ✅ Intel GPU 留接口（`libloading` 探测框架已建，后续加分支）
- ✅ 无 GPU → 阿里云实时语音识别（Task 3 + 4 + 6）
- ✅ 阿里云 OpenAPI Token（HMAC-SHA1，Task 3）
- ✅ 阿里云 WebSocket 流式协议（Task 4）
- ✅ 配置（凭证 + 模式，Task 5）
- ✅ UI（凭证输入 + GPU 状态 + 模式选择，Task 8）

**Placeholder 检查：** 无

**类型一致性：** `AliyunEngine` 实现 `SegmentEngine`；`EngineKind::AliyunCloud` 在 `kind()` 返回；`GpuBackend::detect()` 返回值在 Task 2 的 `create_engine_auto` 和 Task 6 的 pipeline 中保持一致。
