//! 共享应用服务：Tauri / Server / CLI 共用的会话装配与落库。
//!
//! 适配器只负责传输（IPC / WS / 打印），不创建 Pipeline、插件或 LLM。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use talksage_asr::{EngineKind, EnginePool};
use talksage_config::{ConfigManager, SpeakerMode, TranslationMode};
use talksage_core::DomainEvent;
use talksage_llm::{LLMProvider, OpenAICompatProvider};
use talksage_plugins::{
    HookRegistry, QualityDeps, PluginContext, WebhookDeps,
};
use talksage_session::SessionStore;

use crate::finalize::{QualityHost, WebhookHost};
use crate::session_writer::SessionWriter;
use crate::speaker::{self, DEFAULT_THRESHOLD};
use crate::{AudioInput, EventSink, LivePipelineConfig, RuntimeParams, SessionRuntime, SpeakerConfig, StreamConfig};

/// 客户流采集策略。
#[derive(Debug, Clone)]
pub enum ClientCapture {
    /// 跟随场景开关；Windows 用系统回环，其他平台明确降级为单流（不静默改用麦克风）。
    Auto,
    /// CLI `--client` 显式指定。
    Explicit(AudioInput),
    /// 强制关闭双流。
    Off,
}

/// 启动监听请求。未列出的项跟随配置 / 场景。
pub struct StartListen {
    pub user_input: AudioInput,
    pub user_engine: Option<EngineKind>,
    pub client: ClientCapture,
    pub persist: bool,
    pub record: Option<bool>,
    pub noise_level: f32,
    pub kb_folder_override: Option<PathBuf>,
    /// 用户流显示名（默认「我」）。
    pub user_label: Option<String>,
    /// 会中材料包：vault 相对路径。
    pub pinned_note_paths: Vec<String>,
}

impl Default for StartListen {
    fn default() -> Self {
        Self {
            user_input: AudioInput::Mic(None),
            user_engine: None,
            client: ClientCapture::Auto,
            persist: true,
            record: None,
            noise_level: 0.0,
            kb_folder_override: None,
            user_label: None,
            pinned_note_paths: Vec::new(),
        }
    }
}

impl StartListen {
    /// 桌面 / headless UI 的默认监听。
    pub fn desktop() -> Self {
        Self::default()
    }

    /// 文件导入：与实时监听共用场景路由、插件和会话录音。
    pub fn import_file(path: PathBuf, speaker_label: String) -> Self {
        Self {
            user_input: AudioInput::File(path),
            user_engine: None,
            client: ClientCapture::Off,
            persist: true,
            record: Some(true),
            noise_level: 0.0,
            kb_folder_override: None,
            user_label: Some(speaker_label),
            pinned_note_paths: Vec::new(),
        }
    }
}

/// 正在进行的监听（Runtime + 可选 SQLite 会话）。
pub struct RunningListen {
    runtime: SessionRuntime,
    session_id: Option<i64>,
    /// 本次会话的钩子表（与管道内跑的是同一批实例）。`finish()` 用它跑
    /// finalizer 链 —— 依赖已在 register 时注入，此处只能调用，不能再改。
    hooks: HookRegistry,
    /// 事件发射器：手动 flush 时直接发射要点事件用。
    emit: EventSink,
    /// 插件上下文（含 LLM 句柄）：手动 flush 时直接调用 LLM 用。
    plugin_ctx: talksage_plugins::PluginContext,
    /// 独立 SQLite writer；必须在 finalizer 之前 drain。
    session_writer: Option<SessionWriter>,
    stats: Arc<Mutex<Vec<talksage_session::StreamMeta>>>,
    master_recording: Arc<Mutex<Option<String>>>,
}

impl RunningListen {
    pub fn set_noise_level(&self, level: f32) {
        self.runtime.set_noise_level(level);
    }

    pub fn noise_level(&self) -> f32 {
        self.runtime.noise_level()
    }

    pub fn set_paused(&self, paused: bool) {
        self.runtime.set_paused(paused);
    }

    pub fn set_playback_speed(&self, speed: f32) {
        self.runtime.set_playback_speed(speed);
    }

    pub fn is_paused(&self) -> bool {
        self.runtime.is_paused()
    }

    /// 手动触发要点聚合：在后台线程直接调用 LLM 处理当前 buffer 并发射事件。
    /// 返回诊断消息供日志记录。
    pub fn flush_key_points(&self) -> String {
        let has_llm = self.plugin_ctx.llm.is_some();
        let has_observer = self.hooks.has_key_point_llm();
        if !has_observer && !has_llm {
            return "LLM 未配置（请在设置→LLM 填写 API Key），插件无法激活".into();
        }
        if !has_observer {
            return "当前场景未启用「要点聚合（LLM）」插件，请在设置→场景→插件中开启".into();
        }
        if !has_llm {
            return "LLM 未配置，无法整理要点（请在设置→LLM 填写 API Key）".into();
        }
        let emit = self.emit.clone();
        let ctx = self.plugin_ctx.clone();
        let hooks = self.hooks.clone();
        std::thread::spawn(move || {
            hooks.flush_key_points_now(&ctx, &|ev| emit(ev));
        });
        "已启动后台整理".into()
    }

    /// 手动查询一个术语：发 Term 事件，界面显示的同时也随会话入库
    /// （走的是和自动提取同一条 sink，所以落库/展示都不用另写一套）。
    pub fn explain_term(&self, term: &str) -> anyhow::Result<String> {
        let llm = self
            .plugin_ctx
            .llm
            .clone()
            .ok_or_else(|| anyhow::anyhow!("LLM 未配置（请在设置→LLM 填写 API Key）"))?;
        let content = talksage_plugins::term_explainer::lookup_term(llm.as_ref(), term, "")?;
        (self.emit)(DomainEvent::Term {
            result_id: format!("term-manual-{}", unix_secs()),
            status: talksage_core::ResultStatus::Final,
            content: content.clone(),
        });
        Ok(content)
    }

    pub fn session_id(&self) -> Option<i64> {
        self.session_id
    }

    pub fn snapshot(&self) -> talksage_core::SessionSnapshot {
        self.runtime.snapshot()
    }
}

/// 桌面、headless、CLI 共享的用例入口。
#[derive(Clone)]
pub struct TalkSageService {
    config: Arc<ConfigManager>,
    sessions: Option<Arc<SessionStore>>,
    engines: Arc<EnginePool>,
    knowledge: Arc<crate::knowledge::KnowledgeHub>,
}

/// 会话级术语去重：同一个术语只放行第一条。
///
/// 只拦"有内容的 Final"——骨架和撤销骨架的空事件必须原样通过，否则界面上的
/// "识别中…"卡片会永远留在那里。
#[derive(Default)]
struct TermDedup {
    seen: Mutex<std::collections::HashSet<String>>,
}

impl TermDedup {
    fn allow(&self, ev: &DomainEvent) -> bool {
        let DomainEvent::Term { status: talksage_core::ResultStatus::Final, content, .. } = ev else {
            return true;
        };
        if content.trim().is_empty() {
            return true;
        }
        // 一个事件可能带多条术语（每行一条）：逐行判断，全是重复才拦下
        let mut seen = self.seen.lock().unwrap();
        let mut has_new = false;
        for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let key = talksage_core::term_key(line);
            if key.is_empty() || seen.insert(key) {
                has_new = true;
            }
        }
        if !has_new {
            log::debug!("术语去重：整条都已出现过，跳过 {content}");
        }
        has_new
    }
}

/// [`TalkSageService::recover_on_startup`] 的结果汇总。
#[derive(Debug, Default)]
pub struct RecoveryReport {
    /// 补头转正的录音最终路径。
    pub recordings: Vec<PathBuf>,
    /// 被收尾的未结束会话 id。
    pub sessions: Vec<i64>,
}

/// 从语言代码（"zh" / "en"）和全局 ASR 配置解析引擎种类。
/// 中文 → engine_zh，其他一律 engine_en。
fn engine_for_language(lang: &str, asr: &talksage_config::AsrConfig) -> EngineKind {
    if lang == "zh" {
        EngineKind::from_name(&asr.engine_zh).unwrap_or(EngineKind::ParaformerZh)
    } else {
        EngineKind::from_name(&asr.engine_en).unwrap_or(EngineKind::ZipformerEn)
    }
}

fn key_point_aggregation_policy(mode: talksage_config::SceneMode) -> Option<(u64, u64)> {
    match mode {
        talksage_config::SceneMode::LiveTranslation => Some((4, 15_000)),
        talksage_config::SceneMode::Conversation | talksage_config::SceneMode::Bilingual => Some((6, 30_000)),
        talksage_config::SceneMode::Dictation => Some((8, 45_000)),
        talksage_config::SceneMode::Meeting | talksage_config::SceneMode::Lecture => Some((12, 60_000)),
        talksage_config::SceneMode::Custom => None,
    }
}

impl TalkSageService {
    pub fn new(config: Arc<ConfigManager>, sessions: Option<Arc<SessionStore>>, engines: Arc<EnginePool>) -> Self {
        let knowledge = Arc::new(crate::knowledge::KnowledgeHub::new(config.clone()));
        Self {
            config,
            sessions,
            engines,
            knowledge,
        }
    }

    pub fn knowledge(&self) -> Arc<crate::knowledge::KnowledgeHub> {
        self.knowledge.clone()
    }

    /// 在宿主进程退出前主动释放常驻 ASR 模型。
    pub fn clear_engines(&self) {
        self.engines.clear();
    }

    /// 无需启动音频/ASR 的插件状态预检，供设置页、REST 和 doctor 使用。
    pub fn plugin_registrations(&self) -> Vec<talksage_plugins::PluginRegistration> {
        let snapshot = self.config.snapshot();
        let scene = snapshot.scene.effective();
        let mut overrides = plugin_overrides_for(&snapshot.plugins, &scene);
        let has_session_host = self.sessions.is_some();
        merge_override(
            &mut overrides,
            "webhook",
            serde_json::json!({ "enabled": has_session_host }),
        );
        self.knowledge.refresh_if_stale();
        let knowledge_base = self.knowledge.is_ready();
        let availability = talksage_plugins::CapabilityAvailability {
            llm: Self::build_llm(&self.config).is_some(),
            knowledge_base,
            translation_policy: true,
            quality_store: has_session_host,
            webhook: has_session_host,
        };
        talksage_plugins::evaluate_plugin_registrations(
            &talksage_plugins::builtin_plugins(),
            &overrides,
            availability,
        )
    }

    /// 启动恢复：处理上次异常退出留下的两类残留 —— 未完成录音与未结束会话。
    ///
    /// 适配器应在对外服务/开窗之前调用一次。
    pub fn recover_on_startup(&self) -> RecoveryReport {
        let recordings = self.recover_incomplete_recordings();
        let sessions = match self.sessions.as_ref() {
            Some(store) => match store.close_orphan_sessions() {
                Ok(ids) => {
                    if !ids.is_empty() {
                        log::info!("启动恢复：收尾 {} 个未正常结束的会话 {ids:?}", ids.len());
                    }
                    ids
                }
                Err(e) => {
                    log::warn!("启动恢复收尾会话失败: {e}");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        RecoveryReport { recordings, sessions }
    }

    /// 启动恢复：把上次异常退出残留的 `.part` 录音补头转正。
    ///
    /// 录音是顺序写入的，崩溃只影响最后一块；不恢复的话整段音频既进不了历史
    /// 也无法回放。返回恢复出的最终路径（空录音会被清理，不计入）。
    pub fn recover_incomplete_recordings(&self) -> Vec<PathBuf> {
        let snapshot = self.config.snapshot();
        let rec_dir = snapshot.recording.resolve_dir(self.config.data_dir());
        match talksage_audio::recover_orphan_recordings(&rec_dir) {
            Ok(paths) => {
                if !paths.is_empty() {
                    log::info!("启动恢复：转正 {} 个未完成录音", paths.len());
                }
                paths
            }
            Err(e) => {
                log::warn!("启动恢复录音扫描失败 {}: {e}", rec_dir.display());
                Vec::new()
            }
        }
    }

    pub fn config(&self) -> &ConfigManager {
        &self.config
    }

    pub fn sessions(&self) -> Option<&Arc<SessionStore>> {
        self.sessions.as_ref()
    }

    pub fn engines(&self) -> &Arc<EnginePool> {
        &self.engines
    }

    /// 根据配置构建 LLM（无 key 且非 ollama 时返回 None）。
    pub fn build_llm(config: &ConfigManager) -> Option<Arc<dyn LLMProvider>> {
        Self::build_chat_provider(config).map(|p| Arc::new(p) as Arc<dyn LLMProvider>)
    }

    /// 同上，但返回具体类型 —— AI 助手要用的 `stream_chat` 是固有方法，不在 trait 上。
    pub fn build_chat_provider(config: &ConfigManager) -> Option<OpenAICompatProvider> {
        let snapshot = config.snapshot();
        let name = snapshot.llm.default.clone();
        let provider = snapshot.llm.providers.get(&name)?;
        if provider.api_key.is_empty() && name != "ollama" {
            return None;
        }
        let proxy = snapshot.network.proxy_url().map(str::to_string);
        Some(OpenAICompatProvider::new(
            provider.api_key.clone(),
            provider.model.clone(),
            provider
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string()),
        ).with_proxy(proxy))
    }

    /// 探测 models/ 根目录。
    pub fn resolve_models_dir() -> Option<PathBuf> {
        if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
            let p = PathBuf::from(d);
            if !p.as_os_str().is_empty() && (p.is_dir() || std::fs::create_dir_all(&p).is_ok()) {
                return Some(p);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(base) = exe.parent() {
                for rel in ["../../models", "../../../models", "../models"] {
                    let cand = base.join(rel);
                    if cand.is_dir() {
                        return Some(cand);
                    }
                }
            }
        }
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for cand in [
            here.join("../../models"),
            here.join("../../../models"),
            PathBuf::from("models"),
            PathBuf::from("../models"),
        ] {
            if cand.is_dir() {
                return Some(cand);
            }
        }
        // 正式安装不能依赖可执行文件旁边存在可写的 models/。开发环境上面的
        // 仓库目录仍优先；找不到时统一落到用户数据目录并自动创建。
        let user_models = talksage_config::default_data_dir().join("models");
        std::fs::create_dir_all(&user_models).ok().map(|_| user_models)
    }

    /// 场景开启双流时解析客户输入：非 Windows 的 Auto 返回 None，并打日志。
    pub fn resolve_client_input(scene_client_enabled: bool, capture: &ClientCapture) -> Option<AudioInput> {
        match capture {
            ClientCapture::Off => None,
            ClientCapture::Explicit(input) => Some(input.clone()),
            ClientCapture::Auto => {
                if !scene_client_enabled {
                    return None;
                }
                #[cfg(windows)]
                {
                    Some(AudioInput::Loopback)
                }
                #[cfg(not(windows))]
                {
                    log::info!("场景启用了客户流，但系统回环仅 Windows 支持；本会话降级为麦克风单流");
                    None
                }
            }
        }
    }

    /// 不带会后依赖的装配（配置自检 / 不落库的场景）：finalizer 照常注册，
    /// 但没有宿主可调用，等于空转。
    pub fn build_live_config(&self, req: &StartListen) -> Result<LivePipelineConfig> {
        self.build_live_config_with(req, None, None)
    }

    fn build_live_config_with(
        &self,
        req: &StartListen,
        quality: Option<Arc<dyn QualityDeps>>,
        webhook: Option<Arc<dyn WebhookDeps>>,
    ) -> Result<LivePipelineConfig> {
        let model_dir =Self::resolve_models_dir().ok_or_else(|| anyhow!("未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）"))?;
        let snapshot = self.config.snapshot();
        let gpu = talksage_asr::GpuBackend::detect();
        let cloud_configured = !snapshot.asr.aliyun_access_key_id.trim().is_empty()
            && !snapshot.asr.aliyun_access_key_secret.trim().is_empty()
            && !snapshot.asr.aliyun_app_key.trim().is_empty();
        log::info!(
            "ASR 能力探测: physical_gpu={} runtime_backend={} accelerated={} mode={} configured_backend={} cloud_credentials_complete={} note={}",
            talksage_asr::GpuBackend::hardware_candidate(),
            gpu.display_name(),
            gpu.is_accelerated(),
            snapshot.asr.asr_mode,
            snapshot.asr.backend,
            cloud_configured,
            talksage_asr::GpuBackend::availability_note(),
        );
        let asr_route = talksage_asr::resolve_asr_route(
            &snapshot.asr.asr_mode,
            &snapshot.asr.backend,
            gpu,
            talksage_asr::CloudCredentials {
                access_key_id: &snapshot.asr.aliyun_access_key_id,
                access_key_secret: &snapshot.asr.aliyun_access_key_secret,
                app_key: &snapshot.asr.aliyun_app_key,
            },
        )
        .map_err(|error| {
            log::error!(
                "ASR 路由不可用: mode={} configured_backend={} runtime_backend={} cloud_credentials_complete={} error={}",
                snapshot.asr.asr_mode,
                snapshot.asr.backend,
                gpu.display_name(),
                cloud_configured,
                error,
            );
            error
        })?;
        let tokio_handle = tokio::runtime::Handle::try_current().ok();
        if asr_route == talksage_asr::AsrRoute::AliyunCloud && tokio_handle.is_none() {
            return Err(anyhow!("云端 ASR 需要 Tokio runtime，当前启动入口不支持云端模式"));
        }
        log::info!(
            "ASR 路由已确定: route={} detected_gpu={:?} configured_backend={}",
            asr_route.display_name(),
            gpu,
            snapshot.asr.backend,
        );
        let terminology = snapshot.asr.terminology.clone();
        let scene = snapshot.scene.effective();
        // 语言策略：language_mode="scene" 时按场景固定每条流的解码语言
        // （whisper.cpp 等支持语言参数的引擎避免自动检测漂移到英文）；
        // "auto" 时保留 None（模型自动检测）。
        let lang_opt = |lang: &str| -> Option<String> {
            if snapshot.asr.language_mode == "auto" {
                None
            } else {
                let l = lang.trim().to_lowercase();
                if l.is_empty() || l == "auto" { None } else { Some(l) }
            }
        };
        // user 流 / client 流分开构造：语言不同 → 引擎池键不同，各自独立实例。
        let user_engine_options = talksage_asr::EngineOptions {
            hotwords: terminology.normalized_terms(),
            hotword_score: terminology.hotword_score.clamp(0.0, 10.0),
            provider: String::new(),
            language: lang_opt(&scene.language),
        };
        let client_engine_options = talksage_asr::EngineOptions {
            hotwords: terminology.normalized_terms(),
            hotword_score: terminology.hotword_score.clamp(0.0, 10.0),
            provider: String::new(),
            language: lang_opt(&scene.client_language),
        };
        // 引擎解析规则：
        // - Custom 模式：用 scene.user_engine / scene.client_engine（全量用户控制）
        // - Bilingual：user 流 = scene.language 对应引擎，client 流 = scene.client_language 对应引擎
        // - 其他单语言场景：两流均用 scene.language 对应引擎（消除中英混杂）
        let (mut user_engine_kind, mut client_engine_kind) = match snapshot.scene.mode {
            talksage_config::SceneMode::Custom => (
                EngineKind::from_name(&scene.user_engine).unwrap_or(EngineKind::ParaformerZh),
                EngineKind::from_name(&scene.client_engine).unwrap_or(EngineKind::ZipformerEn),
            ),
            talksage_config::SceneMode::Bilingual => (
                engine_for_language(&scene.language, &snapshot.asr),
                engine_for_language(&scene.client_language, &snapshot.asr),
            ),
            _ => {
                let e = engine_for_language(&scene.language, &snapshot.asr);
                (e, e)
            }
        };
        // whisper.cpp GPU 路由：Metal（macOS）或 Vulkan（Windows）→ 用
        // Whisper large-v3-turbo Q5_0（中文/中英混说鲁棒性好，GPU 实时）。
        let gpu_backend = match asr_route {
            talksage_asr::AsrRoute::Local { backend } => backend,
            talksage_asr::AsrRoute::AliyunCloud => talksage_asr::GpuBackend::None,
        };
        if matches!(gpu_backend, talksage_asr::GpuBackend::Metal | talksage_asr::GpuBackend::Vulkan) {
            // 如果用户已明确选择某个 whisper.cpp GPU 模型，尊重其选择；否则默认 large-v3-turbo
            if !matches!(user_engine_kind, EngineKind::WhisperMediumMetal | EngineKind::WhisperLargeV3TurboMetal) {
                user_engine_kind = EngineKind::WhisperLargeV3TurboMetal;
                client_engine_kind = EngineKind::WhisperLargeV3TurboMetal;
            }
            log::info!("whisper.cpp GPU 路由已选择（{}），用户流={} 客户流={}", gpu_backend.display_name(), user_engine_kind.display_name(), client_engine_kind.display_name());
        }
        let user_engine = req.user_engine.unwrap_or(user_engine_kind);
        let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
        let user_model = model_dir.join(user_engine.model_dir_name());
        if !vad_model.is_file() {
            return Err(anyhow!("缺少 VAD 模型: {}", vad_model.display()));
        }
        if asr_route != talksage_asr::AsrRoute::AliyunCloud
            && !user_engine.is_available(&model_dir)
        {
            return Err(anyhow!("用户 ASR 模型未安装或文件不完整: {}", user_model.display()));
        }

        let want_record = req.record.unwrap_or(snapshot.recording.enabled);
        // 只解析录音目录，不在这里创建：有会话时 start() 会按会话目录覆盖并创建，
        // 无会话时 start() 兜底创建，避免在 data/ 下残留空的默认 recordings 目录。
        let recording_dir = if want_record {
            Some(snapshot.recording.resolve_dir(self.config.data_dir()))
        } else {
            None
        };

        let client_engine = client_engine_kind;
        let client_model = model_dir.join(client_engine.model_dir_name());
        let client = Self::resolve_client_input(scene.client_enabled, &req.client).and_then(|input| {
            if asr_route != talksage_asr::AsrRoute::AliyunCloud
                && !client_engine.is_available(&model_dir)
            {
                log::warn!("客户 ASR 模型未安装或文件不完整: {}；关闭客户流", client_model.display());
                return None;
            }
            Some(StreamConfig {
                engine_kind: client_engine,
                model_dir: client_model.clone(),
                input,
                speaker_id: 1,
                speaker_label: if scene.speaker_mode == SpeakerMode::Off { "讲话者".into() } else { "对方".into() },
                engine_options: client_engine_options.clone(),
                terminology: terminology.clone(),
            })
        });

        self.knowledge.refresh_with_folder(req.kb_folder_override.as_deref());
        let kb = if self.knowledge.is_ready() {
            Some(self.knowledge.index())
        } else {
            None
        };

        let speaker = if scene.speaker_mode == SpeakerMode::Voiceprint {
            let spk_model = model_dir.join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx");
            let owner = speaker::load_owner_embedding(self.config.data_dir());
            if spk_model.is_file() {
                Some(SpeakerConfig {
                    model: spk_model,
                    owner_embedding: owner,
                    threshold: DEFAULT_THRESHOLD,
                    classify_user_stream: client.is_none(),
                })
            } else {
                log::warn!("多人说话者区分已请求但未启用：缺少 WeSpeaker 模型");
                None
            }
        } else {
            None
        };

        // 注册表在这里只建一次 —— 两条流共享同一批 filter 实例（跨流去重的前提）。
        let mut plugin_overrides = plugin_overrides_for(&snapshot.plugins, &scene);
        // 段级 ASR 缩短后，聚合窗口也必须跟场景调整：翻译/对话优先
        // 低延迟，会议/课堂优先跨句上下文。自定义模式保留用户插件配置。
        let aggregation = key_point_aggregation_policy(snapshot.scene.mode);
        if let Some((batch_size, tail_timeout_ms)) = aggregation {
            merge_override(
                &mut plugin_overrides,
                "key_point_llm",
                serde_json::json!({ "batch_size": batch_size, "tail_timeout_ms": tail_timeout_ms }),
            );
        }

        let plugin_ctx = PluginContext {
            kb,
            llm: Self::build_llm(&self.config),
            quality,
            webhook,
            translation: Some(talksage_plugins::LiveTranslationPolicy {
                mode: match scene.translation_mode {
                    TranslationMode::Off => talksage_plugins::LiveTranslationMode::Off,
                    TranslationMode::ClientToUser => talksage_plugins::LiveTranslationMode::ClientToUser,
                    TranslationMode::Bidirectional => talksage_plugins::LiveTranslationMode::Bidirectional,
                },
                user_language: scene.language.clone(),           // 改：scene.user_language → scene.language
                client_language: scene.client_language.clone(),
            }),
        };
        // webhook 默认关闭（会把会话内容发到外部）。这里只在「本次会话确实会落库、
        // 有东西可推」时把 finalizer 装上；装上不等于会发 —— 真正发不发由
        // [webhooks] 在会后再判一次（见 WebhookHost::push）。两道闸互不代替。
        merge_override(
            &mut plugin_overrides,
            "webhook",
            serde_json::json!({ "enabled": plugin_ctx.webhook.is_some() }),
        );
        let registry_build = talksage_plugins::build_registry_with_report(
            &talksage_plugins::builtin_plugins(),
            &plugin_overrides,
            &plugin_ctx,
        );
        for registration in &registry_build.registrations {
            if registration.status != talksage_plugins::RegistrationStatus::Active
                && registration.status != talksage_plugins::RegistrationStatus::Disabled
            {
                log::warn!(
                    "插件注册状态: id={} status={:?} missing={:?} issues={:?}",
                    registration.id,
                    registration.status,
                    registration.missing_capabilities,
                    registration.issues,
                );
            }
        }
        let hooks = registry_build.hooks;

        Ok(LivePipelineConfig {
            vad_model,
            chunk_ms: 100,
            vad: scene.to_vad_config(),
            denoise: scene.to_denoise_config(),
            endpoint: snapshot.audio.endpoint.clone(),
            asr_threads: 4,
            input_gain_db: snapshot.audio.input_gain_db,
            user: StreamConfig {
                engine_kind: user_engine,
                model_dir: user_model,
                input: req.user_input.clone(),
                speaker_id: 0,
                speaker_label: req.user_label.clone().unwrap_or_else(|| {
                    if scene.speaker_mode == SpeakerMode::Off { "讲话者".into() } else { "我".into() }
                }),
                engine_options: user_engine_options,
                terminology,
            },
            client,
            plugin_ctx,
            recording_dir,
            runtime: Arc::new(RuntimeParams::with_noise_level(req.noise_level)),
            speaker,
            engine_pool: Some(self.engines.clone()),
            hooks,
            punct_enabled: snapshot.asr.punct_enabled,
            aliyun_access_key_id: snapshot.asr.aliyun_access_key_id.clone(),
            aliyun_access_key_secret: snapshot.asr.aliyun_access_key_secret.clone(),
            aliyun_app_key: snapshot.asr.aliyun_app_key.clone(),
            asr_route,
            tokio_handle,
            force_segment_ms: if user_engine.is_streaming() { 0 } else { scene.asr_segment_ms },
        })
    }

    /// 启动监听。`on_event` 由适配器提供（IPC emit / WS broadcast / 打印）。
    pub fn start(&self, req: StartListen, on_event: EventSink) -> Result<RunningListen> {
        let stats = Arc::new(Mutex::new(Vec::new()));
        let texts = Arc::new(Mutex::new(Vec::new()));
        let master_recording = Arc::new(Mutex::new(None));
        let persist = req.persist;
        let sessions = if persist { self.sessions.clone() } else { None };

        // 会后依赖按会话构造，在装配注册表之前 —— finalizer 一旦进了 HookRegistry
        // 就是不可变的 Arc，只有 register 这一个注入时机。不落库就没有依赖，
        // finalizer 空转。
        let (quality, webhook) = match &sessions {
            Some(store) => (
                Some(Arc::new(QualityHost {
                    config: self.config.clone(),
                    store: store.clone(),
                    stats: stats.clone(),
                    texts: texts.clone(),
                    master_recording: master_recording.clone(),
                    pinned_note_paths: req.pinned_note_paths.clone(),
                }) as Arc<dyn QualityDeps>),
                Some(Arc::new(WebhookHost {
                    config: self.config.clone(),
                    store: store.clone(),
                }) as Arc<dyn WebhookDeps>),
            ),
            None => (None, None),
        };
        let mut cfg = self.build_live_config_with(&req, quality, webhook)?;

        let session_id = if let Some(store) = &sessions {
            let now = unix_secs();
            Some(store.start_session(now)?)
        } else {
            None
        };

        // 录音与导出按会话目录归档：<data_dir>/sessions/<id>/recordings（一次会话一个目录）。
        // 在 session_id 创建后覆盖 recording_dir（build_live_config 阶段还不知道 id）。
        // 无会话（不落库）时兜底创建原目录，保证录音可用。
        if let Some(sid) = session_id {
            if cfg.recording_dir.is_some() {
                let rec_dir = talksage_config::session_recordings_dir(self.config.data_dir(), sid);
                match std::fs::create_dir_all(&rec_dir) {
                    Ok(()) => {
                        cfg.recording_dir = Some(rec_dir);
                        log::info!("会话 #{sid} 录音目录: {}", cfg.recording_dir.as_ref().unwrap().display());
                    }
                    Err(e) => log::warn!("创建会话录音目录失败（本次不录音）: {e}"),
                }
            }
        } else if let Some(dir) = &cfg.recording_dir {
            if let Err(e) = std::fs::create_dir_all(dir) {
                log::warn!("创建录音目录失败（本次不录音）: {e}");
                cfg.recording_dir = None;
            }
        }

        let mut session_writer = match (&sessions, session_id) {
            (Some(store), Some(sid)) => match SessionWriter::start(
                store.clone(),
                sid,
                stats.clone(),
                texts.clone(),
            ) {
                Ok(writer) => Some(writer),
                Err(err) => {
                    let _ = store.end_session(sid, unix_secs());
                    return Err(err.context("启动会话持久化线程失败"));
                }
            },
            _ => None,
        };
        let writer_tx = session_writer.as_ref().map(SessionWriter::sender);
        // 专业术语有三个来源（term_explainer / key_point_llm 关键词 / 手动查词），
        // 它们互不知情。这里是三条路唯一的共同出口，去重放在这一层，界面和入库
        // 就都不会出现同一个词的两条解释。
        let term_dedup = TermDedup::default();
        let sink: EventSink = Arc::new(move |ev: DomainEvent| {
            if !term_dedup.allow(&ev) {
                return;
            }
            if let Some(writer) = &writer_tx {
                writer.enqueue(&ev);
            }
            on_event(ev);
        });

        // 与管道内跑的是同一批实例（HookRegistry 克隆的是 Arc）。
        let hooks = cfg.hooks.clone();
        let plugin_ctx = cfg.plugin_ctx.clone();
        let mut runtime = SessionRuntime::new(cfg);
        if let Err(e) = runtime.start(sink.clone()) {
            if let Some(writer) = &mut session_writer {
                let _ = writer.finish();
            }
            if let (Some(store), Some(sid)) = (&sessions, session_id) {
                let _ = store.end_session(sid, unix_secs());
            }
            return Err(e);
        }
        Ok(RunningListen {
            runtime,
            session_id,
            hooks,
            emit: sink,
            plugin_ctx,
            session_writer,
            stats,
            master_recording,
        })
    }

    /// 停止管道并跑会后 finalizer 链（质量评估、webhook）。
    ///
    /// 具体做什么由注册表决定，这里只负责「停 → 落库 → 收尾」这三步的次序。
    pub fn finish(&self, mut running: RunningListen) -> Result<Option<i64>> {
        // 停止管道并等它完全收尾：会话统计（含录音路径）由管道线程在退出前
        // 发出，`build_master_recording` / `session_quality` 依赖这些统计。
        // 若 5s 内没停完（如 38MB×2 录音 flush 较慢），继续等——统计没就绪
        // 会导致历史回放缺主录音、meta 为空。
        if !running.runtime.stop_with_timeout(crate::STOP_JOIN_TIMEOUT) {
            log::info!("管道线程未在 {}s 内退出，继续等待录音收尾与会话统计…", crate::STOP_JOIN_TIMEOUT.as_secs());
            if !running.runtime.join_remaining(Duration::from_secs(30)) {
                log::warn!("管道线程 30s 后仍未退出，将跳过统计收尾（录音可能不完整）");
            }
        }
        if let Some(writer) = &mut running.session_writer {
            writer.finish()?;
        }
        let Some(sid) = running.session_id else {
            return Ok(None);
        };
        let Some(store) = &self.sessions else {
            return Ok(Some(sid));
        };
        let _ = store.end_session(sid, unix_secs());
        let master = build_master_recording(sid, &running.stats.lock().unwrap());
        *running.master_recording.lock().unwrap() = master;
        // finalizer 之间不经由 context 传值：webhook 的载荷是从库里现取的会话
        // 详情，meta 已由链上游的 session_quality 落库（顺序不变量见 builtin_plugins）。
        let report = running
            .hooks
            .run_finalizers(&talksage_plugins::FinalizeContext { session_id: sid });
        if !report.failed.is_empty() {
            log::warn!(
                "会话 #{sid} 收尾有 {} 项失败: {:?}（timeout={:?}, panic={:?}）",
                report.failed.len(),
                report.failed,
                report.timed_out,
                report.panicked,
            );
        } else {
            log::info!("会话 #{sid} finalizer 完成: {:?}", report.completed);
        }
        Ok(Some(sid))
    }
}

/// 生成面向历史回放的主录音。单流直接复用原始录音；双流生成左右声道 WAV。
fn build_master_recording(sid: i64, stats: &[talksage_session::StreamMeta]) -> Option<String> {
    let recordings = stats
        .iter()
        .filter_map(|s| s.recording.as_deref())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    match recordings.as_slice() {
        [] => None,
        [single] => Some(single.display().to_string()),
        [left, right, ..] => {
            // master 放在会话目录（recordings 的上一级）：<data>/sessions/<id>/master.wav
            let output = match left.parent().and_then(|p| p.parent()) {
                Some(dir) => dir.join(format!("session-{sid}_master.wav")),
                None => left.with_file_name(format!("session-{sid}_master.wav")),
            };
            match talksage_audio::wav::create_stereo_master(left, right, &output) {
                Ok(()) => {
                    log::info!("会话 #{sid} 完整双声道录音已生成: {}", output.display());
                    Some(output.display().to_string())
                }
                Err(e) => {
                    log::warn!("会话 #{sid} 完整录音生成失败，保留原始分轨: {e}");
                    None
                }
            }
        }
    }
}

/// 组装本次会话的插件配置覆盖表。
///
/// **合并顺序**：`plugin.default_config()` → 用户 `[plugins.<id>]` →
/// 宿主/场景最后裁决。前两步的第二步在这里搬运（第一步在 `build_registry`
/// 里与默认值合并），第三步就是本函数余下的部分。
///
/// 这里刻意不认识任何具体插件的配置结构：通用表原样透传，插件 id 只在
/// 「宿主必须裁决」处出现（场景 VAD 参数、跨流去重、简报是否检索主讲人、
/// webhook 宿主可用性），以及场景 allowlist 的循环里 —— 而那个循环遍历的是
/// descriptor 派生的分析插件列表，不是写死的三个名字。
///
/// webhook 不在这里裁决：它要看 `PluginContext.webhook` 是否有宿主，
/// 而那个 ctx 在调用点之后才构造。
fn plugin_overrides_for(
    plugins: &talksage_config::PluginsConfig,
    scene: &talksage_config::SceneParams,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut overrides: std::collections::HashMap<String, serde_json::Value> =
        plugins.entries.clone().into_iter().collect();

    // 宿主裁决的键：来自场景参数或运行期能力，用户配置改不动它们。
    merge_override(
        &mut overrides,
        "short_segment",
        serde_json::json!({ "min_ms": scene.min_segment_ms }),
    );
    merge_override(
        &mut overrides,
        "cross_stream_dedup",
        serde_json::json!({ "enabled": true }),
    );
    merge_override(
        &mut overrides,
        "brief_retriever",
        serde_json::json!({ "include_user": !scene.client_enabled }),
    );

    // 场景 allowlist 最后裁决：分析类插件不在列表里就关掉。只有分析类受此
    // 约束 —— filter/finalizer 是基础设施，不该被场景关掉（见
    // descriptor 分类）。用 allowlist 而非 denylist：新增插件不会
    // 因为某个场景忘了更新而在该场景意外开启。
    //
    // 注意这是**单向**的：allowlist 只能关，不能开。列表里有某个插件而用户在
    // `[plugins.<id>]` 里写了 `enabled = false`，仍然是关 —— 沿用阶段 5 之前
    // 「场景开关与用户开关是两道与门」的语义。
    //
    // 有的插件还有第三道门（简报检索要求「知识库有内容」）—— 那类判断在插件
    // 自己的 register() 里靠 PluginContext 做，宿主这里不重复。
    for id in talksage_plugins::analysis_plugin_ids() {
        if !scene.plugin_allowlist.iter().any(|a| a == id) {
            // 用户明确开启的插件，场景不强制关闭；未设置或关闭的才受场景约束
            let user_enabled = overrides
                .get(id)
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !user_enabled {
                merge_override(&mut overrides, id, serde_json::json!({ "enabled": false }));
            }
        }
    }

    overrides
}

/// 把 `patch` 的键并进某个插件的 override（用户在 `[plugins.<id>]` 里写的
/// 其他键保留）。宿主裁决的键覆盖用户值 —— 场景参数与运行期能力不该被
/// 配置文件推翻。
fn merge_override(
    overrides: &mut std::collections::HashMap<String, serde_json::Value>,
    id: &str,
    patch: serde_json::Value,
) {
    let Some(patch) = patch.as_object() else {
        return;
    };
    let entry = overrides
        .entry(id.to_string())
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !entry.is_object() {
        // 用户把 [plugins.<id>] 写成了标量：丢掉，按空表处理。
        *entry = serde_json::Value::Object(Default::default());
    }
    let Some(dst) = entry.as_object_mut() else {
        return;
    };
    for (k, v) in patch {
        dst.insert(k.clone(), v.clone());
    }
}

pub(crate) fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_config::{Config, ConfigManager, SceneMode};

    #[test]
    fn file_import_uses_scene_engine_and_keeps_playback_recording() {
        let request = StartListen::import_file(PathBuf::from("meeting.mp3"), "说话人".into());
        assert!(matches!(request.user_input, AudioInput::File(_)));
        assert_eq!(request.user_engine, None);
        assert_eq!(request.record, Some(true));
        assert!(request.persist);
        assert!(matches!(request.client, ClientCapture::Off));
    }

    fn temp_service(persist: bool) -> (TalkSageService, tempfile_dir::TempDir) {
        let dir = tempfile_dir::TempDir::new();
        let cfg = ConfigManager::from_config(Config::default(), dir.path().to_path_buf());
        let sessions = if persist {
            Some(Arc::new(
                SessionStore::open(&dir.path().join("sessions.db").to_string_lossy()).unwrap(),
            ))
        } else {
            None
        };
        (
            TalkSageService::new(Arc::new(cfg), sessions, EnginePool::new()),
            dir,
        )
    }

    #[test]
    fn master_recording_reuses_one_track_and_combines_two_tracks() {
        let dir = tempfile_dir::TempDir::new();
        let left = dir.path().join("mic.wav");
        let right = dir.path().join("system.wav");
        for (path, value) in [(&left, 0.1), (&right, -0.1)] {
            let mut recorder = talksage_audio::wav::WavRecorder::create(path, 16000).unwrap();
            recorder.write(&vec![value; 160]).unwrap();
            recorder.finish().unwrap();
        }
        let stream = |label: &str, path: &std::path::Path| talksage_session::StreamMeta {
            speaker_label: label.into(),
            recording: Some(path.display().to_string()),
            ..Default::default()
        };
        assert_eq!(build_master_recording(7, &[stream("我", &left)]), Some(left.display().to_string()));
        let master = build_master_recording(7, &[stream("我", &left), stream("对方", &right)]).unwrap();
        let master = PathBuf::from(master);
        assert!(master.is_file());
        let raw = std::fs::read(master).unwrap();
        assert_eq!(u16::from_le_bytes(raw[22..24].try_into().unwrap()), 2);
    }

    /// 避免给测试加 tempfile 依赖：手写临时目录。
    mod tempfile_dir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        /// 进程内单调计数器，保证并行测试拿到互不相同的目录。
        ///
        /// 不能只用时间戳：macOS 的 `SystemTime::now()` 实际只有微秒分辨率
        /// （`as_nanos()` 末三位恒为 0），连续取值约 95% 重复。并行测试里两个
        /// `TempDir` 会拿到同一路径，先结束的那个 `Drop` 删掉目录，另一个随即
        /// 报 `unable to open database file`。
        static SEQ: AtomicU64 = AtomicU64::new(0);

        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new() -> Self {
                let p = std::env::temp_dir().join(format!(
                    "talksage-svc-{}-{}",
                    std::process::id(),
                    SEQ.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use std::collections::HashSet;

            /// 并行创建必须拿到互不相同的路径 —— 这正是原实现（纯时间戳）
            /// 在 macOS 上失败的场景。
            #[test]
            fn concurrent_temp_dirs_never_collide() {
                let paths: HashSet<PathBuf> = std::thread::scope(|s| {
                    let handles: Vec<_> = (0..16)
                        .map(|_| s.spawn(|| TempDir::new().path().to_path_buf()))
                        .collect();
                    handles.into_iter().map(|h| h.join().unwrap()).collect()
                });
                assert_eq!(paths.len(), 16, "16 次并行创建应得到 16 个不同目录，实际: {paths:?}");
            }
        }
    }

    /// 启动时应把上次崩溃残留的 `.part` 录音补头转正，
    /// 否则那段音频既进不了历史，也没法回放。
    #[test]
    fn startup_recovers_orphan_recordings() {
        let dir = tempfile_dir::TempDir::new();
        let cfg = ConfigManager::from_config(Config::default(), dir.path().to_path_buf());
        let rec_dir = cfg.snapshot().recording.resolve_dir(dir.path());
        std::fs::create_dir_all(&rec_dir).unwrap();

        // 模拟崩溃残留：写了音频但没 finish
        let wav = rec_dir.join("2026-01-01_00-00-00_我.wav");
        {
            let mut rec = talksage_audio::WavRecorder::create(&wav, 16000).unwrap();
            rec.write(&vec![0.3; 640]).unwrap();
        }
        assert!(talksage_audio::part_path_of(&wav).is_file(), "前置条件：应有 .part");

        let svc = TalkSageService::new(Arc::new(cfg), None, EnginePool::new());
        let recovered = svc.recover_incomplete_recordings();

        assert_eq!(recovered.len(), 1, "应恢复 1 个残留录音: {recovered:?}");
        assert!(wav.is_file(), "残留录音应被转正为 .wav");
        assert!(!talksage_audio::part_path_of(&wav).exists(), ".part 应已消失");
        let (_, samples) = talksage_audio::read_wav(&wav).unwrap();
        assert_eq!(samples.len(), 640);
    }

    /// 启动恢复应同时处理两类残留：未完成录音 + 未结束会话。
    #[test]
    fn startup_recovery_closes_orphan_sessions_too() {
        let (svc, dir) = temp_service(true);
        let store = svc.sessions().unwrap();
        let crashed = store.start_session(1_000).unwrap();

        // 同时放一个残留录音
        let rec_dir = svc.config().snapshot().recording.resolve_dir(dir.path());
        std::fs::create_dir_all(&rec_dir).unwrap();
        let wav = rec_dir.join("crash.wav");
        {
            let mut rec = talksage_audio::WavRecorder::create(&wav, 16000).unwrap();
            rec.write(&vec![0.1; 320]).unwrap();
        }

        let report = svc.recover_on_startup();

        assert_eq!(report.recordings.len(), 1, "应恢复残留录音");
        assert_eq!(report.sessions, vec![crashed], "应收尾未结束会话");
        assert!(store.get_session(crashed).unwrap().ended_at.is_some());
        assert!(wav.is_file());
    }

    #[test]
    fn auto_client_is_loopback_only_on_windows() {
        let input = TalkSageService::resolve_client_input(true, &ClientCapture::Auto);
        #[cfg(windows)]
        assert!(matches!(input, Some(AudioInput::Loopback)));
        #[cfg(not(windows))]
        assert!(input.is_none());
        assert!(TalkSageService::resolve_client_input(false, &ClientCapture::Auto).is_none());
        assert!(TalkSageService::resolve_client_input(true, &ClientCapture::Off).is_none());
    }

    #[test]
    fn explicit_client_mic_is_never_used_for_auto() {
        // Auto 不得把客户流静默映射到麦克风（旧 headless 分叉）。
        if let Some(input) = TalkSageService::resolve_client_input(true, &ClientCapture::Auto) {
            assert!(!matches!(input, AudioInput::Mic(_)));
        }
    }

    /// 覆盖表里某插件的 enabled（缺省按插件自己的默认，即「没被裁决」）。
    fn enabled_in(
        o: &std::collections::HashMap<String, serde_json::Value>,
        id: &str,
    ) -> Option<bool> {
        o.get(id).and_then(|v| v.get("enabled")).and_then(|v| v.as_bool())
    }

    /// 每种内置场景必须严格按自己的 allowlist 裁决分析插件。
    #[test]
    fn scene_allowlist_controls_each_preset() {
        let plugins = talksage_config::PluginsConfig::default();
        for mode in [
            SceneMode::Dictation,
            SceneMode::Conversation,
            SceneMode::Bilingual,
            SceneMode::Meeting,
            SceneMode::Lecture,
            SceneMode::Custom,
        ] {
            let params = talksage_config::scene_params(mode);
            let o = plugin_overrides_for(&plugins, &talksage_config::scene_params(mode));
            for id in talksage_plugins::analysis_plugin_ids() {
                if params.plugin_allowlist.iter().any(|allowed| allowed == id) {
                    assert_ne!(enabled_in(&o, id), Some(false), "{mode:?} 应允许 {id}");
                } else {
                    assert_eq!(enabled_in(&o, id), Some(false), "{mode:?} 应关掉 {id}");
                }
            }
        }
    }

    #[test]
    fn key_point_aggregation_latency_follows_scene() {
        assert_eq!(key_point_aggregation_policy(SceneMode::LiveTranslation), Some((4, 15_000)));
        assert_eq!(key_point_aggregation_policy(SceneMode::Conversation), Some((6, 30_000)));
        assert_eq!(key_point_aggregation_policy(SceneMode::Meeting), Some((12, 60_000)));
        assert_eq!(key_point_aggregation_policy(SceneMode::Custom), None);
    }

    /// 基础设施类插件不受场景 allowlist 约束 —— 生活模式也要有短段抑制、
    /// 跨流去重、指标与质量评估。
    #[test]
    fn infrastructure_plugins_survive_the_life_scene() {
        let o = plugin_overrides_for(
            &talksage_config::PluginsConfig::default(),
            &talksage_config::scene_params(SceneMode::Dictation),
        );
        for id in ["conversation_metrics", "session_quality"] {
            assert_eq!(enabled_in(&o, id), None, "{id} 不该被场景裁决");
        }
        assert_eq!(enabled_in(&o, "cross_stream_dedup"), Some(true));
    }

    /// 用户 `[plugins.<id>]` 的键要能穿到 override 表里，且 allowlist 只能关不能开。
    ///
    /// 取 id 而不写死名字：本文件不该认识任何具体插件，测试也一样。
    #[test]
    fn user_entries_pass_through_and_allowlist_only_turns_off() {
        let analysis = talksage_plugins::analysis_plugin_ids();
        let on = analysis[0];
        let off = analysis[1];
        let mut plugins = talksage_config::PluginsConfig::default();
        plugins.merge_entry(on, &serde_json::json!({ "enabled": true, "knob": 99.0 }));
        plugins.merge_entry(off, &serde_json::json!({ "enabled": false }));

        // 会议：allowlist 允许，用户值原样保留
        let meeting =
            plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Meeting));
        assert_eq!(enabled_in(&meeting, on), Some(true));
        assert_eq!(meeting[on]["knob"], serde_json::json!(99.0));
        assert_eq!(
            enabled_in(&meeting, off),
            Some(false),
            "用户关掉的，场景允许也不该打开"
        );

        // 生活：allowlist 不允许，但用户明确开启的插件不应被压掉
        let dictation = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Dictation));
        assert_eq!(enabled_in(&dictation, on), Some(true), "用户明确开启的插件，场景不应强制关闭");
        assert_eq!(
            dictation[on]["knob"],
            serde_json::json!(99.0),
            "场景只裁决 enabled，不该抹掉用户其他配置"
        );
    }

    /// 场景的 min_segment_ms 必须压过用户 `[plugins.short_segment]`
    /// —— 场景参数是宿主裁决的键。
    #[test]
    fn scene_min_segment_ms_wins_over_user_config() {
        let mut plugins = talksage_config::PluginsConfig::default();
        plugins.merge_entry("short_segment", &serde_json::json!({ "min_ms": 7 }));
        let o = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Conversation));
        assert_eq!(o["short_segment"]["min_ms"], serde_json::json!(300));
    }

    /// 无客户流且 allowlist 含简报（演讲）时，必须检索主讲人；有客户流（会议）则不检索主人。
    #[test]
    fn brief_include_user_follows_client_stream_availability() {
        let plugins = talksage_config::PluginsConfig::default();
        let lecture = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Lecture));
        assert_eq!(
            lecture["brief_retriever"]["include_user"],
            serde_json::json!(true),
            "演讲无客户流，简报应检索主讲人"
        );
        let meeting = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Meeting));
        assert_eq!(
            meeting["brief_retriever"]["include_user"],
            serde_json::json!(false),
            "会议有客户流，简报只检索对方"
        );
    }

    /// descriptor 的 host_managed 是设置页「这个控件置灰」的依据，必须与本文件
    /// `plugin_overrides_for` 的实际行为一致：声明为宿主裁决的键，用户写什么
    /// 都得被压掉。漂移的表现是设置页上出现一个能改却不生效的输入框。
    #[test]
    fn declared_host_managed_keys_really_override_user_config() {
        for (id, key) in talksage_plugins::host_managed_keys() {
            let mut plugins = talksage_config::PluginsConfig::default();
            // 用一个不可能与宿主值相同的哨兵：压过了就看不到它
            plugins.merge_entry(id, &serde_json::json!({ *key: "SENTINEL" }));
            let o = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Meeting));
            assert_ne!(
                o[id][key],
                serde_json::json!("SENTINEL"),
                "{id}.{key} 声明为宿主裁决，却没被 plugin_overrides_for 覆盖"
            );
        }
    }

    /// 配置层的 allowlist 与插件 descriptor 派生 id 各存一份
    /// （talksage-config 刻意不依赖 talksage-plugins）。这里锁住两者不漂移。
    #[test]
    fn meeting_allowlist_matches_the_plugin_layers_analysis_ids() {
        let allow = talksage_config::scene_params(SceneMode::Meeting).plugin_allowlist;
        let ids: Vec<String> = talksage_plugins::analysis_plugin_ids()
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(allow, ids, "会议 allowlist 应正好是全部分析类插件");
    }

    #[test]
    fn meeting_scene_enables_client_stream_flag() {
        let mut c = Config::default();
        c.scene.mode = SceneMode::Meeting;
        assert!(c.scene.effective().client_enabled);
        c.scene.mode = SceneMode::Dictation;
        assert!(!c.scene.effective().client_enabled);
    }

    #[test]
    fn persist_ignores_partial_segments() {
        let (svc, _dir) = temp_service(true);
        let store = svc.sessions().unwrap().clone();
        let sid = store.start_session(1).unwrap();
        let stats = Arc::new(Mutex::new(Vec::new()));
        let texts = Arc::new(Mutex::new(Vec::new()));
        let mut writer = SessionWriter::start(store.clone(), sid, stats, texts.clone()).unwrap();
        let tx = writer.sender();
        tx.enqueue(&DomainEvent::Segment {
                speaker_id: 0,
                speaker_label: "我".into(),
                speaker_attribution: None,
                text: "草稿".into(),
                is_partial: true,
                ts_ms: 1,
                duration_ms: 0,
                rms: 0.0,
                revision: 0,
                start_sample: 0,
                end_sample: 0,
            });
        tx.enqueue(&DomainEvent::Segment {
                speaker_id: 0,
                speaker_label: "我".into(),
                speaker_attribution: None,
                text: "定稿".into(),
                is_partial: false,
                ts_ms: 2,
                duration_ms: 400,
                rms: 0.1,
                revision: 0,
                start_sample: 0,
                end_sample: 6400,
            });
        // 刻意保留 sender clone：writer 的显式 Shutdown 不能依赖所有事件源
        // 都及时 drop（Pipeline 超时停止时仍可能持有一份）。
        writer.finish().unwrap();
        let detail = store.get_session(sid).unwrap();
        assert_eq!(detail.segments.len(), 1);
        assert_eq!(detail.segments[0].text, "定稿");
        assert_eq!(texts.lock().unwrap().as_slice(), ["定稿"]);
    }

    /// 三个来源（自动提取 / 要点关键词 / 手动查词）各自解释同一个词时，
    /// 只有第一条能出去 —— 线上见过「付鹏」「雷曼兄弟」各两条、解释还不一样。
    #[test]
    fn term_dedup_keeps_only_the_first_explanation() {
        let dedup = TermDedup::default();
        let term = |content: &str| DomainEvent::Term {
            result_id: "t".into(),
            status: talksage_core::ResultStatus::Final,
            content: content.into(),
        };

        assert!(dedup.allow(&term("付鹏：经济学家，以直白敢言著称")));
        assert!(!dedup.allow(&term("付鹏：指东北证券首席经济学家")), "同一个词的第二条解释应被拦下");
        assert!(dedup.allow(&term("雷曼兄弟：美国投资银行")), "不同的词照常放行");
        assert!(dedup.allow(&term("MOQ：最小起订量")), "首次出现应放行");
        assert!(!dedup.allow(&term("moq: minimum order quantity")), "大小写/中英冒号不影响判重");

        // 一个事件里多条术语：只要有一条是新的就整条放行
        assert!(dedup.allow(&term("付鹏：经济学家\nSLA：服务等级协议")));
        assert!(!dedup.allow(&term("付鹏：经济学家\nSLA：服务等级协议")), "全部重复才拦下");
    }

    /// 骨架与撤销骨架的空事件必须原样通过，否则"识别中…"会永远挂在界面上。
    #[test]
    fn term_dedup_never_blocks_skeletons() {
        let dedup = TermDedup::default();
        let skeleton = DomainEvent::Term {
            result_id: "t".into(),
            status: talksage_core::ResultStatus::Skeleton,
            content: "专业术语识别中…".into(),
        };
        let dismiss = DomainEvent::Term {
            result_id: "t".into(),
            status: talksage_core::ResultStatus::Final,
            content: String::new(),
        };
        assert!(dedup.allow(&skeleton));
        assert!(dedup.allow(&skeleton), "骨架可以重复出现");
        assert!(dedup.allow(&dismiss));
        assert!(dedup.allow(&dismiss));
    }

    /// 专业术语要跟着会话一起入库，且只入「有内容的 Final」：
    /// 骨架卡片和撤销骨架用的空事件都不该留在历史里（曾经 61 条里 49 条是空的），
    /// 一次给出的多条术语也要拆成独立记录，历史页才能逐条列。
    #[test]
    fn persist_terms_splits_lines_and_skips_skeletons() {
        let (svc, _dir) = temp_service(true);
        let store = svc.sessions().unwrap().clone();
        let sid = store.start_session(1).unwrap();
        let stats = Arc::new(Mutex::new(Vec::new()));
        let texts = Arc::new(Mutex::new(Vec::new()));
        let mut writer = SessionWriter::start(store.clone(), sid, stats, texts).unwrap();
        let tx = writer.sender();

        // 骨架：只给界面看，不入库
        tx.enqueue(&DomainEvent::Term {
            result_id: "t1".into(),
            status: talksage_core::ResultStatus::Skeleton,
            content: "专业术语识别中…".into(),
        });
        // 一次给出两条：应拆成两条记录
        tx.enqueue(&DomainEvent::Term {
            result_id: "t1".into(),
            status: talksage_core::ResultStatus::Final,
            content: "MOQ：最小起订量。\n灰度发布：新版本先放小比例用户。".into(),
        });
        // 撤销骨架的空事件：不入库
        tx.enqueue(&DomainEvent::Term {
            result_id: "t2".into(),
            status: talksage_core::ResultStatus::Final,
            content: String::new(),
        });
        writer.finish().unwrap();

        let detail = store.get_session(sid).unwrap();
        assert_eq!(
            detail.terms,
            vec!["MOQ：最小起订量。", "灰度发布：新版本先放小比例用户。"],
            "应只留两条 Final 术语，且一行一条"
        );
    }

    #[test]
    fn persist_final_key_points() {
        let (svc, _dir) = temp_service(true);
        let store = svc.sessions().unwrap().clone();
        let sid = store.start_session(1).unwrap();
        let stats = Arc::new(Mutex::new(Vec::new()));
        let texts = Arc::new(Mutex::new(Vec::new()));
        let mut writer = SessionWriter::start(store.clone(), sid, stats, texts).unwrap();
        let tx = writer.sender();
        tx.enqueue(&DomainEvent::KeyPoint {
            result_id: "kp-1".into(),
            status: talksage_core::ResultStatus::Final,
            category: talksage_core::KeyPointCategory::Requirement,
            content: "We need NPI samples".into(),
            ts_ms: 42,
            manual: false,
            owner: None,
            due_date: None,
            source_refs: Vec::new(),
        });
        writer.finish().unwrap();
        let detail = store.get_session(sid).unwrap();
        assert_eq!(detail.key_points.len(), 1);
        assert_eq!(detail.key_points[0].ts_ms, 42);
        assert!(detail.key_points[0].content.contains("NPI"));
    }

    #[test]
    fn build_live_config_attaches_engine_pool() {
        let (svc, _dir) = temp_service(false);
        match svc.build_live_config(&StartListen::desktop()) {
            Ok(cfg) => assert!(cfg.engine_pool.is_some()),
            Err(e) => {
                // 无模型/无 GPU 环境下会因模型缺失或阿里云路由探测失败而提前返回；
                // 这类环境性错误不算 bug，断言只要不 panic 即可。
                let msg = e.to_string();
                assert!(
                    msg.contains("models") || msg.contains("VAD") || msg.contains("ASR")
                        || msg.contains("GPU") || msg.contains("阿里云"),
                    "unexpected error: {msg}"
                );
            }
        }
    }

    #[test]
    fn build_llm_none_without_key() {
        let dir = tempfile_dir::TempDir::new();
        let cfg = ConfigManager::from_config(Config::default(), dir.path().to_path_buf());
        assert!(TalkSageService::build_llm(&cfg).is_none());
    }

    /// 非自定义场景：引擎由 engine_for_language(scene.language) 决定（中文 → engine_zh，
    /// 其他 → engine_en）；自定义场景：用 scene.user_engine / scene.client_engine。
    #[test]
    fn engine_resolution_uses_engine_for_language() {
        use talksage_config::{AsrConfig, SceneMode};

        // engine_for_language: zh → engine_zh, other → engine_en
        let mut asr = AsrConfig::default();
        asr.engine_zh = "paraformer-zh".into();
        asr.engine_en = "zipformer-en".into();
        let zh_engine = engine_for_language("zh", &asr);
        let en_engine = engine_for_language("en", &asr);
        assert_eq!(zh_engine, EngineKind::ParaformerZh, "zh 语言应解析为 ParaformerZh");
        assert_eq!(en_engine, EngineKind::ZipformerEn, "en 语言应解析为 ZipformerEn");

        // 自定义场景 → 用场景参数（即使全局不同）
        let dir = tempfile_dir::TempDir::new();
        let mut cfg2 = talksage_config::Config::default();
        cfg2.scene.mode = SceneMode::Custom;
        cfg2.scene.custom.user_engine = "qwen3-asr".into();
        let mgr2 = ConfigManager::from_config(cfg2, dir.path().join("cfg2").to_path_buf());
        let snap2 = mgr2.snapshot();
        let scene2 = snap2.scene.effective();
        let user2 = match snap2.scene.mode {
            SceneMode::Custom => EngineKind::from_name(&scene2.user_engine),
            _ => None,
        };
        assert_eq!(user2, Some(EngineKind::Qwen3Asr), "自定义场景应使用场景参数引擎");
    }
}
