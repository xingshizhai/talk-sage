//! 共享应用服务：Tauri / Server / CLI 共用的会话装配与落库。
//!
//! 适配器只负责传输（IPC / WS / 打印），不创建 Pipeline、插件或 LLM。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use talksage_asr::{EngineKind, EnginePool};
use talksage_config::{ConfigManager, SceneMode};
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
        }
    }
}

impl StartListen {
    /// 桌面 / headless UI 的默认监听。
    pub fn desktop() -> Self {
        Self::default()
    }

    /// 文件导入（单流、落库、不录音）。
    pub fn import_file(path: PathBuf, engine: EngineKind, speaker_label: String) -> Self {
        Self {
            user_input: AudioInput::File(path),
            user_engine: Some(engine),
            client: ClientCapture::Off,
            persist: true,
            record: Some(false),
            noise_level: 0.0,
            kb_folder_override: None,
            user_label: Some(speaker_label),
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
    /// 独立 SQLite writer；必须在 finalizer 之前 drain。
    session_writer: Option<SessionWriter>,
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

    pub fn is_paused(&self) -> bool {
        self.runtime.is_paused()
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
}

/// [`TalkSageService::recover_on_startup`] 的结果汇总。
#[derive(Debug, Default)]
pub struct RecoveryReport {
    /// 补头转正的录音最终路径。
    pub recordings: Vec<PathBuf>,
    /// 被收尾的未结束会话 id。
    pub sessions: Vec<i64>,
}

impl TalkSageService {
    pub fn new(config: Arc<ConfigManager>, sessions: Option<Arc<SessionStore>>, engines: Arc<EnginePool>) -> Self {
        Self {
            config,
            sessions,
            engines,
        }
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
        let snapshot = config.snapshot();
        let name = snapshot.llm.default.clone();
        let provider = snapshot.llm.providers.get(&name)?;
        if provider.api_key.is_empty() && name != "ollama" {
            return None;
        }
        Some(Arc::new(OpenAICompatProvider::new(
            provider.api_key.clone(),
            provider.model.clone(),
            provider
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string()),
        )))
    }

    /// 探测 models/ 根目录。
    pub fn resolve_models_dir() -> Option<PathBuf> {
        if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
            let p = PathBuf::from(d);
            if p.is_dir() {
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
        None
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
        let terminology = snapshot.asr.terminology.clone();
        let engine_options = talksage_asr::EngineOptions {
            hotwords: terminology.normalized_terms(),
            hotword_score: terminology.hotword_score.clamp(0.0, 10.0),
        };
        let scene = snapshot.scene.effective();
        // 内置场景只决定 VAD/插件组合；ASR 页的全局模型选择仍应生效。
        // 自定义场景则使用场景自身的模型设置。
        let configured_user_engine = if snapshot.scene.mode == SceneMode::Custom {
            &scene.user_engine
        } else {
            &snapshot.asr.user_engine
        };
        let configured_client_engine = if snapshot.scene.mode == SceneMode::Custom {
            &scene.client_engine
        } else {
            &snapshot.asr.client_engine
        };
        let user_engine = req
            .user_engine
            .or_else(|| EngineKind::from_name(configured_user_engine))
            .unwrap_or(EngineKind::ParaformerZh);
        let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
        let user_model = model_dir.join(user_engine.model_dir_name());
        if !vad_model.is_file() {
            return Err(anyhow!("缺少 VAD 模型: {}", vad_model.display()));
        }
        if !user_engine.is_available(&model_dir) {
            return Err(anyhow!("用户 ASR 模型未安装或文件不完整: {}", user_model.display()));
        }

        let want_record = req.record.unwrap_or(snapshot.recording.enabled);
        let recording_dir = if want_record {
            let dir = snapshot.recording.resolve_dir(self.config.data_dir());
            if let Err(e) = std::fs::create_dir_all(&dir) {
                log::warn!("创建录音目录失败（本次不录音）: {e}");
                None
            } else {
                Some(dir)
            }
        } else {
            None
        };

        let client_engine = EngineKind::from_name(configured_client_engine).unwrap_or(EngineKind::ZipformerEn);
        let client_model = model_dir.join(client_engine.model_dir_name());
        let client = Self::resolve_client_input(scene.client_enabled, &req.client).and_then(|input| {
            if !client_engine.is_available(&model_dir) {
                log::warn!("客户 ASR 模型未安装或文件不完整: {}；关闭客户流", client_model.display());
                return None;
            }
            Some(StreamConfig {
                engine_kind: client_engine,
                model_dir: client_model.clone(),
                input,
                speaker_id: 1,
                speaker_label: "客户".into(),
                engine_options: engine_options.clone(),
                terminology: terminology.clone(),
            })
        });

        let kb_folder = req.kb_folder_override.clone().or_else(|| {
            if snapshot.knowledge_base.enabled && !snapshot.knowledge_base.folder.is_empty() {
                Some(PathBuf::from(&snapshot.knowledge_base.folder))
            } else {
                None
            }
        });
        let kb = kb_folder.and_then(|folder| {
            let mut kb = talksage_knowledge::KnowledgeBase::new();
            kb.index_folder(&folder);
            if kb.chunk_count() > 0 {
                Some(Arc::new(kb))
            } else {
                log::warn!("知识库目录无 .md/.txt 内容: {}", folder.display());
                None
            }
        });

        let speaker = if scene.speaker_enabled {
            let spk_model = model_dir.join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx");
            let owner = speaker::load_owner_embedding(self.config.data_dir());
            if spk_model.is_file() && owner.is_some() {
                Some(SpeakerConfig {
                    model: spk_model,
                    owner_embedding: owner,
                    threshold: DEFAULT_THRESHOLD,
                    classify_user_stream: client.is_none(),
                })
            } else {
                log::warn!(
                    "说话人识别已请求但未启用：{}",
                    if !spk_model.is_file() { "缺少 WeSpeaker 模型" } else { "尚未注册主人声纹" }
                );
                None
            }
        } else {
            None
        };

        // 注册表在这里只建一次 —— 两条流共享同一批 filter 实例（跨流去重的前提）。
        let mut plugin_overrides = plugin_overrides_for(&snapshot.plugins, &scene);

        let plugin_ctx = PluginContext {
            kb,
            llm: Self::build_llm(&self.config),
            quality,
            webhook,
        };
        // webhook 默认关闭（会把会话内容发到外部）。这里只在「本次会话确实会落库、
        // 有东西可推」时把 finalizer 装上；装上不等于会发 —— 真正发不发由
        // [webhooks] 在会后再判一次（见 WebhookHost::push）。两道闸互不代替。
        merge_override(
            &mut plugin_overrides,
            "webhook",
            serde_json::json!({ "enabled": plugin_ctx.webhook.is_some() }),
        );
        let hooks = talksage_plugins::build_registry(
            &talksage_plugins::builtin_plugins(),
            &plugin_overrides,
            &plugin_ctx,
        );

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
                speaker_label: req.user_label.clone().unwrap_or_else(|| "我".into()),
                engine_options,
                terminology,
            },
            client,
            plugin_ctx,
            recording_dir,
            runtime: Arc::new(RuntimeParams::with_noise_level(req.noise_level)),
            speaker,
            engine_pool: Some(self.engines.clone()),
            hooks,
        })
    }

    /// 启动监听。`on_event` 由适配器提供（IPC emit / WS broadcast / 打印）。
    pub fn start(&self, req: StartListen, on_event: EventSink) -> Result<RunningListen> {
        let stats = Arc::new(Mutex::new(Vec::new()));
        let texts = Arc::new(Mutex::new(Vec::new()));
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
                }) as Arc<dyn QualityDeps>),
                Some(Arc::new(WebhookHost {
                    config: self.config.clone(),
                    store: store.clone(),
                }) as Arc<dyn WebhookDeps>),
            ),
            None => (None, None),
        };
        let cfg = self.build_live_config_with(&req, quality, webhook)?;

        let session_id = if let Some(store) = &sessions {
            let now = unix_secs();
            Some(store.start_session(now)?)
        } else {
            None
        };

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
        let sink: EventSink = Arc::new(move |ev: DomainEvent| {
            if let Some(writer) = &writer_tx {
                writer.enqueue(&ev);
            }
            on_event(ev);
        });

        // 与管道内跑的是同一批实例（HookRegistry 克隆的是 Arc）。
        let hooks = cfg.hooks.clone();
        let mut runtime = SessionRuntime::new(cfg);
        if let Err(e) = runtime.start(sink) {
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
            session_writer,
        })
    }

    /// 停止管道并跑会后 finalizer 链（质量评估、webhook）。
    ///
    /// 具体做什么由注册表决定，这里只负责「停 → 落库 → 收尾」这三步的次序。
    pub fn finish(&self, mut running: RunningListen) -> Result<Option<i64>> {
        running.runtime.stop();
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
        // finalizer 之间不经由 context 传值：webhook 的载荷是从库里现取的会话
        // 详情，meta 已由链上游的 session_quality 落库（顺序不变量见 builtin_plugins）。
        let report = running
            .hooks
            .run_finalizers(&talksage_plugins::FinalizeContext { session_id: sid });
        if !report.failed.is_empty() {
            log::warn!("会话 #{sid} 收尾有 {} 项失败: {:?}", report.failed.len(), report.failed);
        }
        Ok(Some(sid))
    }
}

/// 组装本次会话的插件配置覆盖表。
///
/// **合并顺序**：`plugin.default_config()` → 用户 `[plugins.<id>]` →
/// 宿主/场景最后裁决。前两步的第二步在这里搬运（第一步在 `build_registry`
/// 里与默认值合并），第三步就是本函数余下的部分。
///
/// 这里刻意不认识任何具体插件的配置结构：通用表原样透传，插件 id 只在
/// 「宿主必须裁决」的三处出现（场景 VAD 参数、跨流去重、webhook 宿主可用性），
/// 以及场景 allowlist 的循环里 —— 而那个循环遍历的是
/// `ANALYSIS_PLUGIN_IDS`，不是写死的三个名字。
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

    // 场景 allowlist 最后裁决：分析类插件不在列表里就关掉。只有分析类受此
    // 约束 —— filter/finalizer 是基础设施，不该被场景关掉（见
    // ANALYSIS_PLUGIN_IDS 的文档）。用 allowlist 而非 denylist：新增插件不会
    // 因为某个场景忘了更新而在该场景意外开启。
    //
    // 注意这是**单向**的：allowlist 只能关，不能开。列表里有某个插件而用户在
    // `[plugins.<id>]` 里写了 `enabled = false`，仍然是关 —— 沿用阶段 5 之前
    // 「场景开关与用户开关是两道与门」的语义。
    //
    // 有的插件还有第三道门（简报检索要求「知识库有内容」）—— 那类判断在插件
    // 自己的 register() 里靠 PluginContext 做，宿主这里不重复。
    for id in talksage_plugins::ANALYSIS_PLUGIN_IDS {
        if !scene.plugin_allowlist.iter().any(|a| a == id) {
            merge_override(&mut overrides, id, serde_json::json!({ "enabled": false }));
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

    /// 阶段 5 之前的判定是 `scene.X_enabled && snapshot.plugins.X.enabled`，
    /// 三个场景的 X_enabled 分别是 生活=false / 会议=true / 会谈=true。
    /// allowlist 必须复现这一表现。
    #[test]
    fn scene_allowlist_reproduces_the_old_scene_gating() {
        let plugins = talksage_config::PluginsConfig::default();
        for (mode, want_off) in [
            (SceneMode::Life, true),
            (SceneMode::Meeting, false),
            (SceneMode::Talk, false),
            (SceneMode::Custom, false),
        ] {
            let o = plugin_overrides_for(&plugins, &talksage_config::scene_params(mode));
            for id in talksage_plugins::ANALYSIS_PLUGIN_IDS {
                if want_off {
                    assert_eq!(enabled_in(&o, id), Some(false), "{mode:?} 应关掉 {id}");
                } else {
                    assert_ne!(enabled_in(&o, id), Some(false), "{mode:?} 不应关掉 {id}");
                }
            }
        }
    }

    /// 基础设施类插件不受场景 allowlist 约束 —— 生活模式也要有短段抑制、
    /// 跨流去重、指标与质量评估。
    #[test]
    fn infrastructure_plugins_survive_the_life_scene() {
        let o = plugin_overrides_for(
            &talksage_config::PluginsConfig::default(),
            &talksage_config::scene_params(SceneMode::Life),
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
        let on = talksage_plugins::ANALYSIS_PLUGIN_IDS[0];
        let off = talksage_plugins::ANALYSIS_PLUGIN_IDS[1];
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

        // 生活：allowlist 不允许，enabled 被压成 false，但其他键不动
        let life = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Life));
        assert_eq!(enabled_in(&life, on), Some(false));
        assert_eq!(
            life[on]["knob"],
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
        let o = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Talk));
        assert_eq!(o["short_segment"]["min_ms"], serde_json::json!(300));
    }

    /// `HOST_MANAGED_KEYS` 是设置页「这个控件置灰」的依据，必须与本文件
    /// `plugin_overrides_for` 的实际行为一致：声明为宿主裁决的键，用户写什么
    /// 都得被压掉。漂移的表现是设置页上出现一个能改却不生效的输入框。
    #[test]
    fn declared_host_managed_keys_really_override_user_config() {
        for (id, key) in talksage_plugins::HOST_MANAGED_KEYS {
            let mut plugins = talksage_config::PluginsConfig::default();
            // 用一个不可能与宿主值相同的哨兵：压过了就看不到它
            plugins.merge_entry(id, &serde_json::json!({ *key: "SENTINEL" }));
            let o = plugin_overrides_for(&plugins, &talksage_config::scene_params(SceneMode::Meeting));
            assert_ne!(
                o[*id][*key],
                serde_json::json!("SENTINEL"),
                "{id}.{key} 声明为宿主裁决，却没被 plugin_overrides_for 覆盖"
            );
        }
    }

    /// 配置层的 allowlist 与插件层的 ANALYSIS_PLUGIN_IDS 各存一份
    /// （talksage-config 刻意不依赖 talksage-plugins）。这里锁住两者不漂移。
    #[test]
    fn meeting_allowlist_matches_the_plugin_layers_analysis_ids() {
        let allow = talksage_config::scene_params(SceneMode::Meeting).plugin_allowlist;
        let ids: Vec<String> = talksage_plugins::ANALYSIS_PLUGIN_IDS
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
        c.scene.mode = SceneMode::Life;
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

    #[test]
    fn build_live_config_attaches_engine_pool() {
        let (svc, _dir) = temp_service(false);
        match svc.build_live_config(&StartListen::desktop()) {
            Ok(cfg) => assert!(cfg.engine_pool.is_some()),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("models") || msg.contains("VAD") || msg.contains("ASR"),
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
}
