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
    #[error("序列化配置失败: {0}")]
    Ser(#[from] toml::ser::Error),
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
    pub recording: RecordingConfig,
    pub quality: QualityConfig,
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
            recording: RecordingConfig::default(),
            quality: QualityConfig::default(),
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
    pub denoise: DenoiseConfig,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mic_device: None,
            loopback_device: None,
            ducking: DuckingConfig::default(),
            vad: VadConfig::default(),
            denoise: DenoiseConfig::default(),
        }
    }
}

/// 识别灵敏度预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VadPreset {
    /// 标准：平衡灵敏度与抗噪。
    Standard,
    /// 灵敏：捕获弱语音/短句（会议室轻声、快速问答）。
    Sensitive,
    /// 严格：抗背景噪音，长段稳定。
    Strict,
}

impl Default for VadPreset {
    fn default() -> Self {
        Self::Standard
    }
}

impl VadPreset {
    /// 预设参数 (threshold, min_speech_s, min_silence_s, window, max_speech_s)。
    pub fn params(&self) -> (f32, f32, f32, i32, f32) {
        match self {
            VadPreset::Standard => (0.50, 0.25, 0.50, 512, 10.0),
            VadPreset::Sensitive => (0.35, 0.15, 0.30, 512, 10.0),
            VadPreset::Strict => (0.65, 0.35, 0.80, 512, 10.0),
        }
    }
}

/// 流式 VAD 参数（预设 + 可覆盖）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    /// 灵敏度预设。
    pub preset: VadPreset,
    /// 覆盖：检测阈值（None = 用预设）。
    pub threshold: Option<f32>,
    /// 覆盖：最小语音时长（秒）。
    pub min_speech_ms: Option<u64>,
    /// 覆盖：段结束静音（秒）。
    pub min_silence_ms: Option<u64>,
    /// 覆盖：最长语音（秒）。
    pub max_speech_ms: Option<u64>,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            preset: VadPreset::Standard,
            threshold: None,
            min_speech_ms: None,
            min_silence_ms: None,
            max_speech_ms: None,
        }
    }
}

impl VadConfig {
    /// 解析实际生效参数。
    pub fn effective(&self) -> (f32, f32, f32, i32, f32) {
        let (t, ms, msil, w, maxs) = self.preset.params();
        (
            self.threshold.unwrap_or(t),
            self.min_speech_ms.map(|v| v as f32 / 1000.0).unwrap_or(ms),
            self.min_silence_ms.map(|v| v as f32 / 1000.0).unwrap_or(msil),
            w,
            self.max_speech_ms.map(|v| v as f32 / 1000.0).unwrap_or(maxs),
        )
    }
}

/// 音频预处理（背景噪音处理）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DenoiseConfig {
    /// 总开关。
    pub enabled: bool,
    /// 噪声门：低于该 RMS 的块视为静音（抑制稳态背景噪音）。
    pub gate_threshold: f32,
    /// 高通滤波开关（去除低频轰鸣/空调声）。
    pub highpass: bool,
    /// 高通截止频率（Hz）。
    pub highpass_cutoff_hz: f32,
}

impl Default for DenoiseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gate_threshold: 0.008,
            highpass: true,
            highpass_cutoff_hz: 100.0,
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

/// 会议录音配置（边用边录，形成"录制 → 裁剪 → 回放验证"闭环）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    /// 监听时是否自动保存录音。
    pub enabled: bool,
    /// 录音目录（相对 data_dir 或绝对路径；空 = `<data_dir>/recordings`）。
    pub dir: String,
    /// 是否自动做静音裁剪（预留：当前由 `talksage trim` 手动完成）。
    pub clean_silence: bool,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: String::new(),
            clean_silence: false,
        }
    }
}

impl RecordingConfig {
    /// 解析录音目录：相对 data_dir 解析；绝对路径原样返回。
    pub fn resolve_dir(&self, data_dir: &Path) -> PathBuf {
        let p = PathBuf::from(&self.dir);
        if p.is_absolute() {
            p
        } else if self.dir.trim().is_empty() {
            data_dir.join("recordings")
        } else {
            data_dir.join(p)
        }
    }
}

/// 会话质量评估配置（噪音检测阈值）。
///
/// 用于判定"有效语音 / 噪音 / 静音"会话并决定是否跳过下游分析。
/// `auto_detect = true` 时，能量类阈值（silence_rms / high_rms）会根据
/// 会话中非语音块的背景噪音水平自动计算，覆盖手工设定值。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QualityConfig {
    /// 自动检测背景噪音并自动设置能量阈值。
    pub auto_detect: bool,
    /// 文本噪音评分阈值（0..1）：段文本噪音分高于此值判噪音。默认 0.45。
    pub text_noise_threshold: f32,
    /// 静音判定：语音占比低于此值（0..1）。默认 0.15。
    pub min_speech_ratio: f32,
    /// 噪音判定：语音占比高于此值（0..1，几乎不停顿 = 持续噪音/旁人说话）。默认 0.85。
    pub max_speech_ratio: f32,
    /// 静音能量阈值（avg_rms 低于此值且无语音 → 静音）。默认 0.01。
    pub silence_rms: f32,
    /// 高能量噪音阈值（avg_rms 高于此值 → 环境噪音大）。默认 0.5。
    pub high_rms: f32,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            auto_detect: true,
            text_noise_threshold: 0.45,
            min_speech_ratio: 0.15,
            max_speech_ratio: 0.85,
            silence_rms: 0.01,
            high_rms: 0.5,
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

/// 配置管理器：负责分层加载、运行时更新与持久化。
pub struct ConfigManager {
    data_dir: PathBuf,
    config: std::sync::RwLock<Config>,
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
        Ok(Self {
            data_dir,
            config: std::sync::RwLock::new(config),
        })
    }

    /// 直接以自定义目录构建（测试 / headless 模式用）。
    pub fn from_config(config: Config, data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            config: std::sync::RwLock::new(config),
        }
    }

    /// 完整配置快照（克隆，线程安全）。
    pub fn snapshot(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    /// 数据目录（会话、录音、数据库所在）。
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 运行时更新配置（回调内修改），并立即持久化到 `talksage.toml`。
    pub fn update<R>(&self, f: impl FnOnce(&mut Config) -> R) -> Result<R, ConfigError> {
        let mut config = self.config.write().unwrap();
        let result = f(&mut config);
        let raw = toml::to_string(&*config)?;
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::write(self.data_dir.join("talksage.toml"), raw)?;
        Ok(result)
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
                preset: user.audio.vad.preset,
                threshold: user.audio.vad.threshold.or(default.audio.vad.threshold),
                min_speech_ms: user.audio.vad.min_speech_ms.or(default.audio.vad.min_speech_ms),
                min_silence_ms: user.audio.vad.min_silence_ms.or(default.audio.vad.min_silence_ms),
                max_speech_ms: user.audio.vad.max_speech_ms.or(default.audio.vad.max_speech_ms),
            },
            denoise: DenoiseConfig {
                enabled: user.audio.denoise.enabled,
                gate_threshold: user.audio.denoise.gate_threshold,
                highpass: user.audio.denoise.highpass,
                highpass_cutoff_hz: user.audio.denoise.highpass_cutoff_hz,
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
        recording: RecordingConfig {
            enabled: user.recording.enabled,
            dir: take_or(user.recording.dir, default.recording.dir),
            clean_silence: user.recording.clean_silence,
        },
        quality: QualityConfig {
            auto_detect: user.quality.auto_detect,
            text_noise_threshold: user.quality.text_noise_threshold,
            min_speech_ratio: user.quality.min_speech_ratio,
            max_speech_ratio: user.quality.max_speech_ratio,
            silence_rms: user.quality.silence_rms,
            high_rms: user.quality.high_rms,
        },
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
        assert_eq!(c.audio.vad.effective(), (0.50, 0.25, 0.50, 512, 10.0));
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

    #[test]
    fn update_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("talksage-cfg-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = ConfigManager::load(Some(dir.clone()), None).unwrap();
        mgr.update(|c| {
            c.llm.default = "kimi".into();
            c.plugins.translator.enabled = false;
        })
        .unwrap();

        // 重新加载同一目录，应读到更新后的值
        let reloaded = ConfigManager::load(Some(dir.clone()), None).unwrap();
        assert_eq!(reloaded.snapshot().llm.default, "kimi");
        assert!(!reloaded.snapshot().plugins.translator.enabled);
        // 未修改字段保持默认
        assert_eq!(reloaded.snapshot().asr.user_engine, "paraformer-zh");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_returns_independent_clone() {
        let dir = std::env::temp_dir().join(format!("talksage-cfg-snap-{}", std::process::id()));
        let mgr = ConfigManager::from_config(Config::default(), dir.clone());
        let mut snap = mgr.snapshot();
        snap.server.port = 12345;
        assert_eq!(mgr.snapshot().server.port, 8080);
        std::fs::remove_dir_all(dir).ok();
    }
}
