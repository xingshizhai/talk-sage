//! TalkSage v2 headless 服务（M4）：axum HTTP + WebSocket + SPA 静态托管。
//!
//! 形态：`talksage serve --host 127.0.0.1 --port 8080 [--token xxx]`
//! - `/api/*`：健康/配置/会话/纪要/监听控制
//! - `/ws`：领域事件广播（转写/术语/翻译…，前端订阅）
//! - 静态托管 `web/dist`（SPA），浏览器访问即 UI
//!
//! 音频采集仍在服务端本机（麦克风/回环，复用 talksage-pipeline）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::broadcast;

use talksage_asr::EngineKind;
use talksage_config::ConfigManager;
use talksage_core::DomainEvent;
use talksage_llm::{LLMProvider, OpenAICompatProvider};
use talksage_notes::NotesGenerator;
use talksage_pipeline::{AudioInput, LivePipeline, LivePipelineConfig, StreamConfig};
use talksage_plugins::{brief_retriever::BriefRetrieverPlugin, term_explainer::TermExplainerPlugin, translator::TranslatorPlugin, PluginContext};
use talksage_session::SessionStore;

/// 共享服务状态。
#[derive(Clone)]
pub struct ServerState {
    pub config: Arc<ConfigManager>,
    pub sessions: Arc<SessionStore>,
    /// 领域事件广播（前端 /ws 订阅）。
    pub events: broadcast::Sender<DomainEvent>,
    /// 当前监听管道。
    pub pipeline: Arc<Mutex<Option<LivePipeline>>>,
    /// 当前会话 id（监听期间）。
    pub current_session: Arc<Mutex<Option<i64>>>,
    /// 可选鉴权 token（空 = 不鉴权）。
    pub token: String,
}

/// 启动 headless 服务（阻塞运行）。
pub async fn run(host: &str, port: u16, token: &str, web_dist: &PathBuf) -> Result<()> {
    let config = ConfigManager::load(None, None).map_err(|e| anyhow!("配置加载失败: {e}"))?;
    let sessions = Arc::new(
        SessionStore::open(&config.data_dir().join("sessions.db").to_string_lossy()).map_err(|e| anyhow!("会话库: {e}"))?,
    );
    let (tx, _rx) = broadcast::channel::<DomainEvent>(256);
    let state = ServerState {
        config: Arc::new(config),
        sessions,
        events: tx,
        pipeline: Arc::new(Mutex::new(None)),
        current_session: Arc::new(Mutex::new(None)),
        token: token.to_string(),
    };

    let app = build_router(state, web_dist);
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow!("绑定 {addr} 失败: {e}"))?;
    println!("TalkSage headless 服务: http://{addr}");
    if !token.is_empty() {
        println!("鉴权 token 已启用（请求头 X-Talksage-Token）");
    }
    axum::serve(listener, app).await.map_err(|e| anyhow!("服务退出: {e}"))
}

/// 构建路由（供测试复用）。
pub fn build_router(state: ServerState, web_dist: &PathBuf) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/config", get(get_config_api))
        .route("/sessions", get(list_sessions_api))
        .route("/search", get(search_api))
        .route("/session/{id}", get(get_session_api))
        .route("/templates", get(list_templates_api))
        .route("/session/{id}/notes", axum::routing::post(generate_notes_api))
        .route("/listen/start", axum::routing::post(start_listen_api))
        .route("/listen/stop", axum::routing::post(stop_listen_api))
        .route("/ws", get(ws_handler))
        .with_state(state);

    Router::new()
        .nest("/api", api)
        .fallback_service(tower_http::services::ServeDir::new(web_dist).append_index_html_on_directories(true))
}

// ── 鉴权辅助 ──────────────────────────────────────────────

fn token_ok(state: &ServerState, headers: &axum::http::HeaderMap) -> bool {
    if state.token.is_empty() {
        return true;
    }
    headers
        .get("x-talksage-token")
        .and_then(|v| v.to_str().ok())
        .map(|t| t == state.token)
        .unwrap_or(false)
}

// ── API handlers ──────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "version": talksage_core::VERSION }))
}

async fn get_config_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let cfg = state.config.snapshot();
    let body = serde_json::json!({
        "asr": { "client_engine": cfg.asr.client_engine, "user_engine": cfg.asr.user_engine, "backend": cfg.asr.backend },
        "plugins": {
            "term_explainer": cfg.plugins.term_explainer,
            "translator": cfg.plugins.translator,
            "brief_retriever": cfg.plugins.brief_retriever,
        },
        "server": { "host": cfg.server.host, "port": cfg.server.port },
    });
    (StatusCode::OK, Json(body)).into_response()
}

async fn list_sessions_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    match state.sessions.list_sessions(100) {
        Ok(list) => (StatusCode::OK, Json(list)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
}

async fn search_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    match state.sessions.search(&params.q, 50) {
        Ok(hits) => (StatusCode::OK, Json(hits)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn get_session_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    match state.sessions.get_session(id) {
        Ok(detail) => (StatusCode::OK, Json(detail)).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn list_templates_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let templates: Vec<_> = talksage_notes::builtin_templates()
        .into_iter()
        .map(|t| serde_json::json!({ "id": t.id, "name": t.name, "description": t.description }))
        .collect();
    (StatusCode::OK, Json(templates)).into_response()
}

#[derive(Deserialize)]
struct NotesBody {
    template_id: String,
}

async fn generate_notes_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Json(body): Json<NotesBody>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let Some(llm) = build_llm(&state.config) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未配置 LLM" }))).into_response();
    };
    let Some(template) = talksage_notes::get_template(&body.template_id) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未知模板" }))).into_response();
    };
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    let gen = NotesGenerator::new(llm);
    match gen.generate(&detail.segments, &detail.terms, &detail.translations, &template) {
        Ok(notes) => {
            let _ = state.sessions.set_notes(id, &notes);
            (StatusCode::OK, Json(serde_json::json!({ "notes": notes }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn start_listen_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let mut guard = state.pipeline.lock().unwrap();
    if guard.is_some() {
        return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "已在监听中" }))).into_response();
    }
    let cfg = match build_pipeline_config(&state.config) {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    };
    let events = state.events.clone();
    let current_session = state.current_session.clone();
    let sessions = state.sessions.clone();
    let sink: Arc<dyn Fn(DomainEvent) + Send + Sync> = Arc::new(move |ev| {
        // 落库
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
                    DomainEvent::Term { status: talksage_core::ResultStatus::Final, content, .. } => {
                        let _ = sessions.add_term(sid, content);
                    }
                    DomainEvent::Translation { content, .. } => {
                        let _ = sessions.add_translation(sid, "translate", content);
                    }
                    _ => {}
                }
            }
        }
        let _ = events.send(ev);
    });
    let mut pipeline = LivePipeline::new(cfg);
    if let Err(e) = pipeline.start(sink) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Ok(sid) = state.sessions.start_session(now) {
        *state.current_session.lock().unwrap() = Some(sid);
    }
    *guard = Some(pipeline);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

async fn stop_listen_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    if let Some(mut p) = state.pipeline.lock().unwrap().take() {
        p.stop();
    }
    if let Some(sid) = state.current_session.lock().unwrap().take() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let _ = state.sessions.end_session(sid, now);
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

async fn ws_handler(State(state): State<ServerState>, headers: axum::http::HeaderMap, ws: WebSocketUpgrade) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Html("unauthorized")).into_response();
    }
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: ServerState) {
    let mut rx = state.events.subscribe();
    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(ev) => {
                        if let Ok(text) = serde_json::to_string(&ev) {
                            if socket.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // 忽略客户端消息
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

// ── 装配辅助（与 Tauri 侧等价） ────────────────────────────

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

fn resolve_models_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
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

fn build_pipeline_config(config: &ConfigManager) -> Result<LivePipelineConfig> {
    let model_dir = resolve_models_dir().ok_or_else(|| anyhow!("未找到 models/ 目录"))?;
    let user_engine = EngineKind::from_name(&config.snapshot().asr.user_engine).unwrap_or(EngineKind::ParaformerZh);
    let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
    let user_model = model_dir.join(match user_engine {
        EngineKind::ParaformerZh => "sherpa-onnx-streaming-paraformer-zh",
        EngineKind::ZipformerEn => "sherpa-onnx-streaming-zipformer-en-2023-06-26",
    });
    if !vad_model.is_file() {
        return Err(anyhow!("缺少 VAD 模型: {}", vad_model.display()));
    }
    if !user_model.is_dir() {
        return Err(anyhow!("缺少 ASR 模型目录: {}", user_model.display()));
    }
    // 插件装配
    let snapshot = config.snapshot();
    let llm = build_llm(config);
    let kb = {
        let kb_cfg = &snapshot.knowledge_base;
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
    let mut plugins: Vec<Arc<dyn talksage_plugins::AnalyzerPlugin>> = Vec::new();
    if snapshot.plugins.term_explainer.enabled {
        plugins.push(Arc::new(TermExplainerPlugin::new(snapshot.plugins.term_explainer.cooldown_seconds as f64)));
    }
    if snapshot.plugins.translator.enabled {
        plugins.push(Arc::new(TranslatorPlugin::new()));
    }
    if snapshot.plugins.brief_retriever.enabled && kb.is_some() {
        plugins.push(Arc::new(BriefRetrieverPlugin::new(snapshot.plugins.brief_retriever.cooldown_seconds as f64, 0.05)));
    }
    Ok(LivePipelineConfig {
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
        client: None, // 回环接入后续（headless 场景音频在服务端本机）
        plugins,
        plugin_ctx: PluginContext { kb, llm },
    })
}
