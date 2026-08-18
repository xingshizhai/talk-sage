//! TalkSage v2 — Tauri 适配器。
//!
//! 职责：把 Rust 核心域暴露给前端（React SPA）：
//!   - command：get_version / get_config / ping / start_listen / stop_listen
//!   - event：领域事件推送（talksage://event 通道，含实时转写）
//!
//! 这是"可插拔传输适配器"之一；删除本 crate 即回到纯 headless（M4 预留 axum 适配器）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{Emitter, Manager};
use talksage_config::ConfigManager;
use talksage_core::{DomainEvent, ResultStatus, StatusStage};
use talksage_pipeline::{AudioInput, LivePipeline, LivePipelineConfig, StreamConfig};
use talksage_asr::EngineKind;
use talksage_llm::{LLMProvider, OpenAICompatProvider};
use talksage_plugins::{brief_retriever::BriefRetrieverPlugin, term_explainer::TermExplainerPlugin, translator::TranslatorPlugin, PluginContext};
use talksage_session::SessionStore;

/// 应用状态（Tauri managed state）。
pub struct AppState {
    config: ConfigManager,
    /// 当前监听管道（None = 未监听）。
    pipeline: Mutex<Option<LivePipeline>>,
    /// 会话存储（常驻 SQLite）。
    sessions: Arc<SessionStore>,
    /// 当前会话 id（监听期间有效）。
    current_session: Arc<Mutex<Option<i64>>>,
}

/// 版本。
#[tauri::command]
fn get_version() -> String {
    talksage_core::VERSION.to_string()
}

/// 配置快照。
#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> serde_json::Value {
    serde_json::to_value(state.config.snapshot()).unwrap_or(serde_json::Value::Null)
}

/// hello-world 事件：前端 ping → 后端推送领域事件。
#[tauri::command]
fn ping(app: tauri::AppHandle) -> Result<(), String> {
    app.emit(
        "talksage://event",
        DomainEvent::Status {
            stage: StatusStage::Idle,
            message: "pong from rust".into(),
        },
    )
    .map_err(|e| e.to_string())
}

/// 开始实时监听（麦克风 → VAD → ASR → 事件推送）。
#[tauri::command]
fn start_listen(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.pipeline.lock().map_err(|_| "pipeline 锁失败".to_string())?;
    if guard.is_some() {
        return Err("已在监听中".into());
    }

    let model_dir = resolve_models_dir();
    let user_engine = EngineKind::from_name(&state.config.snapshot().asr.user_engine)
        .unwrap_or(EngineKind::ParaformerZh);
    let vad_model = model_dir
        .join("silero-vad")
        .join("silero_vad.onnx");
    let user_model = model_dir.join(match user_engine {
        EngineKind::ParaformerZh => "sherpa-onnx-streaming-paraformer-zh",
        EngineKind::ZipformerEn => "sherpa-onnx-streaming-zipformer-en-2023-06-26",
    });
    if !vad_model.is_file() {
        return Err(format!("缺少 VAD 模型: {}", vad_model.display()));
    }
    if !user_model.is_dir() {
        return Err(format!("缺少用户 ASR 模型目录: {}", user_model.display()));
    }

    let cfg = LivePipelineConfig {
        vad_model,
        chunk_ms: 100,
        min_silence_seconds: 0.5,
        user: StreamConfig {
            engine_kind: user_engine,
            model_dir: user_model,
            input: AudioInput::Mic(None),
            speaker_id: 0,
            speaker_label: "我".into(),
        },
        // 客户流：系统回环采集（视频会议中客户语音）+ 英文引擎
        client: {
            let client_model = model_dir.join("sherpa-onnx-streaming-zipformer-en-2023-06-26");
            if client_model.is_dir() {
                #[cfg(windows)]
                {
                    Some(StreamConfig {
                        engine_kind: EngineKind::ZipformerEn,
                        model_dir: client_model,
                        input: AudioInput::Loopback,
                        speaker_id: 1,
                        speaker_label: "客户".into(),
                    })
                }
                #[cfg(not(windows))]
                {
                    None
                }
            } else {
                None
            }
        },
        plugins: build_plugins(&state.config),
        plugin_ctx: build_plugin_ctx(&state.config),
    };

    let mut pipeline = LivePipeline::new(cfg);
    let sessions = state.sessions.clone();
    let current_session = state.current_session.clone();
    let sink: Arc<dyn Fn(DomainEvent) + Send + Sync> = Arc::new(move |ev: DomainEvent| {
        // 会话落库（监听期间）
        if let Ok(guard) = current_session.lock() {
            if let Some(sid) = *guard {
                match &ev {
                    DomainEvent::Segment { text, is_partial: false, speaker_id, speaker_label, ts_ms, .. } => {
                        let _ = sessions.add_segment(
                            sid,
                            &talksage_core::TranscriptSegment {
                                speaker_id: *speaker_id,
                                speaker_label: speaker_label.clone(),
                                text: text.clone(),
                                is_partial: false,
                                ts_ms: *ts_ms,
                            },
                        );
                    }
                    DomainEvent::Term { status: ResultStatus::Final, content, .. } => {
                        let _ = sessions.add_term(sid, content);
                    }
                    DomainEvent::Translation { content, .. } => {
                        let _ = sessions.add_translation(sid, "translate", content);
                    }
                    _ => {}
                }
            }
        }
        let _ = app.emit("talksage://event", ev);
    });
    pipeline.start(sink).map_err(|e| e.to_string())?;
    // 开启会话
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let sid = state
        .sessions
        .start_session(now)
        .map_err(|e| format!("开启会话失败: {e}"))?;
    *state.current_session.lock().unwrap() = Some(sid);
    *guard = Some(pipeline);
    Ok(())
}

/// 停止实时监听。
#[tauri::command]
fn stop_listen(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.pipeline.lock().map_err(|_| "pipeline 锁失败".to_string())?;
    if let Some(mut p) = guard.take() {
        p.stop();
    }
    // 结束会话
    if let Some(sid) = state.current_session.lock().unwrap().take() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let _ = state.sessions.end_session(sid, now);
    }
    Ok(())
}

/// 会话列表（历史）。
#[tauri::command]
fn list_sessions(state: tauri::State<'_, AppState>) -> Result<Vec<talksage_session::SessionRecord>, String> {
    state.sessions.list_sessions(100).map_err(|e| e.to_string())
}

/// 全文检索。
#[tauri::command]
fn search_sessions(query: String, state: tauri::State<'_, AppState>) -> Result<Vec<talksage_session::SegmentHit>, String> {
    state.sessions.search(&query, 50).map_err(|e| e.to_string())
}

/// 会话详情。
#[tauri::command]
fn get_session(session_id: i64, state: tauri::State<'_, AppState>) -> Result<talksage_session::SessionDetail, String> {
    state.sessions.get_session(session_id).map_err(|e| e.to_string())
}

/// 根据配置构建 LLM Provider（OpenAI 兼容）。
fn build_llm(config: &ConfigManager) -> Option<Arc<dyn LLMProvider>> {
    let snapshot = config.snapshot();
    let name = snapshot.llm.default.clone();
    let provider = snapshot.llm.providers.get(&name)?;
    if provider.api_key.is_empty() && name != "ollama" {
        return None;
    }
    Some(Arc::new(OpenAICompatProvider::new(
        provider.api_key.clone(),
        provider.model.clone(),
        provider.base_url.clone().unwrap_or_else(|| "https://api.deepseek.com/v1".to_string()),
    )))
}

/// 构建插件列表（按配置开关）。
fn build_plugins(config: &ConfigManager) -> Vec<Arc<dyn talksage_plugins::AnalyzerPlugin>> {
    let mut plugins: Vec<Arc<dyn talksage_plugins::AnalyzerPlugin>> = Vec::new();
    let plugins_cfg = &config.snapshot().plugins;
    if plugins_cfg.term_explainer.enabled {
        plugins.push(Arc::new(TermExplainerPlugin::new(plugins_cfg.term_explainer.cooldown_seconds as f64)));
    }
    if plugins_cfg.translator.enabled {
        plugins.push(Arc::new(TranslatorPlugin::new()));
    }
    if plugins_cfg.brief_retriever.enabled {
        plugins.push(Arc::new(BriefRetrieverPlugin::new(
            plugins_cfg.brief_retriever.cooldown_seconds as f64,
            0.05,
        )));
    }
    plugins
}

/// 构建插件上下文（知识库 + LLM）。
fn build_plugin_ctx(config: &ConfigManager) -> PluginContext {
    let llm = build_llm(config);
    let kb = {
        let kb_cfg = &config.snapshot().knowledge_base;
        if kb_cfg.enabled && !kb_cfg.folder.is_empty() {
            let mut kb = talksage_knowledge::KnowledgeBase::new();
            kb.index_folder(std::path::Path::new(&kb_cfg.folder));
            if kb.chunk_count() > 0 {
                Some(Arc::new(kb))
            } else {
                None
            }
        } else {
            None
        }
    };
    PluginContext { kb, llm }
}

/// 解析模型根目录：优先环境变量，其次相对可执行文件探测。
fn resolve_models_dir() -> PathBuf {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return p;
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(base) = exe.parent() {
            candidates.push(base.join("../../models")); // target/debug → 仓库根/models
            candidates.push(base.join("../../../models"));
        }
    }
    candidates.push(PathBuf::from("models"));
    candidates.push(PathBuf::from("../models"));
    for c in candidates {
        if c.is_dir() {
            return c;
        }
    }
    PathBuf::from("models")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = ConfigManager::load(None, None).expect("加载配置失败");
    let data_dir = config.data_dir().to_path_buf();
    let sessions = Arc::new(
        SessionStore::open(&data_dir.join("sessions.db").to_string_lossy()).expect("打开会话库失败"),
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            config,
            pipeline: Mutex::new(None),
            sessions,
            current_session: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            get_version,
            get_config,
            ping,
            start_listen,
            stop_listen,
            list_sessions,
            search_sessions,
            get_session
        ])
        .setup(move |app| {
            if let Err(e) = std::fs::create_dir_all(&data_dir) {
                eprintln!("创建数据目录失败 {}: {e}", data_dir.display());
            }
            let _ = app.emit(
                "talksage://event",
                DomainEvent::Status {
                    stage: StatusStage::Starting,
                    message: "TalkSage 已启动".into(),
                },
            );
            let _ = app.get_webview_window("main");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running TalkSage");
}
