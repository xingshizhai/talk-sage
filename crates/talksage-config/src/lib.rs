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
///
/// 数据目录与配置目录分离（v0.2+）：数据（sessions.db / 录音 / 导出 / 声纹 /
/// 窗口状态 / tmp）放这里；`talksage.toml` 见 [`default_config_file`]。
pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = env::var("TALKSAGE_DATA_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs_home().join(".talksage")
}

/// 配置文件（`talksage.toml`）路径：`TALKSAGE_CONFIG_DIR` 优先；
/// 未设时与数据目录相同（`<data_dir>/talksage.toml`，兼容旧版单目录布局）。
pub fn default_config_file(data_dir: &Path) -> PathBuf {
    if let Ok(d) = env::var("TALKSAGE_CONFIG_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d).join("talksage.toml");
        }
    }
    data_dir.join("talksage.toml")
}

/// 会话目录：`<data_dir>/sessions/<id>/`。
///
/// 一个会话的所有文件（录音分轨 / master 主录音 / 导出 md/txt）都放在这里，
/// 便于按会话归档与清理。返回前**不创建**目录（调用方按需创建）。
pub fn session_dir(data_dir: &Path, session_id: i64) -> PathBuf {
    data_dir.join("sessions").join(format!("{session_id}"))
}

/// 会话录音目录：`<data_dir>/sessions/<id>/recordings/`。
pub fn session_recordings_dir(data_dir: &Path, session_id: i64) -> PathBuf {
    session_dir(data_dir, session_id).join("recordings")
}

/// 会话导出目录：`<data_dir>/sessions/<id>/exports/`。
pub fn session_exports_dir(data_dir: &Path, session_id: i64) -> PathBuf {
    session_dir(data_dir, session_id).join("exports")
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

/// 场景模式：不同场景使用不同的参数组合（VAD 灵敏度/降噪/最短提交/引擎/插件/说话人）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneMode {
    /// 单人听写：低资源、灵敏 VAD、单流。
    Dictation,
    /// 一对一会话：按输入通道区分双方，两流使用相同语言。
    Conversation,
    /// 双语对话：双方通道使用不同语言，可选双向翻译。
    #[serde(alias = "translation")]   // 兼容旧配置文件
    Bilingual,
    /// 实时翻译：单一语言输入，自动翻译到目标语言输出。
    LiveTranslation,
    /// 多人会议：启用在线声纹聚类和段内换人检测。
    Meeting,
    /// 演讲/课堂：长段单流，开启术语与简报，不运行声纹模型。
    Lecture,
    /// 自定义：使用 `SceneConfig.custom` 全部参数。
    Custom,
}

impl Default for SceneMode {
    fn default() -> Self {
        Self::Conversation
    }
}

/// 角色归属策略。Channel 只使用输入通道，Voiceprint 才加载 WeSpeaker。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerMode {
    Off,
    Channel,
    Voiceprint,
}

impl Default for SpeakerMode {
    fn default() -> Self { Self::Channel }
}

/// 实时翻译策略。一对一单向翻译固定为“对方语言 → 我的语言”。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationMode {
    Off,
    ClientToUser,
    Bidirectional,
}

impl Default for TranslationMode {
    fn default() -> Self { Self::Off }
}

/// 场景参数集（一个场景的完整有效参数）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneParams {
    /// VAD 灵敏度预设。
    pub vad_preset: VadPreset,
    /// VAD 覆盖：检测阈值。
    pub vad_threshold: Option<f32>,
    /// VAD 覆盖：最小语音（ms）。
    pub vad_min_speech_ms: Option<u64>,
    /// VAD 覆盖：段结束静音（ms）。
    pub vad_min_silence_ms: Option<u64>,
    /// VAD 覆盖：最长语音（ms）。
    pub vad_max_speech_ms: Option<u64>,
    /// 降噪开关（噪声门 + 高通）。
    pub denoise_enabled: bool,
    /// 降噪噪声门阈值（RMS）。
    pub denoise_gate: f32,
    /// 最短提交时长（ms；0 = 不限制，噪音短段抑制）。
    pub min_segment_ms: u64,
    /// 段级 ASR 最长上下文（ms；0 = 不主动切分）。
    /// 该值只影响 Whisper/Qwen 等整段推理引擎，流式引擎忽略。
    pub asr_segment_ms: u64,
    /// 用户流引擎（默认 Qwen3-ASR；仍可显式选择旧流式或 Whisper）。
    pub user_engine: String,
    /// 是否启用客户流（双流；系统回环 + 英文引擎）。
    pub client_enabled: bool,
    /// 客户流引擎。
    pub client_engine: String,
    /// 本场景主语言（所有单语言场景两流均使用此语言）。
    /// 双语场景中为「我的语言」；实时翻译场景中为「输入语言」。
    #[serde(alias = "user_language")]
    pub language: String,
    /// 对方语言：双语场景为「对方讲的语言」；实时翻译场景为「翻译目标语言」。
    pub client_language: String,
    pub translation_mode: TranslationMode,
    /// 该场景允许启用的分析类插件 id。不在列表里的一律关闭。
    ///
    /// 用 allowlist 而非 denylist —— 新增插件不会因为某个场景忘了更新而意外开启。
    /// 只约束**分析类**插件（术语/翻译/简报这类「会议辅助功能」）；短段抑制、
    /// 跨流去重、质量评估是基础设施，不受此列表影响（见
    /// `talksage_plugins` 的 analysis descriptor）。
    pub plugin_allowlist: Vec<String>,
    /// 多人说话人区分（wespeaker 在线聚类）。主人声纹是可选增强：存在时把
    /// 匹配身份标为“我”，不存在时仍可区分“讲话者/客户 1/客户 2”。
    pub speaker_mode: SpeakerMode,
    /// 质量评估自动检测背景噪音（auto_detect）。
    pub noise_auto_detect: bool,
}

impl Default for SceneParams {
    fn default() -> Self {
        scene_params(SceneMode::Conversation)
    }
}

/// 分析类插件全开的 allowlist（双语 / 会议 / 自定义共用）。
///
/// 这里的 id 必须与 `talksage_plugins` 的 analysis descriptor 对齐 —— 配置层
/// 刻意不依赖插件层（依赖方向是「pipeline 实现、plugins 定义」），所以两处
/// 各存一份；一致性由 talksage-pipeline 的 `scene_allowlist` 测试锁住。
fn all_analysis_plugins() -> Vec<String> {
    ["term_explainer", "translator", "brief_retriever", "key_point_llm"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 内置场景参数模板。
pub fn scene_params(mode: SceneMode) -> SceneParams {
    match mode {
        SceneMode::Dictation => SceneParams {
            vad_preset: VadPreset::Sensitive,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: Some(600),
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 0,
            asr_segment_ms: 5_000,
            user_engine: "qwen3-asr".into(),
            client_enabled: false,
            client_engine: "qwen3-asr".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: ["key_point_llm"].iter().map(|s| s.to_string()).collect(),
            speaker_mode: SpeakerMode::Off,
            noise_auto_detect: true,
        },
        SceneMode::Conversation => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            asr_segment_ms: 4_000,
            user_engine: "qwen3-asr".into(),
            client_enabled: true,
            client_engine: "qwen3-asr".into(),
            language: "zh".into(),
            client_language: "zh".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: ["term_explainer", "brief_retriever", "key_point_llm"].iter().map(|s| s.to_string()).collect(),
            speaker_mode: SpeakerMode::Channel,
            noise_auto_detect: true,
        },
        SceneMode::Bilingual => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            asr_segment_ms: 4_000,
            user_engine: "qwen3-asr".into(),
            client_enabled: true,
            client_engine: "qwen3-asr".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Bidirectional,
            plugin_allowlist: all_analysis_plugins(),
            speaker_mode: SpeakerMode::Channel,
            noise_auto_detect: true,
        },
        SceneMode::LiveTranslation => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            asr_segment_ms: 3_000,
            user_engine: "qwen3-asr".into(),
            client_enabled: false,
            client_engine: "qwen3-asr".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Bidirectional,
            plugin_allowlist: ["translator"].iter().map(|s| s.to_string()).collect(),
            speaker_mode: SpeakerMode::Off,
            noise_auto_detect: true,
        },
        SceneMode::Meeting => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 0,
            asr_segment_ms: 6_000,
            user_engine: "qwen3-asr".into(),
            client_enabled: true,
            client_engine: "qwen3-asr".into(),
            language: "zh".into(),
            client_language: "zh".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: all_analysis_plugins(),
            speaker_mode: SpeakerMode::Voiceprint,
            noise_auto_detect: true,
        },
        SceneMode::Lecture => SceneParams {
            vad_preset: VadPreset::Strict,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: Some(700),
            vad_max_speech_ms: Some(60_000),
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300,
            asr_segment_ms: 6_000,
            user_engine: "qwen3-asr".into(),
            client_enabled: false,
            client_engine: "qwen3-asr".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: ["term_explainer", "brief_retriever", "key_point_llm"].iter().map(|s| s.to_string()).collect(),
            speaker_mode: SpeakerMode::Off,
            noise_auto_detect: true,
        },
        SceneMode::Custom => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 0,
            asr_segment_ms: 4_000,
            user_engine: "qwen3-asr".into(),
            client_enabled: true,
            client_engine: "qwen3-asr".into(),
            language: "zh".into(),
            client_language: "en".into(),
            translation_mode: TranslationMode::Off,
            plugin_allowlist: all_analysis_plugins(),
            speaker_mode: SpeakerMode::Channel,
            noise_auto_detect: true,
        },
    }
}

/// 场景配置：模式 + 自定义参数（非自定义模式忽略 custom）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SceneConfig {
    /// 当前场景模式。
    pub mode: SceneMode,
    /// 自定义模式的完整参数（其他模式忽略；初始为会议模板副本）。
    pub custom: SceneParams,
}

impl Default for SceneConfig {
    fn default() -> Self {
        Self {
            mode: SceneMode::Conversation,
            custom: scene_params(SceneMode::Custom),
        }
    }
}

impl SceneConfig {
    /// 当前生效参数：自定义模式用 custom，否则用内置模板。
    pub fn effective(&self) -> SceneParams {
        match self.mode {
            SceneMode::Custom => self.custom.clone(),
            m => scene_params(m),
        }
    }

    /// 当前模式的名称（中文，前端展示）。
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            SceneMode::Dictation => "单人听写",
            SceneMode::Conversation => "一对一会话",
            SceneMode::Bilingual => "双语对话",
            SceneMode::LiveTranslation => "实时翻译",
            SceneMode::Meeting => "多人会议",
            SceneMode::Lecture => "演讲/课堂",
            SceneMode::Custom => "自定义",
        }
    }
}

impl SceneParams {
    /// 转成 VadConfig（场景的 VAD 参数）。
    pub fn to_vad_config(&self) -> VadConfig {
        VadConfig {
            preset: self.vad_preset,
            threshold: self.vad_threshold,
            min_speech_ms: self.vad_min_speech_ms,
            min_silence_ms: self.vad_min_silence_ms,
            max_speech_ms: self.vad_max_speech_ms,
        }
    }

    /// 转成 DenoiseConfig。
    pub fn to_denoise_config(&self) -> DenoiseConfig {
        DenoiseConfig {
            enabled: self.denoise_enabled,
            gate_threshold: self.denoise_gate,
            highpass: true,
            highpass_cutoff_hz: 100.0,
        }
    }
}

/// 把前端提交的场景自定义参数（JSON 对象）应用到 SceneParams（逐字段，未提交字段保留）。
pub fn apply_scene_params(p: &mut SceneParams, u: &serde_json::Value) {
    if let Some(v) = u.get("vad_preset").and_then(|v| v.as_str()) {
        p.vad_preset = match v {
            "sensitive" => VadPreset::Sensitive,
            "strict" => VadPreset::Strict,
            _ => VadPreset::Standard,
        };
    }
    if let Some(v) = u.get("vad_threshold") {
        p.vad_threshold = v.as_f64().map(|f| f as f32);
    }
    if let Some(v) = u.get("vad_min_speech_ms") {
        p.vad_min_speech_ms = v.as_u64();
    }
    if let Some(v) = u.get("vad_min_silence_ms") {
        p.vad_min_silence_ms = v.as_u64();
    }
    if let Some(v) = u.get("vad_max_speech_ms") {
        p.vad_max_speech_ms = v.as_u64();
    }
    if let Some(v) = u.get("denoise_enabled").and_then(|v| v.as_bool()) {
        p.denoise_enabled = v;
    }
    if let Some(v) = u.get("denoise_gate").and_then(|v| v.as_f64()) {
        p.denoise_gate = v as f32;
    }
    if let Some(v) = u.get("min_segment_ms") {
        p.min_segment_ms = v.as_u64().unwrap_or(0);
    }
    if let Some(v) = u.get("asr_segment_ms") {
        p.asr_segment_ms = v.as_u64().unwrap_or(0).min(60_000);
    }
    if let Some(v) = u.get("user_engine").and_then(|v| v.as_str()) {
        p.user_engine = v.to_string();
    }
    if let Some(v) = u.get("client_enabled").and_then(|v| v.as_bool()) {
        p.client_enabled = v;
    }
    if let Some(v) = u.get("client_engine").and_then(|v| v.as_str()) {
        p.client_engine = v.to_string();
    }
    // allowlist 整体替换而非逐项增删：前端提交的就是「这个场景允许哪些插件」的
    // 完整答案，半个列表没有意义。非字符串项直接丢弃。
    if let Some(v) = u.get("plugin_allowlist").and_then(|v| v.as_array()) {
        p.plugin_allowlist = v
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(v) = u.get("language").or_else(|| u.get("user_language")).and_then(|v| v.as_str()) {
        p.language = v.to_string();
    }
    if let Some(v) = u.get("client_language").and_then(|v| v.as_str()) {
        p.client_language = v.to_string();
    }
    if let Some(v) = u.get("translation_mode").and_then(|v| v.as_str()) {
        p.translation_mode = match v {
            "client_to_user" => TranslationMode::ClientToUser,
            "bidirectional" => TranslationMode::Bidirectional,
            _ => TranslationMode::Off,
        };
    }
    if let Some(v) = u.get("speaker_mode").and_then(|v| v.as_str()) {
        p.speaker_mode = match v {
            "off" => SpeakerMode::Off,
            "voiceprint" => SpeakerMode::Voiceprint,
            _ => SpeakerMode::Channel,
        };
    }
    if let Some(v) = u.get("noise_auto_detect").and_then(|v| v.as_bool()) {
        p.noise_auto_detect = v;
    }
}

/// 顶层配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub asr: AsrConfig,
    pub audio: AudioConfig,
    pub llm: LlmConfig,
    pub plugins: PluginsConfig,
    pub recording: RecordingConfig,
    pub quality: QualityConfig,
    pub privacy: PrivacyConfig,
    pub server: ServerConfig,
    pub knowledge_base: KnowledgeBaseConfig,
    pub webhooks: WebhooksConfig,
    pub scene: SceneConfig,
    pub network: NetworkConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            asr: AsrConfig::default(),
            audio: AudioConfig::default(),
            llm: LlmConfig::default(),
            plugins: PluginsConfig::default(),
            recording: RecordingConfig::default(),
            quality: QualityConfig::default(),
            privacy: PrivacyConfig::default(),
            server: ServerConfig::default(),
            knowledge_base: KnowledgeBaseConfig::default(),
            webhooks: WebhooksConfig::default(),
            scene: SceneConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

/// 网络代理配置。
/// 代理仅对外网请求生效（模型下载、LLM API、Webhook）；
/// 阿里云 ASR 等国内服务始终直连，不受此配置影响。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    /// HTTP/HTTPS 代理地址（如 `http://127.0.0.1:7890`）。
    /// 留空或不填表示直连。
    pub proxy: String,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self { proxy: String::new() }
    }
}

impl NetworkConfig {
    /// 返回有效的代理 URL；空字符串时返回 `None`。
    pub fn proxy_url(&self) -> Option<&str> {
        let s = self.proxy.trim();
        if s.is_empty() { None } else { Some(s) }
    }
}

/// 会议结束 Webhook（借鉴 Call.md workflow-webhook）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhooksConfig {
    /// 总开关。
    pub enabled: bool,
    /// 目标 URL 列表（http/https；调用前做 SSRF 防护，拒绝内网/回环地址）。
    pub urls: Vec<String>,
}

impl Default for WebhooksConfig {
    fn default() -> Self {
        Self { enabled: false, urls: Vec::new() }
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
    /// 中文场景使用的引擎（两流均用此引擎）。
    #[serde(alias = "user_engine")]
    pub engine_zh: String,
    /// 英文场景使用的引擎（两流均用此引擎）。
    #[serde(alias = "client_engine")]
    pub engine_en: String,
    /// 本地推理后端：auto | cpu | cuda | metal。Apple GPU 由独立
    /// whisper.cpp/Metal 引擎管理，不使用 sherpa-onnx CoreML provider。
    pub backend: String,
    /// 专业术语热词和确定性纠错配置。
    pub terminology: TerminologyConfig,
    /// 是否启用标点恢复与语义分段（流式引擎且模型已安装时生效）。
    pub punct_enabled: bool,
    /// 阿里云智能语音 AccessKey ID。
    #[serde(default)]
    pub aliyun_access_key_id: String,
    /// 阿里云智能语音 AccessKey Secret。
    #[serde(default)]
    pub aliyun_access_key_secret: String,
    /// 阿里云 NLS 项目 AppKey。
    #[serde(default)]
    pub aliyun_app_key: String,
    /// ASR 模式："auto" | "local" | "cloud"。
    #[serde(default = "default_asr_mode")]
    pub asr_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminologyConfig {
    pub enabled: bool,
    /// 上下文偏置强度；仅支持热词的模型使用。
    pub hotword_score: f32,
    /// 每项一个产品名、人名、缩写或行业术语。
    pub terms: Vec<String>,
    /// 常见误识别 → 正确术语。匹配在 ASR 输出后同步完成，不增加推理延迟。
    pub corrections: std::collections::BTreeMap<String, String>,
}

impl Default for TerminologyConfig {
    fn default() -> Self {
        Self { enabled: false, hotword_score: 1.5, terms: Vec::new(), corrections: Default::default() }
    }
}

impl TerminologyConfig {
    pub fn normalized_terms(&self) -> Vec<String> {
        if !self.enabled { return Vec::new(); }
        let mut terms: Vec<String> = self.terms.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        terms.sort();
        terms.dedup();
        terms.truncate(256);
        terms
    }

    pub fn correct(&self, text: &str) -> String {
        if !self.enabled { return text.to_string(); }
        let mut entries: Vec<_> = self.corrections.iter()
            .filter(|(wrong, right)| !wrong.is_empty() && !right.is_empty())
            .take(256)
            .collect();
        // 长别名优先，避免“向量”先替换导致“向量数据库”规则失效。
        entries.sort_by_key(|(wrong, _)| std::cmp::Reverse(wrong.chars().count()));
        entries.into_iter().fold(text.to_string(), |out, (wrong, right)| out.replace(wrong, right))
    }
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            engine_zh: "qwen3-asr".into(),
            engine_en: "qwen3-asr".into(),
            backend: "auto".into(),
            terminology: TerminologyConfig::default(),
            punct_enabled: true,
            aliyun_access_key_id: String::new(),
            aliyun_access_key_secret: String::new(),
            aliyun_app_key: String::new(),
            asr_mode: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub mic_device: Option<i32>,
    pub loopback_device: Option<i32>,
    /// 采集源："mic"（麦克风，默认）| "loopback"（系统音频，用于视频会议识别对方）。
    #[serde(default = "AudioConfig::default_audio_source")]
    pub audio_source: String,
    /// 麦克风输入增益（dB，0..24）；声道选择后、录音和 ASR 前应用。
    pub input_gain_db: f32,
    pub ducking: DuckingConfig,
    pub vad: VadConfig,
    pub denoise: DenoiseConfig,
    /// 流式 ASR 文本稳定性端点：结合短暂停顿自然提交。
    pub endpoint: EndpointConfig,
    /// 最短提交时长（ms）：final 段时长低于该值的丢弃（噪音短段抑制，
    /// 减少"无效短段"污染转写/历史）；None/0 = 不限制。
    pub min_segment_ms: Option<u64>,
}

impl AudioConfig {
    fn default_audio_source() -> String { "mic".into() }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mic_device: None,
            loopback_device: None,
            audio_source: Self::default_audio_source(),
            input_gain_db: 12.0,
            ducking: DuckingConfig::default(),
            vad: VadConfig::default(),
            denoise: DenoiseConfig::default(),
            endpoint: EndpointConfig::default(),
            min_segment_ms: None,
        }
    }
}

/// 流式识别的低开销混合端点配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EndpointConfig {
    pub enabled: bool,
    pub stable_ms: u64,
    pub quiet_ms: u64,
    /// 即使文本仍变化，连续强停顿达到该时长也提交。
    pub force_quiet_ms: u64,
    pub quiet_rms: f32,
    pub min_segment_ms: u64,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self { enabled: true, stable_ms: 350, quiet_ms: 450, force_quiet_ms: 850, quiet_rms: 0.008, min_segment_ms: 1000 }
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
            VadPreset::Standard => (0.50, 0.25, 0.50, 512, 30.0),
            VadPreset::Sensitive => (0.35, 0.15, 0.30, 512, 30.0),
            VadPreset::Strict => (0.65, 0.35, 0.80, 512, 30.0),
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
            // 默认关闭降噪：噪声门会把远端/轻声的弱信号块整体静音，导致 VAD 判"无声"。
            // 保持默认关闭以保留弱语音识别能力；嘈杂环境可在设置页手动开启（gate 0.008 较温和）。
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

/// 插件配置表。
///
/// **通用表，不认识具体插件。** 键是插件 id，值的结构由插件自己的
/// `default_config()` 定义；这里只负责原样存取。缺省是空表 —— 每个插件的
/// 默认值归插件自己，配置文件里没写就是「用默认」。
///
/// 破坏性变更（设计 §4）：阶段 5 之前这里是 `term_explainer` /`translator` /
/// `brief_retriever` 三个具名字段。旧配置文件里的这三段会被原样读进
/// `entries`，键名相同，所以 `enabled` / `cooldown_seconds` 仍然生效；
/// 不做读时迁移。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// 通用插件表：键是插件 id，值由插件自己的 default_config() 定义结构。
    #[serde(flatten)]
    pub entries: std::collections::BTreeMap<String, serde_json::Value>,
    /// 纪要模板 —— 不是插件配置，是宿主自己的设置，保持具名。
    pub notes: NotesConfig,
}

impl PluginsConfig {
    /// 读某个插件的某个布尔键（表里没有该插件 / 该键时返回 `default`）。
    /// 只给宿主展示用（doctor / API），真正的合并在 `build_registry` 里。
    pub fn get_bool(&self, id: &str, key: &str, default: bool) -> bool {
        self.entries
            .get(id)
            .and_then(|v| v.get(key))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default)
    }

    /// 应用前端提交的 `plugins` 表（设置面板 / API 保存）。
    ///
    /// 逐插件逐键合并，不认识具体插件 —— 前端提交什么 id 就存什么 id，
    /// 校验归插件自己（读配置时 `get_*` 都带默认值）。`notes` 不是插件，
    /// 单独走具名字段。
    pub fn apply_updates(&mut self, updates: &serde_json::Value) {
        let Some(obj) = updates.as_object() else {
            return;
        };
        for (id, patch) in obj {
            if id == "notes" {
                if let Some(t) = patch.get("template").and_then(|v| v.as_str()) {
                    self.notes.template = t.to_string();
                }
                continue;
            }
            self.merge_entry(id, patch);
        }
    }

    /// 把 `patch` 里的键并进 `[plugins.<id>]`（未提交的键保留）。
    pub fn merge_entry(&mut self, id: &str, patch: &serde_json::Value) {
        let Some(patch) = patch.as_object() else {
            return;
        };
        let entry = self
            .entries
            .entry(id.to_string())
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        if !entry.is_object() {
            *entry = serde_json::Value::Object(Default::default());
        }
        let Some(dst) = entry.as_object_mut() else {
            return;
        };
        for (k, v) in patch {
            dst.insert(k.clone(), v.clone());
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
///
/// 目录分离：`data_dir`（数据：会话库/录音/导出/声纹）与 `config_file`
/// （配置：`talksage.toml`）彼此独立，见 [`default_config_file`]。
pub struct ConfigManager {
    data_dir: PathBuf,
    config_file: PathBuf,
    config: std::sync::RwLock<Config>,
}

impl ConfigManager {
    /// 从默认配置 + 用户文件加载。
    ///
    /// `file` 为 None 时使用 [`default_config_file`]（`TALKSAGE_CONFIG_DIR`
    /// 优先，否则 `<data_dir>/talksage.toml`；不存在则仅默认值）。
    pub fn load(data_dir: Option<PathBuf>, file: Option<&Path>) -> Result<Self, ConfigError> {
        let data_dir = data_dir.unwrap_or_else(default_data_dir);
        let mut config = Config::default();
        let config_file = file
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| default_config_file(&data_dir));
        let path = &config_file;
        if path.exists() {
            let raw = std::fs::read_to_string(path)?;
            let user: Config = toml::from_str(&raw)?;
            config = merge_config(config, user);
            // 双通道：log 进日志文件（server/cli 在 load 前已初始化日志）；
            // eprintln 兜底（tauri 先 load 配置才知道日志目录，此前的 log 会丢）。
            log::info!("配置文件: {}", path.display());
            eprintln!("[talksage] 配置文件: {}", path.display());
        } else {
            log::warn!(
                "未找到配置文件: {}；使用内置默认值运行（LLM 功能不可用）。\
                 提示: 复制 config/talksage.example.toml 到该路径并填写 API Key 等配置，\
                 或设置环境变量 TALKSAGE_CONFIG_DIR / TALKSAGE_DATA_DIR 指向配置目录。",
                path.display()
            );
            eprintln!(
                "[talksage] 未找到配置文件: {}\n\
                 提示: 复制 config/talksage.example.toml 到该路径并填写 API Key 等配置，\
                 或设置环境变量 TALKSAGE_CONFIG_DIR / TALKSAGE_DATA_DIR 指向配置目录。\
                 当前使用内置默认值运行（LLM 功能不可用）。",
                path.display()
            );
        }
        apply_env_overrides(&mut config);
        Ok(Self {
            data_dir,
            config_file,
            config: std::sync::RwLock::new(config),
        })
    }

    /// 直接以自定义目录构建（测试用；配置与数据同目录，自包含，不读环境变量）。
    pub fn from_config(config: Config, data_dir: PathBuf) -> Self {
        let config_file = data_dir.join("talksage.toml");
        Self {
            data_dir,
            config_file,
            config: std::sync::RwLock::new(config),
        }
    }

    /// 完整配置快照（克隆，线程安全）。
    pub fn snapshot(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    /// 数据目录（会话库、录音、导出、声纹、窗口状态所在）。
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 配置文件路径（`talksage.toml`；可能与数据目录不同）。
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// 配置文件所在目录。
    pub fn config_dir(&self) -> &Path {
        self.config_file.parent().unwrap_or(&self.data_dir)
    }

    /// 运行时更新配置（回调内修改），并立即持久化到配置文件（`talksage.toml`）。
    pub fn update<R>(&self, f: impl FnOnce(&mut Config) -> R) -> Result<R, ConfigError> {
        let mut config = self.config.write().unwrap();
        let result = f(&mut config);
        let raw = toml::to_string(&*config)?;
        if let Some(parent) = self.config_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.config_file, raw)?;
        Ok(result)
    }
}

/// 用户配置字段覆盖默认配置（非 None/非默认值时生效）。
fn merge_config(default: Config, user: Config) -> Config {
    Config {
        asr: AsrConfig {
            engine_zh: take_or(user.asr.engine_zh, default.asr.engine_zh),
            engine_en: take_or(user.asr.engine_en, default.asr.engine_en),
            backend: take_or(user.asr.backend, default.asr.backend),
            terminology: user.asr.terminology,
            punct_enabled: user.asr.punct_enabled,
            aliyun_access_key_id: user.asr.aliyun_access_key_id,
            aliyun_access_key_secret: user.asr.aliyun_access_key_secret,
            aliyun_app_key: user.asr.aliyun_app_key,
            asr_mode: user.asr.asr_mode,
        },
        audio: AudioConfig {
            mic_device: user.audio.mic_device.or(default.audio.mic_device),
            loopback_device: user.audio.loopback_device.or(default.audio.loopback_device),
            audio_source: take_or(user.audio.audio_source, default.audio.audio_source),
            input_gain_db: user.audio.input_gain_db.clamp(0.0, 24.0),
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
            endpoint: user.audio.endpoint,
            min_segment_ms: user.audio.min_segment_ms.or(default.audio.min_segment_ms),
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
        // 通用表：用户写了什么就是什么，宿主不认识键也不该替插件填默认值
        // —— 默认值归 plugin.default_config()，合并在 build_registry 里发生。
        plugins: user.plugins,
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
        webhooks: WebhooksConfig {
            enabled: user.webhooks.enabled,
            urls: user.webhooks.urls,
        },
        network: user.network,
        scene: SceneConfig {
            mode: user.scene.mode,
            // 自定义参数跟随用户文件（未写时用默认模板）
            custom: SceneParams {
                vad_preset: user.scene.custom.vad_preset,
                vad_threshold: user.scene.custom.vad_threshold.or(default.scene.custom.vad_threshold),
                vad_min_speech_ms: user.scene.custom.vad_min_speech_ms.or(default.scene.custom.vad_min_speech_ms),
                vad_min_silence_ms: user.scene.custom.vad_min_silence_ms.or(default.scene.custom.vad_min_silence_ms),
                vad_max_speech_ms: user.scene.custom.vad_max_speech_ms.or(default.scene.custom.vad_max_speech_ms),
                denoise_enabled: user.scene.custom.denoise_enabled,
                denoise_gate: user.scene.custom.denoise_gate,
                min_segment_ms: user.scene.custom.min_segment_ms,
                asr_segment_ms: user.scene.custom.asr_segment_ms.min(60_000),
                user_engine: user.scene.custom.user_engine,
                client_enabled: user.scene.custom.client_enabled,
                client_engine: user.scene.custom.client_engine,
                language: user.scene.custom.language,
                client_language: user.scene.custom.client_language,
                translation_mode: user.scene.custom.translation_mode,
                plugin_allowlist: user.scene.custom.plugin_allowlist,
                speaker_mode: user.scene.custom.speaker_mode,
                noise_auto_detect: user.scene.custom.noise_auto_detect,
            },
        },
    }
}

fn default_asr_mode() -> String { "auto".into() }

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
    // 敏感字段：模板建议用环境变量覆盖（不写入 talksage.toml）
    if let Ok(v) = env::var("TALKSAGE_LLM_API_KEY") {
        if !v.trim().is_empty() {
            if let Some(p) = cfg.llm.providers.get_mut(&cfg.llm.default) {
                p.api_key = v.trim().to_string();
            }
        }
    }
    if let Ok(v) = env::var("ALIYUN_ACCESS_ID") {
        if !v.trim().is_empty() {
            cfg.asr.aliyun_access_key_id = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("ALIYUN_ACCESS_SECRET") {
        if !v.trim().is_empty() {
            cfg.asr.aliyun_access_key_secret = v.trim().to_string();
        }
    }
    if let Ok(v) = env::var("ALIYUN_APP_ID") {
        if !v.trim().is_empty() {
            cfg.asr.aliyun_app_key = v.trim().to_string();
        }
    }
}

// ── 设置页配置面：读快照 / 写更新（桌面端与 headless 共用）──────────────
//
// 这两件事以前在 `talksage-server` 与 `web/src-tauri` 里各抄了一份。抄本会
// drift，而 drift 的代价用户看不见：headless 少返回一段配置 → 设置页把默认值
// 当成真值显示 → 保存时原样写回去 → 静默覆盖用户配置（scene / recording /
// quality / network 都这么丢过）。放在这里，两个宿主只能调同一份实现。

/// 密钥出口策略：本机 IPC 明文，跨网络打码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretPolicy {
    /// 明文返回。桌面端 IPC：同进程同用户，设置页本来就要把 key 显示在输入框里。
    Reveal,
    /// 打码返回。headless 的 `/api/config` 在未设 token 时匿名可读，
    /// 明文吐 key 等于把凭据挂在端口上。
    Mask,
}

/// 密钥掩码：`sk-••••••••cdef`；空值返回空串（前端据此区分「未配置」）。
///
/// 掩码必须稳定且可原样写回：[`apply_updates`] 把「与当前值掩码相同的提交」
/// 视作未修改，设置页因此不需要知道自己拿到的是掩码还是真值。
pub fn mask_secret(secret: &str) -> String {
    let s = secret.trim();
    if s.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    // 太短的密钥露头露尾就等于露全部。
    if chars.len() <= 8 {
        return "••••••••".to_string();
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}••••••••{tail}")
}

/// 提交值是否就是 `stored` 的掩码 —— 即用户没有修改这个字段。
pub fn is_secret_mask(submitted: &str, stored: &str) -> bool {
    !stored.trim().is_empty() && submitted == mask_secret(stored)
}

/// 密钥写入：掩码原样回传 = 保持原值；其余一律采用提交值（空串 = 主动清空）。
fn apply_secret(stored: &mut String, submitted: &str) {
    if !is_secret_mask(submitted, stored) {
        *stored = submitted.trim().to_string();
    }
}

/// 密钥读入（设置页「检查」按钮）：空或掩码 → 回落到已存的值。
pub fn resolve_secret_input(submitted: Option<&str>, stored: &str) -> String {
    match submitted {
        Some(v) if !v.trim().is_empty() && !is_secret_mask(v, stored) => v.trim().to_string(),
        _ => stored.to_string(),
    }
}

/// 设置页看到的配置快照：整份 `Config` + `plugins` 换成生效配置。
///
/// 整份序列化是刻意的 —— 手挑字段的版本每加一个配置段就会漏一个。`plugins`
/// 单独替换：通用表里只有用户显式写过的插件，默认值归插件所有，宿主在出口处
/// 替前端补齐；`notes` 不是插件配置，保持具名。
pub fn ui_config_json(
    config: &Config,
    plugins: serde_json::Map<String, serde_json::Value>,
    secrets: SecretPolicy,
) -> serde_json::Value {
    let mut value = serde_json::to_value(config).unwrap_or(serde_json::Value::Null);
    let Some(obj) = value.as_object_mut() else {
        return value;
    };
    let mut plugins = plugins;
    plugins.insert(
        "notes".into(),
        serde_json::json!({ "template": config.plugins.notes.template }),
    );
    obj.insert("plugins".into(), serde_json::Value::Object(plugins));
    if secrets == SecretPolicy::Mask {
        mask_config_secrets(obj);
    }
    value
}

/// 打码 LLM `api_key` / 阿里云 AccessKey Secret / server token。
///
/// AccessKey ID 与 AppKey 是标识不是凭据（单独签不出 token），保持明文 ——
/// 设置页要显示它们，「检查」按钮也要拿它们去验签。
fn mask_config_secrets(obj: &mut serde_json::Map<String, serde_json::Value>) {
    fn mask_in_place(field: Option<&mut serde_json::Value>) {
        if let Some(v) = field {
            *v = serde_json::Value::String(mask_secret(v.as_str().unwrap_or_default()));
        }
    }
    if let Some(providers) = obj
        .get_mut("llm")
        .and_then(|llm| llm.get_mut("providers"))
        .and_then(|p| p.as_object_mut())
    {
        for provider in providers.values_mut() {
            mask_in_place(provider.get_mut("api_key"));
        }
    }
    mask_in_place(obj.get_mut("asr").and_then(|a| a.get_mut("aliyun_access_key_secret")));
    mask_in_place(obj.get_mut("server").and_then(|s| s.get_mut("token")));
}

/// 把设置页提交的更新应用到配置：逐字段合并，未提交的键保持不变。
///
/// 提交的形状见 `web/src/sections/SettingsSection.tsx` 的 `buildSnapshot()`
/// —— 那里发什么，这里就得收什么，少收一段就是一次静默覆盖。
pub fn apply_updates(c: &mut Config, updates: &serde_json::Value) {
    if let Some(llm) = updates.get("llm") {
        if let Some(default) = llm.get("default").and_then(|v| v.as_str()) {
            c.llm.default = default.to_string();
        }
        if let Some(providers) = llm.get("providers").and_then(|v| v.as_object()) {
            for (name, p) in providers {
                let entry = c.llm.providers.entry(name.clone()).or_default();
                if let Some(k) = p.get("api_key").and_then(|v| v.as_str()) {
                    apply_secret(&mut entry.api_key, k);
                }
                if let Some(m) = p.get("model").and_then(|v| v.as_str()) {
                    entry.model = m.to_string();
                }
                if let Some(b) = p.get("base_url").and_then(|v| v.as_str()) {
                    entry.base_url = Some(b.to_string());
                }
            }
        }
    }
    if let Some(plugins) = updates.get("plugins") {
        // 通用表：逐插件逐键合并，宿主不认识具体插件的配置结构。
        c.plugins.apply_updates(plugins);
    }
    if let Some(kb) = updates.get("knowledge_base") {
        if let Some(e) = kb.get("enabled").and_then(|v| v.as_bool()) {
            c.knowledge_base.enabled = e;
        }
        if let Some(f) = kb.get("folder").and_then(|v| v.as_str()) {
            c.knowledge_base.folder = f.to_string();
        }
    }
    if let Some(asr) = updates.get("asr") {
        if let Some(e) = asr.get("engine_en").or_else(|| asr.get("client_engine")).and_then(|v| v.as_str()) {
            c.asr.engine_en = e.to_string();
        }
        if let Some(e) = asr.get("engine_zh").or_else(|| asr.get("user_engine")).and_then(|v| v.as_str()) {
            c.asr.engine_zh = e.to_string();
        }
        if let Some(b) = asr.get("backend").and_then(|v| v.as_str()) {
            c.asr.backend = b.to_string();
        }
        if let Some(v) = asr.get("punct_enabled").and_then(|v| v.as_bool()) {
            c.asr.punct_enabled = v;
        }
        if let Some(v) = asr.get("asr_mode").and_then(|v| v.as_str()) {
            c.asr.asr_mode = v.to_string();
        }
        if let Some(v) = asr.get("aliyun_access_key_id").and_then(|v| v.as_str()) {
            c.asr.aliyun_access_key_id = v.trim().to_string();
        }
        if let Some(v) = asr.get("aliyun_access_key_secret").and_then(|v| v.as_str()) {
            apply_secret(&mut c.asr.aliyun_access_key_secret, v);
        }
        if let Some(v) = asr.get("aliyun_app_key").and_then(|v| v.as_str()) {
            c.asr.aliyun_app_key = v.trim().to_string();
        }
        if let Some(t) = asr.get("terminology") {
            if let Some(v) = t.get("enabled").and_then(|v| v.as_bool()) { c.asr.terminology.enabled = v; }
            if let Some(v) = t.get("hotword_score").and_then(|v| v.as_f64()) { c.asr.terminology.hotword_score = (v as f32).clamp(0.0, 10.0); }
            if let Some(v) = t.get("terms").and_then(|v| v.as_array()) {
                c.asr.terminology.terms = v.iter().filter_map(|x| x.as_str()).map(str::to_string).collect();
            }
            if let Some(v) = t.get("corrections").and_then(|v| v.as_object()) {
                c.asr.terminology.corrections = v.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect();
            }
        }
    }
    if let Some(audio) = updates.get("audio") {
        // 采集来源：麦克风 / 系统音频回环。SideNav 的就地切换也走这里
        // （App.tsx 直接 saveConfig({audio:{audio_source}})），两条路同一个键。
        if let Some(v) = audio.get("audio_source").and_then(|v| v.as_str()) {
            c.audio.audio_source = if v == "loopback" { "loopback".to_string() } else { "mic".to_string() };
        }
        if let Some(v) = audio.get("input_gain_db").and_then(|v| v.as_f64()) {
            c.audio.input_gain_db = (v as f32).clamp(0.0, 24.0);
        }
        if let Some(vad) = audio.get("vad") {
            if let Some(p) = vad.get("preset").and_then(|v| v.as_str()) {
                c.audio.vad.preset = match p {
                    "sensitive" => VadPreset::Sensitive,
                    "strict" => VadPreset::Strict,
                    _ => VadPreset::Standard,
                };
            }
            if let Some(t) = vad.get("threshold").and_then(|v| v.as_f64()) {
                c.audio.vad.threshold = Some(t as f32);
            }
        }
        if let Some(d) = audio.get("denoise") {
            if let Some(e) = d.get("enabled").and_then(|v| v.as_bool()) {
                c.audio.denoise.enabled = e;
            }
            if let Some(g) = d.get("gate_threshold").and_then(|v| v.as_f64()) {
                c.audio.denoise.gate_threshold = g as f32;
            }
            if let Some(h) = d.get("highpass").and_then(|v| v.as_bool()) {
                c.audio.denoise.highpass = h;
            }
        }
        if let Some(e) = audio.get("endpoint") {
            if let Some(v) = e.get("enabled").and_then(|v| v.as_bool()) { c.audio.endpoint.enabled = v; }
            if let Some(v) = e.get("stable_ms").and_then(|v| v.as_u64()) { c.audio.endpoint.stable_ms = v.max(100); }
            if let Some(v) = e.get("quiet_ms").and_then(|v| v.as_u64()) { c.audio.endpoint.quiet_ms = v.max(100); }
            if let Some(v) = e.get("force_quiet_ms").and_then(|v| v.as_u64()) { c.audio.endpoint.force_quiet_ms = v.max(200); }
            if let Some(v) = e.get("quiet_rms").and_then(|v| v.as_f64()) { c.audio.endpoint.quiet_rms = (v as f32).clamp(0.0, 0.5); }
            if let Some(v) = e.get("min_segment_ms").and_then(|v| v.as_u64()) { c.audio.endpoint.min_segment_ms = v; }
        }
        // 最短提交时长（ms）：0/null = 不限制
        if let Some(m) = audio.get("min_segment_ms") {
            if let Some(v) = m.as_u64() {
                c.audio.min_segment_ms = if v == 0 { None } else { Some(v) };
            } else if m.is_null() {
                c.audio.min_segment_ms = None;
            }
        }
    }
    if let Some(rec) = updates.get("recording") {
        if let Some(e) = rec.get("enabled").and_then(|v| v.as_bool()) {
            c.recording.enabled = e;
        }
        if let Some(d) = rec.get("dir").and_then(|v| v.as_str()) {
            c.recording.dir = d.to_string();
        }
        if let Some(cs) = rec.get("clean_silence").and_then(|v| v.as_bool()) {
            c.recording.clean_silence = cs;
        }
    }
    // quality：null → 恢复默认；否则按字段更新
    match updates.get("quality") {
        Some(serde_json::Value::Null) => {
            c.quality = QualityConfig::default();
        }
        Some(q) => {
            if let Some(a) = q.get("auto_detect").and_then(|v| v.as_bool()) {
                c.quality.auto_detect = a;
            }
            if let Some(t) = q.get("text_noise_threshold").and_then(|v| v.as_f64()) {
                c.quality.text_noise_threshold = t as f32;
            }
            if let Some(v) = q.get("min_speech_ratio").and_then(|v| v.as_f64()) {
                c.quality.min_speech_ratio = v as f32;
            }
            if let Some(v) = q.get("max_speech_ratio").and_then(|v| v.as_f64()) {
                c.quality.max_speech_ratio = v as f32;
            }
            if let Some(v) = q.get("silence_rms").and_then(|v| v.as_f64()) {
                c.quality.silence_rms = v as f32;
            }
            if let Some(v) = q.get("high_rms").and_then(|v| v.as_f64()) {
                c.quality.high_rms = v as f32;
            }
        }
        None => {}
    }
    // 会议结束 Webhook（借鉴 Call.md workflow-webhook）
    if let Some(w) = updates.get("webhooks") {
        if let Some(e) = w.get("enabled").and_then(|v| v.as_bool()) {
            c.webhooks.enabled = e;
        }
        if let Some(urls) = w.get("urls").and_then(|v| v.as_array()) {
            c.webhooks.urls = urls
                .iter()
                .filter_map(|u| u.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    if let Some(net) = updates.get("network") {
        if let Some(p) = net.get("proxy").and_then(|v| v.as_str()) {
            c.network.proxy = p.trim().to_string();
        }
    }
    // 场景模式
    if let Some(scene) = updates.get("scene") {
        if let Some(m) = scene.get("mode").and_then(|v| v.as_str()) {
            c.scene.mode = match m {
                "dictation" => SceneMode::Dictation,
                "conversation" => SceneMode::Conversation,
                "translation" | "bilingual" => SceneMode::Bilingual,
                "live_translation" => SceneMode::LiveTranslation,
                "meeting" => SceneMode::Meeting,
                "lecture" => SceneMode::Lecture,
                "custom" => SceneMode::Custom,
                _ => SceneMode::Conversation,
            };
        }
        if let Some(cu) = scene.get("custom") {
            apply_scene_params(&mut c.scene.custom, cu);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 已下线的 `[session]` 段（sqlite / export_markdown / record_audio 三个开关
    /// 从来没有代码读，删掉了）不能让老配置文件加载失败 —— 用户机器上还留着它。
    #[test]
    fn removed_session_section_is_ignored_by_old_config_files() {
        let raw = r#"
[asr]
engine_zh = "qwen3-asr"

[session]
sqlite = true
export_markdown = true
record_audio = true
"#;
        let cfg: Config = toml::from_str(raw).expect("含已下线段的老配置仍应能加载");
        assert_eq!(cfg.asr.engine_zh, "qwen3-asr", "其余字段照常解析");
    }

    #[test]
    fn session_dirs_follow_per_session_layout() {
        let data = std::path::Path::new("/data");
        assert_eq!(
            session_dir(data, 7),
            std::path::PathBuf::from("/data/sessions/7")
        );
        assert_eq!(
            session_recordings_dir(data, 7),
            std::path::PathBuf::from("/data/sessions/7/recordings")
        );
        assert_eq!(
            session_exports_dir(data, 7),
            std::path::PathBuf::from("/data/sessions/7/exports")
        );
        // 不同会话隔离
        assert_ne!(session_dir(data, 7), session_dir(data, 8));
    }

    /// `ConfigManager::load` 末尾会调 `apply_env_overrides` 读进程环境变量，
    /// 而 `env_overrides_win` 会 `set_var` —— 环境变量是进程全局的，Rust 测试
    /// 默认并行，两者相撞时读方会拿到别人设的值（实测连跑 5 次失败 4 次，
    /// 现象是 user_file_overrides_defaults 读到 7070 而非文件里的 9090）。
    ///
    /// 所有触碰环境变量的测试在此串行。
    static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        // 只需互斥、不共享状态，前一个测试 panic 导致的毒化可以忽略。
        ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn defaults_load_without_file() {
        let _env = env_lock();
        let mgr = ConfigManager::load(None, None).unwrap();
        let c = mgr.snapshot();
        assert_eq!(c.asr.engine_zh, "qwen3-asr");
        assert_eq!(c.server.host, "127.0.0.1");
        assert!(!c.server.enabled);
        assert_eq!(c.audio.vad.effective(), (0.50, 0.25, 0.50, 512, 30.0));
    }

    #[test]
    fn user_file_overrides_defaults() {
        let _env = env_lock();
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

[audio]
min_segment_ms = 600
"#,
        )
        .unwrap();
        let mgr = ConfigManager::load(None, Some(&file)).unwrap();
        let c = mgr.snapshot();
        assert_eq!(c.asr.backend, "cuda");
        assert!(c.server.enabled);
        assert_eq!(c.server.port, 9090);
        // 最短提交时长（噪音短段抑制）
        assert_eq!(c.audio.min_segment_ms, Some(600));
        // 未覆盖字段保持默认
        assert_eq!(c.asr.engine_zh, "qwen3-asr");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_overrides_win() {
        let _env = env_lock();
        unsafe {
            std::env::set_var("TALKSAGE_SERVER_PORT", "7070");
        }
        let mgr = ConfigManager::load(None, None).unwrap();
        assert_eq!(mgr.snapshot().server.port, 7070);
        unsafe {
            std::env::remove_var("TALKSAGE_SERVER_PORT");
        }
    }

    /// 模板建议的敏感字段环境变量（TALKSAGE_LLM_API_KEY / ALIYUN_*）必须真正生效。
    #[test]
    fn env_overrides_sensitive_fields() {
        let _env = env_lock();
        unsafe {
            std::env::set_var("TALKSAGE_LLM_API_KEY", "sk-env-test");
            std::env::set_var("ALIYUN_ACCESS_ID", "akid-env");
            std::env::set_var("ALIYUN_ACCESS_SECRET", "aksec-env");
            std::env::set_var("ALIYUN_APP_ID", "appkey-env");
        }
        let mgr = ConfigManager::load(None, None).unwrap();
        let snap = mgr.snapshot();
        // 默认 provider 是 deepseek
        assert_eq!(snap.llm.providers["deepseek"].api_key, "sk-env-test");
        assert_eq!(snap.asr.aliyun_access_key_id, "akid-env");
        assert_eq!(snap.asr.aliyun_access_key_secret, "aksec-env");
        assert_eq!(snap.asr.aliyun_app_key, "appkey-env");
        unsafe {
            std::env::remove_var("TALKSAGE_LLM_API_KEY");
            std::env::remove_var("ALIYUN_ACCESS_ID");
            std::env::remove_var("ALIYUN_ACCESS_SECRET");
            std::env::remove_var("ALIYUN_APP_ID");
        }
    }

    #[test]
    fn update_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("talksage-cfg-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talksage.toml");
        let mgr = ConfigManager::load(Some(dir.clone()), Some(&file)).unwrap();
        mgr.update(|c| {
            c.llm.default = "kimi".into();
            c.plugins
                .merge_entry("translator", &serde_json::json!({ "enabled": false }));
        })
        .unwrap();

        // 重新加载同一目录，应读到更新后的值
        let reloaded = ConfigManager::load(Some(dir.clone()), Some(&file)).unwrap();
        assert_eq!(reloaded.snapshot().llm.default, "kimi");
        assert!(!reloaded.snapshot().plugins.get_bool("translator", "enabled", true));
        // 未修改字段保持默认
        assert_eq!(reloaded.snapshot().asr.engine_zh, "qwen3-asr");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_file_follows_config_dir_env() {
        let _env = env_lock();
        let cfg_dir = std::env::temp_dir().join(format!("talksage-cfg-cfgdir-{}", std::process::id()));
        std::fs::create_dir_all(&cfg_dir).unwrap();
        unsafe {
            std::env::set_var("TALKSAGE_CONFIG_DIR", &cfg_dir);
        }
        // 数据目录与配置目录分离：配置文件落在 TALKSAGE_CONFIG_DIR，而非数据目录
        let data_dir = std::env::temp_dir().join(format!("talksage-cfg-datadir-{}", std::process::id()));
        let file = default_config_file(&data_dir);
        assert_eq!(file, cfg_dir.join("talksage.toml"));
        unsafe {
            std::env::remove_var("TALKSAGE_CONFIG_DIR");
        }
        // 未设时回退到数据目录（兼容旧版）
        let file = default_config_file(&data_dir);
        assert_eq!(file, data_dir.join("talksage.toml"));
        std::fs::remove_dir_all(&cfg_dir).ok();
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

    #[test]
    fn scene_conversation_is_the_default() {
        let cfg = Config::default();
        let p = cfg.scene.effective();
        assert_eq!(cfg.scene.mode, SceneMode::Conversation);
        assert_eq!(p.vad_preset, VadPreset::Standard);
        assert!(!p.denoise_enabled);
        assert_eq!(p.min_segment_ms, 300);
        assert_eq!(p.asr_segment_ms, 4_000);
        assert_eq!(p.user_engine, "qwen3-asr");
        assert!(p.client_enabled);
        assert_eq!(p.language, "zh");
        assert_eq!(
            p.plugin_allowlist,
            vec!["term_explainer", "brief_retriever", "key_point_llm"]
        );
        assert_eq!(p.speaker_mode, SpeakerMode::Channel);
        assert_eq!(p.translation_mode, TranslationMode::Off);
        // 与场景 to_* 转换一致
        assert_eq!(p.to_vad_config().effective(), (0.50, 0.25, 0.50, 512, 30.0));
    }

    #[test]
    fn scene_templates_express_distinct_workloads() {
        let dictation = scene_params(SceneMode::Dictation);
        let bilingual = scene_params(SceneMode::Bilingual);
        let live_translation = scene_params(SceneMode::LiveTranslation);
        let meeting = scene_params(SceneMode::Meeting);
        let lecture = scene_params(SceneMode::Lecture);

        assert_eq!(dictation.vad_preset, VadPreset::Sensitive);
        assert!(!dictation.client_enabled, "听写场景应单流");
        assert!(dictation.plugin_allowlist.contains(&"key_point_llm".to_string()), "听写场景应允许要点聚合");
        assert_eq!(dictation.vad_min_silence_ms, Some(600));
        assert_eq!(dictation.asr_segment_ms, 5_000);

        assert_eq!(bilingual.translation_mode, TranslationMode::Bidirectional);
        assert_eq!(bilingual.language, "zh");
        assert_eq!(bilingual.client_language, "en");
        assert!(bilingual.client_enabled);

        assert_eq!(live_translation.translation_mode, TranslationMode::Bidirectional);
        assert!(!live_translation.client_enabled, "实时翻译默认单流");
        assert!(live_translation.plugin_allowlist.contains(&"translator".to_string()));
        assert_eq!(live_translation.language, "zh");
        assert_eq!(live_translation.client_language, "en");
        assert_eq!(live_translation.asr_segment_ms, 3_000);

        assert_eq!(meeting.speaker_mode, SpeakerMode::Voiceprint);
        assert_eq!(meeting.language, "zh");
        assert_eq!(meeting.asr_segment_ms, 6_000);

        assert_eq!(lecture.vad_max_speech_ms, Some(60_000));

        let cfg = SceneConfig { mode: SceneMode::Meeting, custom: scene_params(SceneMode::Custom) };
        assert_eq!(cfg.effective().vad_preset, meeting.vad_preset);

        let cfg_custom = SceneConfig { mode: SceneMode::Custom, custom: dictation.clone() };
        assert_eq!(cfg_custom.effective().vad_preset, VadPreset::Sensitive);
    }

    #[test]
    fn scene_custom_roundtrip_via_toml() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("talksage-cfg-scene-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talksage.toml");
        std::fs::write(
            &file,
            r#"
[scene]
mode = "dictation"
"#,
        )
        .unwrap();
        let mgr = ConfigManager::load(None, Some(&file)).unwrap();
        let cfg = mgr.snapshot();
        assert_eq!(cfg.scene.mode, SceneMode::Dictation);
        assert_eq!(cfg.scene.effective().vad_preset, VadPreset::Sensitive);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plugins_config_is_a_generic_table() {
        let mut c = Config::default();
        c.plugins.entries.insert(
            "term_explainer".into(),
            serde_json::json!({ "enabled": false, "cooldown_seconds": 99.0 }),
        );
        let toml = toml::to_string(&c).expect("应可序列化");
        assert!(toml.contains("term_explainer"), "通用表应写进 toml");
    }

    /// 通用表要能装宿主完全不认识的插件 —— 这是「加插件不用改配置结构」的前提。
    #[test]
    fn unknown_plugin_ids_survive_a_toml_roundtrip() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("talksage-cfg-plug-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talksage.toml");
        std::fs::write(
            &file,
            r#"
[plugins.some_future_plugin]
enabled = true
knob = 42
"#,
        )
        .unwrap();
        let cfg = ConfigManager::load(None, Some(&file)).unwrap().snapshot();
        assert_eq!(
            cfg.plugins.entries.get("some_future_plugin").and_then(|v| v.get("knob")),
            Some(&serde_json::json!(42))
        );
        // notes 仍是具名字段，不该被吸进通用表
        assert!(!cfg.plugins.entries.contains_key("notes"), "notes 不是插件");
        assert_eq!(cfg.plugins.notes.template, "standard_meeting");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 场景用 allowlist：不在列表里的插件一律关闭。
    /// 用 allowlist 而非 denylist —— 新增插件不会因为某个场景忘了更新而意外开启。
    #[test]
    fn dictation_scene_allows_key_point_llm_only() {
        let allow = scene_params(SceneMode::Dictation).plugin_allowlist;
        assert!(allow.contains(&"key_point_llm".to_string()), "听写模式应允许要点聚合");
        for id in ["term_explainer", "translator", "brief_retriever"] {
            assert!(!allow.contains(&id.to_string()), "听写模式不应允许 {id}");
        }
    }

    #[test]
    fn meeting_scene_allows_all_analysis_plugins() {
        let allow = scene_params(SceneMode::Meeting).plugin_allowlist;
        for id in ["term_explainer", "translator", "brief_retriever", "key_point_llm"] {
            assert!(allow.contains(&id.to_string()), "会议模式应允许 {id}");
        }
    }

    #[test]
    fn bilingual_scene_allows_all_analysis_plugins() {
        let allow = scene_params(SceneMode::Bilingual).plugin_allowlist;
        for id in ["term_explainer", "translator", "brief_retriever", "key_point_llm"] {
            assert!(allow.contains(&id.to_string()), "双语模式应允许 {id}");
        }
    }

    #[test]
    fn bilingual_scene_enables_translator() {
        let p = scene_params(SceneMode::Bilingual);
        assert!(p.plugin_allowlist.contains(&"translator".to_string()));
        assert_eq!(p.language, "zh");
        assert_eq!(p.client_language, "en");
    }

    /// 演讲保留简报插件，但没有客户流 —— 检索主讲人由 pipeline 的 include_user 覆盖实现。
    #[test]
    fn lecture_keeps_brief_retriever_without_a_client_stream() {
        let p = scene_params(SceneMode::Lecture);
        assert!(!p.client_enabled, "演讲是单流");
        assert!(
            p.plugin_allowlist.contains(&"brief_retriever".to_string()),
            "演讲应保留简报，而不是从 allowlist 拿掉"
        );
    }

    #[test]
    fn apply_scene_params_replaces_the_whole_allowlist() {
        let mut p = scene_params(SceneMode::Meeting);
        apply_scene_params(&mut p, &serde_json::json!({ "plugin_allowlist": ["translator"] }));
        assert_eq!(p.plugin_allowlist, vec!["translator"]);
        // 未提交时保留原值
        apply_scene_params(&mut p, &serde_json::json!({ "denoise_enabled": true }));
        assert_eq!(p.plugin_allowlist, vec!["translator"]);
    }

    #[test]
    fn apply_scene_params_clamps_asr_segment_duration() {
        let mut p = scene_params(SceneMode::Custom);
        apply_scene_params(&mut p, &serde_json::json!({ "asr_segment_ms": 3500 }));
        assert_eq!(p.asr_segment_ms, 3500);
        apply_scene_params(&mut p, &serde_json::json!({ "asr_segment_ms": 120_000 }));
        assert_eq!(p.asr_segment_ms, 60_000);
        apply_scene_params(&mut p, &serde_json::json!({ "asr_segment_ms": 0 }));
        assert_eq!(p.asr_segment_ms, 0);
    }

    #[test]
    fn apply_scene_params_updates_language_translation_and_speaker_policy() {
        let mut p = scene_params(SceneMode::Custom);
        apply_scene_params(
            &mut p,
            &serde_json::json!({
                "language": "en",
                "client_language": "zh",
                "translation_mode": "client_to_user",
                "speaker_mode": "voiceprint"
            }),
        );
        assert_eq!(p.language, "en");
        assert_eq!(p.client_language, "zh");
        assert_eq!(p.translation_mode, TranslationMode::ClientToUser);
        assert_eq!(p.speaker_mode, SpeakerMode::Voiceprint);
    }

    #[test]
    fn old_user_language_key_still_works_in_apply_scene_params() {
        let mut p = scene_params(SceneMode::Custom);
        apply_scene_params(
            &mut p,
            &serde_json::json!({ "user_language": "en" }),
        );
        assert_eq!(p.language, "en", "旧键 user_language 应映射到 language");
    }

    #[test]
    fn bilingual_mode_deserializes_from_old_translation_key() {
        let _env = env_lock();
        let dir = std::env::temp_dir().join(format!("talksage-cfg-bilingual-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talksage.toml");
        std::fs::write(&file, "[scene]\nmode = \"translation\"\n").unwrap();
        let cfg = ConfigManager::load(None, Some(&file)).unwrap().snapshot();
        assert_eq!(cfg.scene.mode, SceneMode::Bilingual, "旧 translation 配置应反序列化为 Bilingual");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scene_custom_params_persist() {
        let dir = std::env::temp_dir().join(format!("talksage-cfg-scenec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talksage.toml");
        let mgr = ConfigManager::load(Some(dir.clone()), Some(&file)).unwrap();
        mgr.update(|c| {
            c.scene.mode = SceneMode::Custom;
            c.scene.custom.vad_preset = VadPreset::Strict;
            c.scene.custom.min_segment_ms = 500;
            c.scene.custom.client_enabled = false;
        })
        .unwrap();
        let reloaded = ConfigManager::load(Some(dir.clone()), Some(&file)).unwrap();
        let c = reloaded.snapshot();
        assert_eq!(c.scene.mode, SceneMode::Custom);
        let p = c.scene.effective();
        assert_eq!(p.vad_preset, VadPreset::Strict);
        assert_eq!(p.min_segment_ms, 500);
        assert!(!p.client_enabled);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn terminology_normalizes_and_corrects_long_alias_first() {
        let mut terminology = TerminologyConfig {
            enabled: true,
            hotword_score: 1.5,
            terms: vec![" TalkSage ".into(), "".into(), "TalkSage".into(), "向量数据库".into()],
            corrections: Default::default(),
        };
        terminology.corrections.insert("向量".into(), "Vector".into());
        terminology.corrections.insert("向量数据库".into(), "Vector DB".into());
        terminology.corrections.insert("拓思者".into(), "TalkSage".into());
        assert_eq!(terminology.normalized_terms(), vec!["TalkSage", "向量数据库"]);
        assert_eq!(terminology.correct("拓思者使用向量数据库"), "TalkSage使用Vector DB");
    }

    #[test]
    fn asr_config_has_aliyun_fields() {
        let cfg = AsrConfig::default();
        assert_eq!(cfg.engine_zh, "qwen3-asr");
        assert_eq!(cfg.engine_en, "qwen3-asr");
        assert!(cfg.aliyun_access_key_id.is_empty());
        assert!(cfg.aliyun_access_key_secret.is_empty());
        assert!(cfg.aliyun_app_key.is_empty());
        assert_eq!(cfg.asr_mode, "auto");
    }

    #[test]
    fn disabled_terminology_is_a_passthrough() {
        let mut terminology = TerminologyConfig::default();
        terminology.terms.push("TalkSage".into());
        terminology.corrections.insert("拓思者".into(), "TalkSage".into());
        assert!(terminology.normalized_terms().is_empty());
        assert_eq!(terminology.correct("拓思者"), "拓思者");
    }
}

/// 设置页配置面：快照出口与更新入口的行为锁。
///
/// 这一组测试守的是同一件事 —— 设置页读到什么就该能原样写回什么。
/// 破了它，用户的表现就是「打开设置页点一下保存，配置被清空」。
#[cfg(test)]
mod config_plane_tests {
    use super::*;

    fn plugins_stub() -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::new()
    }

    fn masked(cfg: &Config) -> serde_json::Value {
        ui_config_json(cfg, plugins_stub(), SecretPolicy::Mask)
    }

    #[test]
    fn snapshot_carries_every_top_level_config_section() {
        // 手挑字段的快照每加一个配置段就漏一个（scene / recording / quality /
        // network 都漏过）。这里直接对着 Config 的序列化结果比键集合：
        // 新增配置段却没进快照 —— 也就是设置页读不到 —— 会当场红。
        let cfg = Config::default();
        let full = serde_json::to_value(&cfg).unwrap();
        let snapshot = masked(&cfg);
        for key in full.as_object().unwrap().keys() {
            assert!(
                snapshot.get(key).is_some(),
                "快照缺少配置段 `{key}`：设置页会拿默认值当真值，保存时覆盖用户配置"
            );
        }
    }

    #[test]
    fn snapshot_masks_credentials_but_keeps_identifiers() {
        let mut cfg = Config::default();
        cfg.llm.providers.insert(
            "deepseek".into(),
            LlmProviderConfig { base_url: None, model: "deepseek-chat".into(), api_key: "sk-1234567890abcdef".into() },
        );
        cfg.asr.aliyun_access_key_id = "LTAI5tSomeKeyId".into();
        cfg.asr.aliyun_access_key_secret = "verySecretValue123".into();
        cfg.asr.aliyun_app_key = "app-key-plain".into();
        cfg.server.token = "server-token-value".into();

        let v = masked(&cfg);
        let body = v.to_string();
        for secret in ["sk-1234567890abcdef", "verySecretValue123", "server-token-value"] {
            assert!(!body.contains(secret), "密钥明文出现在 headless 快照里: {secret}");
        }
        // 标识不是凭据：设置页要显示，「检查」按钮要拿去验签。
        assert_eq!(v["asr"]["aliyun_access_key_id"], "LTAI5tSomeKeyId");
        assert_eq!(v["asr"]["aliyun_app_key"], "app-key-plain");
    }

    #[test]
    fn desktop_snapshot_keeps_secrets_readable() {
        // 桌面端走 IPC：同进程同用户，输入框里本来就该显示真值。
        let mut cfg = Config::default();
        cfg.asr.aliyun_access_key_secret = "verySecretValue123".into();
        let v = ui_config_json(&cfg, plugins_stub(), SecretPolicy::Reveal);
        assert_eq!(v["asr"]["aliyun_access_key_secret"], "verySecretValue123");
    }

    #[test]
    fn masked_secret_written_back_unchanged_keeps_the_stored_value() {
        // 设置页拿到掩码 → 用户改了别的 tab → 保存时把掩码原样提交回来。
        // 这一步一旦按字面写入，用户的 key 就没了。
        let mut cfg = Config::default();
        cfg.llm.providers.insert(
            "deepseek".into(),
            LlmProviderConfig { base_url: None, model: "deepseek-chat".into(), api_key: "sk-1234567890abcdef".into() },
        );
        cfg.asr.aliyun_access_key_secret = "verySecretValue123".into();

        let snapshot = masked(&cfg);
        let updates = serde_json::json!({
            "llm": { "default": "deepseek", "providers": { "deepseek": { "api_key": snapshot["llm"]["providers"]["deepseek"]["api_key"] } } },
            "asr": { "aliyun_access_key_secret": snapshot["asr"]["aliyun_access_key_secret"] },
        });
        apply_updates(&mut cfg, &updates);

        assert_eq!(cfg.llm.providers["deepseek"].api_key, "sk-1234567890abcdef");
        assert_eq!(cfg.asr.aliyun_access_key_secret, "verySecretValue123");
    }

    #[test]
    fn a_real_new_secret_replaces_and_an_empty_one_clears() {
        let mut cfg = Config::default();
        cfg.llm.providers.insert(
            "deepseek".into(),
            LlmProviderConfig { base_url: None, model: "deepseek-chat".into(), api_key: "sk-old".into() },
        );

        apply_updates(&mut cfg, &serde_json::json!({
            "llm": { "providers": { "deepseek": { "api_key": "sk-brand-new-key" } } }
        }));
        assert_eq!(cfg.llm.providers["deepseek"].api_key, "sk-brand-new-key");

        // 清空必须仍然可行：空串是「用户主动删掉」，不是「没读到」。
        apply_updates(&mut cfg, &serde_json::json!({
            "llm": { "providers": { "deepseek": { "api_key": "" } } }
        }));
        assert_eq!(cfg.llm.providers["deepseek"].api_key, "");
    }

    #[test]
    fn short_secrets_are_masked_whole() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("   "), "");
        assert_eq!(mask_secret("short"), "••••••••");
        assert!(mask_secret("sk-1234567890abcdef").starts_with("sk-"));
        assert!(mask_secret("sk-1234567890abcdef").ends_with("cdef"));
    }

    #[test]
    fn secret_input_falls_back_to_stored_for_empty_or_masked() {
        let stored = "sk-1234567890abcdef";
        assert_eq!(resolve_secret_input(None, stored), stored);
        assert_eq!(resolve_secret_input(Some(""), stored), stored);
        assert_eq!(resolve_secret_input(Some(&mask_secret(stored)), stored), stored);
        // 用户真的改了 → 用新值（「检查」按钮要验的是输入框里的那把 key）。
        assert_eq!(resolve_secret_input(Some("sk-typed-by-user"), stored), "sk-typed-by-user");
    }

    #[test]
    fn updates_cover_the_sections_the_settings_page_submits() {
        // recording / quality / network / audio_source 曾经只有桌面端认（或两边都不认），
        // headless 保存后一切照旧 —— 用户改完设置以为生效了。
        let mut cfg = Config::default();
        apply_updates(&mut cfg, &serde_json::json!({
            "audio": { "audio_source": "loopback" },
            "recording": { "enabled": false, "dir": "D:/rec", "clean_silence": true },
            "quality": { "auto_detect": false, "silence_rms": 0.02 },
            "network": { "proxy": "  http://127.0.0.1:7890  " },
            "webhooks": { "enabled": true, "urls": ["https://example.com/hook", "  "] },
        }));
        assert_eq!(cfg.audio.audio_source, "loopback");
        assert!(!cfg.recording.enabled);
        assert_eq!(cfg.recording.dir, "D:/rec");
        assert!(cfg.recording.clean_silence);
        assert!(!cfg.quality.auto_detect);
        assert_eq!(cfg.quality.silence_rms, 0.02);
        assert_eq!(cfg.network.proxy, "http://127.0.0.1:7890");
        assert_eq!(cfg.webhooks.urls, vec!["https://example.com/hook".to_string()]);
    }

    #[test]
    fn quality_null_restores_defaults() {
        let mut cfg = Config::default();
        cfg.quality.silence_rms = 0.42;
        apply_updates(&mut cfg, &serde_json::json!({ "quality": serde_json::Value::Null }));
        assert_eq!(cfg.quality.silence_rms, QualityConfig::default().silence_rms);
    }

    #[test]
    fn snapshot_round_trip_changes_nothing() {
        // 设置页最常见的一次操作：打开、什么都不改、点保存。
        // 快照原样写回后配置必须逐字节相同 —— 这是整个配置面的验收条件。
        let mut cfg = Config::default();
        cfg.llm.providers.insert(
            "deepseek".into(),
            LlmProviderConfig { base_url: Some("https://api.deepseek.com/v1".into()), model: "deepseek-chat".into(), api_key: "sk-1234567890abcdef".into() },
        );
        cfg.asr.aliyun_access_key_secret = "verySecretValue123".into();
        cfg.scene.mode = SceneMode::Meeting;
        cfg.recording.dir = "D:/rec".into();
        cfg.network.proxy = "http://127.0.0.1:7890".into();
        cfg.webhooks.urls = vec!["https://example.com/hook".into()];

        let before = toml::to_string(&cfg).unwrap();
        let snapshot = masked(&cfg);
        apply_updates(&mut cfg, &snapshot);
        assert_eq!(toml::to_string(&cfg).unwrap(), before);
    }
}
