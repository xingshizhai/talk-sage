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

/// 场景模式：不同场景使用不同的参数组合（VAD 灵敏度/降噪/最短提交/引擎/插件/说话人）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneMode {
    /// 生活：日常对话/家庭闲聊——灵敏 VAD 抓短句弱语音，单流，关闭分析插件。
    Life,
    /// 会议：正式会议（默认）——双流（我 + 客户英文），分析插件全开。
    Meeting,
    /// 会谈：商务洽谈/一对一谈判——双流，术语/翻译/简报全开。
    Talk,
    /// 自定义：使用 `SceneConfig.custom` 全部参数。
    Custom,
}

impl Default for SceneMode {
    fn default() -> Self {
        Self::Meeting
    }
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
    /// 用户流引擎（paraformer-zh | zipformer-en）。
    pub user_engine: String,
    /// 是否启用客户流（双流；系统回环 + 英文引擎）。
    pub client_enabled: bool,
    /// 客户流引擎。
    pub client_engine: String,
    /// 该场景允许启用的分析类插件 id。不在列表里的一律关闭。
    ///
    /// 用 allowlist 而非 denylist —— 新增插件不会因为某个场景忘了更新而意外开启。
    /// 只约束**分析类**插件（术语/翻译/简报这类「会议辅助功能」）；短段抑制、
    /// 跨流去重、质量评估是基础设施，不受此列表影响（见
    /// `talksage_plugins::ANALYSIS_PLUGIN_IDS`）。
    pub plugin_allowlist: Vec<String>,
    /// 说话人识别（wespeaker + 主人声纹）。
    pub speaker_enabled: bool,
    /// 质量评估自动检测背景噪音（auto_detect）。
    pub noise_auto_detect: bool,
}

impl Default for SceneParams {
    fn default() -> Self {
        scene_params(SceneMode::Meeting)
    }
}

/// 分析类插件全开的 allowlist（会议 / 会谈 / 自定义共用）。
///
/// 这里的 id 必须与 `talksage_plugins::ANALYSIS_PLUGIN_IDS` 对齐 —— 配置层
/// 刻意不依赖插件层（依赖方向是「pipeline 实现、plugins 定义」），所以两处
/// 各存一份；一致性由 talksage-pipeline 的 `scene_allowlist` 测试锁住。
fn all_analysis_plugins() -> Vec<String> {
    ["term_explainer", "translator", "brief_retriever"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 内置场景参数模板。
pub fn scene_params(mode: SceneMode) -> SceneParams {
    match mode {
        SceneMode::Life => SceneParams {
            vad_preset: VadPreset::Sensitive, // 灵敏：抓短句/弱语音（日常对话碎句多）
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false, // 生活噪音多变，弱信号优先，默认不开门限
            denoise_gate: 0.008,
            min_segment_ms: 0, // 生活短句不丢
            user_engine: "paraformer-zh".into(),
            client_enabled: false, // 日常单方说话，不开双流
            client_engine: "zipformer-en".into(),
            // 省资源/安静：生活模式不允许任何分析类插件
            plugin_allowlist: Vec::new(),
            speaker_enabled: false, // 默认关闭说话人识别（回环双录时在线聚类会产生重复标签，先把实时转写做好）
            noise_auto_detect: true,
        },
        SceneMode::Meeting => SceneParams {
            // 与历史默认行为一致（VAD standard / 降噪关 / 双流 / 插件全开）
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 0, // 保持默认不限制（可在自定义中调整）
            user_engine: "paraformer-zh".into(),
            client_enabled: true,
            client_engine: "zipformer-en".into(),
            plugin_allowlist: all_analysis_plugins(),
            speaker_enabled: false, // 默认关闭说话人识别（先把实时转写做好）
            noise_auto_detect: true,
        },
        SceneMode::Talk => SceneParams {
            vad_preset: VadPreset::Standard,
            vad_threshold: None,
            vad_min_speech_ms: None,
            vad_min_silence_ms: None,
            vad_max_speech_ms: None,
            denoise_enabled: false,
            denoise_gate: 0.008,
            min_segment_ms: 300, // 谈判短促确认句较多，300ms 起才提交
            user_engine: "paraformer-zh".into(),
            client_enabled: true,
            client_engine: "zipformer-en".into(),
            plugin_allowlist: all_analysis_plugins(),
            speaker_enabled: false, // 默认关闭说话人识别（先把实时转写做好）
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
            user_engine: "paraformer-zh".into(),
            client_enabled: true,
            client_engine: "zipformer-en".into(),
            plugin_allowlist: all_analysis_plugins(),
            speaker_enabled: false, // 默认关闭说话人识别（先把实时转写做好）
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
            mode: SceneMode::Meeting,
            custom: scene_params(SceneMode::Meeting),
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
            SceneMode::Life => "生活",
            SceneMode::Meeting => "会议",
            SceneMode::Talk => "会谈",
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
    if let Some(v) = u.get("speaker_enabled").and_then(|v| v.as_bool()) {
        p.speaker_enabled = v;
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
    pub session: SessionConfig,
    pub recording: RecordingConfig,
    pub quality: QualityConfig,
    pub privacy: PrivacyConfig,
    pub server: ServerConfig,
    pub knowledge_base: KnowledgeBaseConfig,
    pub webhooks: WebhooksConfig,
    pub scene: SceneConfig,
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
            webhooks: WebhooksConfig::default(),
            scene: SceneConfig::default(),
        }
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
    /// 客户（英文）流式引擎。
    pub client_engine: String,
    /// 用户（中文）流式引擎。
    pub user_engine: String,
    /// 推理后端：auto | cpu | cuda | metal。
    pub backend: String,
    /// 专业术语热词和确定性纠错配置。
    pub terminology: TerminologyConfig,
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
            client_engine: "zipformer-en".into(),
            user_engine: "paraformer-zh".into(),
            backend: "auto".into(),
            terminology: TerminologyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub mic_device: Option<i32>,
    pub loopback_device: Option<i32>,
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

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mic_device: None,
            loopback_device: None,
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
            terminology: user.asr.terminology,
        },
        audio: AudioConfig {
            mic_device: user.audio.mic_device.or(default.audio.mic_device),
            loopback_device: user.audio.loopback_device.or(default.audio.loopback_device),
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
        webhooks: WebhooksConfig {
            enabled: user.webhooks.enabled,
            urls: user.webhooks.urls,
        },
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
                user_engine: user.scene.custom.user_engine,
                client_enabled: user.scene.custom.client_enabled,
                client_engine: user.scene.custom.client_engine,
                plugin_allowlist: user.scene.custom.plugin_allowlist,
                speaker_enabled: user.scene.custom.speaker_enabled,
                noise_auto_detect: user.scene.custom.noise_auto_detect,
            },
        },
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
        assert_eq!(c.audio.vad.effective(), (0.50, 0.25, 0.50, 512, 30.0));
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
            c.plugins
                .merge_entry("translator", &serde_json::json!({ "enabled": false }));
        })
        .unwrap();

        // 重新加载同一目录，应读到更新后的值
        let reloaded = ConfigManager::load(Some(dir.clone()), None).unwrap();
        assert_eq!(reloaded.snapshot().llm.default, "kimi");
        assert!(!reloaded.snapshot().plugins.get_bool("translator", "enabled", true));
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

    #[test]
    fn scene_meeting_template_matches_legacy_defaults() {
        // 默认场景=会议，有效参数应与历史默认一致（VAD standard / 降噪关 / 双流 / 插件全开）
        let cfg = Config::default();
        let p = cfg.scene.effective();
        assert_eq!(cfg.scene.mode, SceneMode::Meeting);
        assert_eq!(p.vad_preset, VadPreset::Standard);
        assert!(!p.denoise_enabled);
        assert_eq!(p.min_segment_ms, 0);
        assert_eq!(p.user_engine, "paraformer-zh");
        assert!(p.client_enabled);
        assert_eq!(
            p.plugin_allowlist,
            vec!["term_explainer", "translator", "brief_retriever"]
        );
        // 说话人识别默认关闭（回环双录时在线聚类产生重复标签；先把实时转写做好）
        assert!(!p.speaker_enabled);
        // 与场景 to_* 转换一致
        assert_eq!(p.to_vad_config().effective(), (0.50, 0.25, 0.50, 512, 30.0));
    }

    #[test]
    fn scene_life_and_talk_templates_differ() {
        let life = scene_params(SceneMode::Life);
        let talk = scene_params(SceneMode::Talk);
        let meeting = scene_params(SceneMode::Meeting);
        assert_eq!(life.vad_preset, VadPreset::Sensitive);
        assert!(!life.client_enabled, "生活场景应单流");
        assert!(life.plugin_allowlist.is_empty(), "生活场景应关闭分析插件");
        assert_eq!(talk.min_segment_ms, 300);
        assert!(talk.client_enabled);
        // 默认（会议）→ effective 用模板而非 custom
        let cfg = SceneConfig { mode: SceneMode::Meeting, custom: scene_params(SceneMode::Custom) };
        assert_eq!(cfg.effective().vad_preset, meeting.vad_preset);
        // 自定义 → 用 custom
        let cfg_custom = SceneConfig { mode: SceneMode::Custom, custom: life.clone() };
        assert_eq!(cfg_custom.effective().vad_preset, VadPreset::Sensitive);
    }

    #[test]
    fn scene_custom_roundtrip_via_toml() {
        let dir = std::env::temp_dir().join(format!("talksage-cfg-scene-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talksage.toml");
        std::fs::write(
            &file,
            r#"
[scene]
mode = "life"
"#,
        )
        .unwrap();
        let mgr = ConfigManager::load(None, Some(&file)).unwrap();
        let cfg = mgr.snapshot();
        assert_eq!(cfg.scene.mode, SceneMode::Life);
        // 生活模板生效（未写 custom）
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
    fn life_scene_allows_no_analysis_plugins() {
        let allow = scene_params(SceneMode::Life).plugin_allowlist;
        for id in ["term_explainer", "translator", "brief_retriever"] {
            assert!(!allow.contains(&id.to_string()), "生活模式不应允许 {id}");
        }
    }

    #[test]
    fn meeting_scene_allows_all_analysis_plugins() {
        let allow = scene_params(SceneMode::Meeting).plugin_allowlist;
        for id in ["term_explainer", "translator", "brief_retriever"] {
            assert!(allow.contains(&id.to_string()), "会议模式应允许 {id}");
        }
    }

    /// 会谈与会议一致 —— 阶段 5 之前两者的三个 *_enabled 都是 true。
    #[test]
    fn talk_scene_allows_all_analysis_plugins() {
        assert_eq!(
            scene_params(SceneMode::Talk).plugin_allowlist,
            scene_params(SceneMode::Meeting).plugin_allowlist
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
    fn scene_custom_params_persist() {
        let dir = std::env::temp_dir().join(format!("talksage-cfg-scenec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = ConfigManager::load(Some(dir.clone()), None).unwrap();
        mgr.update(|c| {
            c.scene.mode = SceneMode::Custom;
            c.scene.custom.vad_preset = VadPreset::Strict;
            c.scene.custom.min_segment_ms = 500;
            c.scene.custom.client_enabled = false;
        })
        .unwrap();
        let reloaded = ConfigManager::load(Some(dir.clone()), None).unwrap();
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
    fn disabled_terminology_is_a_passthrough() {
        let mut terminology = TerminologyConfig::default();
        terminology.terms.push("TalkSage".into());
        terminology.corrections.insert("拓思者".into(), "TalkSage".into());
        assert!(terminology.normalized_terms().is_empty());
        assert_eq!(terminology.correct("拓思者"), "拓思者");
    }
}
