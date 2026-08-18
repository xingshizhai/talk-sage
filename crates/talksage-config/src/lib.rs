//! TalkSage v2 分层配置。
//!
//! 分层合并（简化版 DSH patch 思想）：
//!   内置默认 → 用户 `talksage.toml` → 环境变量（`TALKSAGE_*`）→ 代码调用方覆盖。
//!
//! M0 范围：内置默认 + 用户文件覆盖 + 环境变量覆盖（基础键）。

use std::env;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// 配置错误。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("读取配置文件失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("解析配置文件失败: {0}")]
    Parse(#[from] toml::de::Error),
}

/// 用户数据根目录（`TALKSAGE_DATA_DIR` 优先，默认 `~/.talksage`）。
pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = env::var("TALKSAGE_DATA_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs_home().join(".talksage")
}

/// 跨平台用户主目录。
fn dirs_home() -> PathBuf {
    if let Ok(h) = env::var("USERPROFILE") {
        return PathBuf::from(h);
    }
    if let Ok(h) = env::var("HOME") {
        return PathBuf::from(h);
    }
    PathBuf::from(".")
}

/// 顶层配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub asr: AsrConfig,
    pub audio: AudioConfig,
    pub llm: LlmConfig,
    pub plugins: PluginsConfig,
    pub session: SessionConfig,
    pub privacy: PrivacyConfig,
    pub server: ServerConfig,
    pub knowledge_base: KnowledgeBaseConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            asr: AsrConfig::default(),
            audio: AudioConfig::default(),
            llm: LlmConfig::default(),
            plugins: PluginsConfig::default(),
            session: SessionConfig::default(),
            privacy: PrivacyConfig::default(),
            server: ServerConfig::default(),
            knowledge_base: KnowledgeBaseConfig::default(),
        }
    }
}

/// 客户简报知识库配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeBaseConfig {
    pub enabled: bool,
    pub folder: String,
}

impl Default for KnowledgeBaseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            folder: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrConfig {
    /// 客户（英文）流式引擎。
    pub client_engine: String,
    /// 用户（中文）流式引擎。
    pub user_engine: String,
    /// 推理后端：auto | cpu | cuda | metal。
    pub backend: String,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            client_engine: "zipformer-en".into(),
            user_engine: "paraformer-zh".into(),
            backend: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub mic_device: Option<i32>,
    pub loopback_device: Option<i32>,
    pub ducking: DuckingConfig,
    pub vad: VadConfig,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mic_device: None,
            loopback_device: None,
            ducking: DuckingConfig::default(),
            vad: VadConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckingConfig {
    pub enabled: bool,
    pub threshold: f32,
    pub factor: f32,
}

impl Default for DuckingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.04,
            factor: 0.35,
        }
    }
}

/// 流式 VAD 参数（参考 Meetily 调优经验）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub redemption_ms: u64,
    pub pre_pad_ms: u64,
    pub post_pad_ms: u64,
    pub min_speech_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            redemption_ms: 2000,
            pre_pad_ms: 300,
            post_pad_ms: 400,
            min_speech_ms: 250,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub default: String,
    pub providers: std::collections::HashMap<String, LlmProviderConfig>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        let mut providers = std::collections::HashMap::new();
        providers.insert(
            "deepseek".into(),
            LlmProviderConfig {
                base_url: Some("https://api.deepseek.com/v1".into()),
                model: "deepseek-chat".into(),
                api_key: String::new(),
            },
        );
        providers.insert(
            "ollama".into(),
            LlmProviderConfig {
                base_url: Some("http://localhost:11434/v1".into()),
                model: "llama3".into(),
                api_key: "ollama".into(),
            },
        );
        Self {
            default: "deepseek".into(),
            providers,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmProviderConfig {
    pub base_url: Option<String>,
    pub model: String,
    pub api_key: String,
}

impl Default for LlmProviderConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            model: "deepseek-chat".into(),
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    pub term_explainer: PluginToggle,
    pub translator: PluginToggle,
    pub brief_retriever: PluginToggle,
    pub notes: NotesConfig,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            term_explainer: PluginToggle {
                enabled: true,
                cooldown_seconds: 10.0,
            },
            translator: PluginToggle {
                enabled: true,
                cooldown_seconds: 3.0,
            },
            brief_retriever: PluginToggle {
                enabled: true,
                cooldown_seconds: 15.0,
            },
            notes: NotesConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginToggle {
    pub enabled: bool,
    pub cooldown_seconds: f32,
}

impl Default for PluginToggle {
    fn default() -> Self {
        Self {
            enabled: true,
            cooldown_seconds: 10.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotesConfig {
    pub template: String,
}

impl Default for NotesConfig {
    fn default() -> Self {
        Self {
            template: "standard_meeting".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub sqlite: bool,
    pub export_markdown: bool,
    pub record_audio: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            sqlite: true,
            export_markdown: true,
            record_audio: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub recording_consent_accepted: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            recording_consent_accepted: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub token: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: "127.0.0.1".into(),
            port: 8080,
            token: String::new(),
        }
    }
}

/// 配置管理器：负责分层加载与查询。
#[derive(Debug, Clone)]
pub struct ConfigManager {
    data_dir: PathBuf,
    config: Config,
}

impl ConfigManager {
    /// 从默认配置 + 用户文件加载。
    ///
    /// `file` 为 None 时使用 `<data_dir>/talksage.toml`（不存在则仅默认值）。
    pub fn load(data_dir: Option<PathBuf>, file: Option<&Path>) -> Result<Self, ConfigError> {
        let data_dir = data_dir.unwrap_or_else(default_data_dir);
        let mut config = Config::default();
        let path = file
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| data_dir.join("talksage.toml"));
        if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            let user: Config = toml::from_str(&raw)?;
            config = merge_config(config, user);
        }
        apply_env_overrides(&mut config);
        Ok(Self { data_dir, config })
    }

    /// 直接以自定义目录构建（测试 / headless 模式用）。
    pub fn from_config(config: Config, data_dir: PathBuf) -> Self {
        Self { data_dir, config }
    }

    /// 完整配置快照。
    pub fn snapshot(&self) -> &Config {
        &self.config
    }

    /// 数据目录（会话、录音、数据库所在）。
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// 用户配置字段覆盖默认配置（非 None/非默认值时生效）。
fn merge_config(default: Config, user: Config) -> Config {
    Config {
        asr: AsrConfig {
            client_engine: take_or(user.asr.client_engine, default.asr.client_engine),
            user_engine: take_or(user.asr.user_engine, default.asr.user_engine),
            backend: take_or(user.asr.backend, default.asr.backend),
        },
        audio: AudioConfig {
            mic_device: user.audio.mic_device.or(default.audio.mic_device),
            loopback_device: user.audio.loopback_device.or(default.audio.loopback_device),
            ducking: DuckingConfig {
                enabled: user.audio.ducking.enabled,
                threshold: user.audio.ducking.threshold,
                factor: user.audio.ducking.factor,
            },
            vad: VadConfig {
                redemption_ms: user.audio.vad.redemption_ms,
                pre_pad_ms: user.audio.vad.pre_pad_ms,
                post_pad_ms: user.audio.vad.post_pad_ms,
                min_speech_ms: user.audio.vad.min_speech_ms,
            },
        },
        llm: LlmConfig {
            default: take_or(user.llm.default, default.llm.default),
            providers: {
                let mut merged = default.llm.providers;
                for (k, v) in user.llm.providers {
                    merged.insert(k, v);
                }
                merged
            },
        },
        plugins: PluginsConfig {
            term_explainer: user.plugins.term_explainer,
            translator: user.plugins.translator,
            brief_retriever: user.plugins.brief_retriever,
            notes: user.plugins.notes,
        },
        session: user.session,
        privacy: user.privacy,
        server: user.server,
        knowledge_base: user.knowledge_base,
    }
}

fn take_or(user: String, default: String) -> String {
    if user.trim().is_empty() {
        default
    } else {
        user
    }
}

/// 环境变量覆盖：`TALKSAGE_<KEY>`，支持 asr.backend / server.port / server.host / llm.default 等。
fn apply_env_overrides(cfg: &mut Config) {
    if let Ok(v) = env::var("TALKSAGE_ASR_BACKEND") {
        if !v.trim().is_empty() {
            cfg.asr.backend = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("TALKSAGE_SERVER_HOST") {
        if !v.trim().is_empty() {
            cfg.server.host = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("TALKSAGE_SERVER_PORT") {
        if let Ok(p) = v.trim().parse() {
            cfg.server.port = p;
        }
    }
    if let Ok(v) = env::var("TALKSAGE_SERVER_TOKEN") {
        cfg.server.token = v;
    }
    if let Ok(v) = env::var("TALKSAGE_SERVER_ENABLED") {
        if let Ok(b) = v.trim().parse() {
            cfg.server.enabled = b;
        }
    }
    if let Ok(v) = env::var("TALKSAGE_LLM_DEFAULT") {
        if !v.trim().is_empty() {
            cfg.llm.default = v.trim().to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_file() {
        let mgr = ConfigManager::load(None, None).unwrap();
        let c = mgr.snapshot();
        assert_eq!(c.asr.user_engine, "paraformer-zh");
        assert_eq!(c.server.host, "127.0.0.1");
        assert!(!c.server.enabled);
        assert_eq!(c.audio.vad.redemption_ms, 2000);
    }

    #[test]
    fn user_file_overrides_defaults() {
        let dir = std::env::temp_dir().join(format!("talksage-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talksage.toml");
        std::fs::write(
            &file,
            r#"
[asr]
backend = "cuda"

[server]
enabled = true
port = 9090
"#,
        )
        .unwrap();
        let mgr = ConfigManager::load(None, Some(&file)).unwrap();
        let c = mgr.snapshot();
        assert_eq!(c.asr.backend, "cuda");
        assert!(c.server.enabled);
        assert_eq!(c.server.port, 9090);
        // 未覆盖字段保持默认
        assert_eq!(c.asr.user_engine, "paraformer-zh");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_overrides_win() {
        unsafe {
            std::env::set_var("TALKSAGE_SERVER_PORT", "7070");
        }
        let mgr = ConfigManager::load(None, None).unwrap();
        assert_eq!(mgr.snapshot().server.port, 7070);
        unsafe {
            std::env::remove_var("TALKSAGE_SERVER_PORT");
        }
    }
}
