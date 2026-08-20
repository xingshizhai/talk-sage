//! 共享应用服务：Tauri / Server / CLI 共用的会话装配与落库。
//!
//! 适配器只负责传输（IPC / WS / 打印），不创建 Pipeline、插件或 LLM。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use talksage_asr::{EngineKind, EnginePool};
use talksage_config::ConfigManager;
use talksage_core::{DomainEvent, ResultStatus, TranscriptSegment};
use talksage_llm::{LLMProvider, OpenAICompatProvider};
use talksage_plugins::{
    brief_retriever::BriefRetrieverPlugin, term_explainer::TermExplainerPlugin, translator::TranslatorPlugin,
    AnalyzerPlugin, PluginContext,
};
use talksage_session::{SessionStore, StreamMeta};

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
    stats: Arc<Mutex<Vec<StreamMeta>>>,
    texts: Arc<Mutex<Vec<String>>>,
}

impl RunningListen {
    pub fn set_noise_level(&self, level: f32) {
        self.runtime.set_noise_level(level);
    }

    pub fn noise_level(&self) -> f32 {
        self.runtime.noise_level()
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

    pub fn build_live_config(&self, req: &StartListen) -> Result<LivePipelineConfig> {
        let model_dir = Self::resolve_models_dir().ok_or_else(|| anyhow!("未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）"))?;
        let snapshot = self.config.snapshot();
        let scene = snapshot.scene.effective();
        let user_engine = req
            .user_engine
            .or_else(|| EngineKind::from_name(&scene.user_engine))
            .unwrap_or(EngineKind::ParaformerZh);
        let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
        let user_model = model_dir.join(user_engine.model_dir_name());
        if !vad_model.is_file() {
            return Err(anyhow!("缺少 VAD 模型: {}", vad_model.display()));
        }
        if !user_model.is_dir() {
            return Err(anyhow!("缺少用户 ASR 模型目录: {}", user_model.display()));
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

        let client_engine = EngineKind::from_name(&scene.client_engine).unwrap_or(EngineKind::ZipformerEn);
        let client_model = model_dir.join(client_engine.model_dir_name());
        let client = Self::resolve_client_input(scene.client_enabled, &req.client).and_then(|input| {
            if !client_model.is_dir() {
                log::warn!("缺少客户 ASR 模型目录: {}；关闭客户流", client_model.display());
                return None;
            }
            Some(StreamConfig {
                engine_kind: client_engine,
                model_dir: client_model.clone(),
                input,
                speaker_id: 1,
                speaker_label: "客户".into(),
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

        let mut plugins: Vec<Arc<dyn AnalyzerPlugin>> = Vec::new();
        if scene.term_enabled && snapshot.plugins.term_explainer.enabled {
            plugins.push(Arc::new(TermExplainerPlugin::new(
                snapshot.plugins.term_explainer.cooldown_seconds as f64,
            )));
        }
        if scene.translation_enabled && snapshot.plugins.translator.enabled {
            plugins.push(Arc::new(TranslatorPlugin::new()));
        }
        if scene.brief_enabled && snapshot.plugins.brief_retriever.enabled && kb.is_some() {
            plugins.push(Arc::new(BriefRetrieverPlugin::new(
                snapshot.plugins.brief_retriever.cooldown_seconds as f64,
                0.05,
            )));
        }

        let speaker = if scene.speaker_enabled {
            let spk_model = model_dir.join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx");
            if spk_model.is_file() {
                let owner = speaker::load_owner_embedding(self.config.data_dir());
                Some(SpeakerConfig {
                    model: spk_model,
                    owner_embedding: owner,
                    threshold: DEFAULT_THRESHOLD,
                })
            } else {
                None
            }
        } else {
            None
        };

        Ok(LivePipelineConfig {
            vad_model,
            chunk_ms: 100,
            vad: scene.to_vad_config(),
            denoise: scene.to_denoise_config(),
            asr_threads: 4,
            user: StreamConfig {
                engine_kind: user_engine,
                model_dir: user_model,
                input: req.user_input.clone(),
                speaker_id: 0,
                speaker_label: req.user_label.clone().unwrap_or_else(|| "我".into()),
            },
            client,
            plugins,
            plugin_ctx: PluginContext {
                kb,
                llm: Self::build_llm(&self.config),
            },
            recording_dir,
            runtime: Arc::new(RuntimeParams::with_noise_level(req.noise_level)),
            speaker,
            engine_pool: Some(self.engines.clone()),
            min_commit_ms: scene.min_segment_ms,
        })
    }

    /// 启动监听。`on_event` 由适配器提供（IPC emit / WS broadcast / 打印）。
    pub fn start(&self, req: StartListen, on_event: EventSink) -> Result<RunningListen> {
        let cfg = self.build_live_config(&req)?;
        let stats = Arc::new(Mutex::new(Vec::new()));
        let texts = Arc::new(Mutex::new(Vec::new()));
        let persist = req.persist;
        let sessions = if persist { self.sessions.clone() } else { None };

        let session_id = if let Some(store) = &sessions {
            let now = unix_secs();
            Some(store.start_session(now)?)
        } else {
            None
        };

        let sink_sessions = sessions.clone();
        let sink_sid = session_id;
        let sink_stats = stats.clone();
        let sink_texts = texts.clone();
        let sink: EventSink = Arc::new(move |ev: DomainEvent| {
            persist_domain_event(
                sink_sessions.as_deref(),
                sink_sid,
                &sink_stats,
                &sink_texts,
                &ev,
            );
            on_event(ev);
        });

        let mut runtime = SessionRuntime::new(cfg);
        if let Err(e) = runtime.start(sink) {
            if let (Some(store), Some(sid)) = (&sessions, session_id) {
                let _ = store.end_session(sid, unix_secs());
            }
            return Err(e);
        }
        Ok(RunningListen {
            runtime,
            session_id,
            stats,
            texts,
        })
    }

    /// 停止管道、评估质量、触发 webhook。
    pub fn finish(&self, mut running: RunningListen) -> Result<Option<i64>> {
        running.runtime.stop();
        let Some(sid) = running.session_id else {
            return Ok(None);
        };
        let Some(store) = &self.sessions else {
            return Ok(Some(sid));
        };
        let now = unix_secs();
        let _ = store.end_session(sid, now);
        let stats = running.stats.lock().unwrap().clone();
        let texts = running.texts.lock().unwrap().clone();
        if !stats.is_empty() {
            let snapshot = self.config.snapshot();
            let mut params = talksage_session::QualityParams::from_config(&snapshot.quality);
            params.auto_detect = snapshot.scene.effective().noise_auto_detect;
            let meta = talksage_session::SessionMeta::evaluate(stats, &texts, now, &params);
            if let Err(e) = store.set_session_meta(sid, &meta) {
                log::warn!("保存会话元数据失败: {e}");
            }
            log::info!(
                "会话 #{sid} 质量评估: {}（时长 {}s，语音占比 {:.0}%，文本噪音 {:.2}，跳过下游分析={}）",
                meta.quality_label(),
                meta.duration_ms / 1000,
                meta.speech_ratio * 100.0,
                meta.text_noise,
                meta.skipped_analysis,
            );
        }
        let wh_cfg = self.config.snapshot().webhooks;
        if wh_cfg.enabled && !wh_cfg.urls.is_empty() {
            let sessions = store.clone();
            std::thread::spawn(move || {
                if let Ok(detail) = sessions.get_session(sid) {
                    let results = talksage_session::trigger_meeting_webhooks(&detail, &wh_cfg);
                    for r in &results {
                        log::info!(
                            "webhook {}: {}（{}）",
                            if r.ok { "成功" } else { "失败" },
                            r.url,
                            r.message
                        );
                    }
                }
            });
        }
        Ok(Some(sid))
    }
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn persist_domain_event(
    sessions: Option<&SessionStore>,
    session_id: Option<i64>,
    stats: &Mutex<Vec<StreamMeta>>,
    texts: &Mutex<Vec<String>>,
    ev: &DomainEvent,
) {
    if let (Some(store), Some(sid)) = (sessions, session_id) {
        match ev {
            DomainEvent::Segment {
                text,
                is_partial: false,
                speaker_id,
                speaker_label,
                ts_ms,
                duration_ms,
                rms,
                ..
            } => {
                if let Ok(mut t) = texts.lock() {
                    t.push(text.clone());
                }
                let _ = store.add_segment(
                    sid,
                    &TranscriptSegment {
                        speaker_id: *speaker_id,
                        speaker_label: speaker_label.clone(),
                        text: text.clone(),
                        is_partial: false,
                        ts_ms: *ts_ms,
                        duration_ms: *duration_ms,
                        rms: *rms,
                    },
                );
            }
            DomainEvent::Term {
                status: ResultStatus::Final,
                content,
                ..
            } => {
                let _ = store.add_term(sid, content);
            }
            DomainEvent::Translation { content, .. } => {
                let _ = store.add_translation(sid, "translate", content);
            }
            _ => {}
        }
    }
    if let DomainEvent::SessionStats {
        speaker_label,
        total_ms,
        speech_ms,
        final_segments,
        avg_rms,
        max_rms,
        non_speech_avg_rms,
        recording,
        vad_preset,
        vad_threshold,
        words,
        questions,
        ..
    } = ev
    {
        if let Ok(mut sm) = stats.lock() {
            sm.push(StreamMeta {
                speaker_label: speaker_label.clone(),
                total_ms: *total_ms,
                speech_ms: *speech_ms,
                final_segments: *final_segments,
                avg_rms: *avg_rms,
                max_rms: *max_rms,
                non_speech_avg_rms: *non_speech_avg_rms,
                recording: recording.clone(),
                vad_preset: vad_preset.clone(),
                vad_threshold: *vad_threshold,
                words: *words,
                questions: *questions,
            });
        }
    }
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
        let store = svc.sessions().unwrap();
        let sid = store.start_session(1).unwrap();
        let stats = Mutex::new(Vec::new());
        let texts = Mutex::new(Vec::new());
        persist_domain_event(
            Some(store.as_ref()),
            Some(sid),
            &stats,
            &texts,
            &DomainEvent::Segment {
                speaker_id: 0,
                speaker_label: "我".into(),
                text: "草稿".into(),
                is_partial: true,
                ts_ms: 1,
                duration_ms: 0,
                rms: 0.0,
                revision: 0,
                start_sample: 0,
                end_sample: 0,
            },
        );
        persist_domain_event(
            Some(store.as_ref()),
            Some(sid),
            &stats,
            &texts,
            &DomainEvent::Segment {
                speaker_id: 0,
                speaker_label: "我".into(),
                text: "定稿".into(),
                is_partial: false,
                ts_ms: 2,
                duration_ms: 400,
                rms: 0.1,
                revision: 0,
                start_sample: 0,
                end_sample: 6400,
            },
        );
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
