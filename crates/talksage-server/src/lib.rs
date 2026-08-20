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

use talksage_asr::{EngineKind, EnginePool};
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
    /// ASR 引擎池（监听 + OpenAI 兼容转写 API 共用，热启动复用）。
    pub engine_pool: Arc<EnginePool>,
}

/// 启动 headless 服务（阻塞运行）。
pub async fn run(host: &str, port: u16, token: &str, web_dist: &PathBuf) -> Result<()> {
    let _log_guard = talksage_logging::init(None);
    log::info!("headless 服务启动，host={host} port={port} token={}", if token.is_empty() { "none" } else { "set" });
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
        engine_pool: EnginePool::new(),
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
        .route("/config", get(get_config_api).post(save_config_api))
        .route("/sessions", get(list_sessions_api))
        .route("/search", get(search_api))
        .route("/session/{id}", get(get_session_api).delete(delete_session_api))
        .route("/templates", get(list_templates_api))
        .route("/session/{id}/notes", axum::routing::post(generate_notes_api))
        .route("/session/{id}/trio-notes", axum::routing::post(generate_trio_notes_api))
        .route("/logs", get(read_logs_api))
        .route("/listen/start", axum::routing::post(start_listen_api))
        .route("/listen/stop", axum::routing::post(stop_listen_api))
        .route("/noise_level", axum::routing::post(set_noise_level_api))
        .route("/voiceprint/status", axum::routing::get(voiceprint_status_api))
        .route("/voiceprint/enroll", axum::routing::post(voiceprint_enroll_api))
        .route("/voiceprint/remove", axum::routing::post(voiceprint_remove_api))
        .route("/recordings/{filename}", axum::routing::get(get_recording_api))
        .route("/ws", get(ws_handler))
        .with_state(state.clone());

    // OpenAI 兼容 API（/v1/*）：本地转写对接既有生态（whisper 客户端/脚本）。
    let v1 = Router::new()
        .route("/models", get(models_api))
        .route("/audio/transcriptions", axum::routing::post(transcribe_api))
        .with_state(state.clone());

    Router::new()
        .nest("/api", api)
        .nest("/v1", v1)
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

/// OpenAI 兼容端点鉴权：接受 `X-Talksage-Token` 或标准 `Authorization: Bearer <token>`。
fn token_ok_v1(state: &ServerState, headers: &axum::http::HeaderMap) -> bool {
    if state.token.is_empty() {
        return true;
    }
    headers
        .get("x-talksage-token")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
        })
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

/// 保存配置（设置面板提交）。
async fn save_config_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    Json(updates): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    match state
        .config
        .update(|c| {
            apply_config_updates(c, &updates);
        }) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 应用配置更新（与 Tauri 侧共享逻辑）。
fn apply_config_updates(c: &mut talksage_config::Config, updates: &serde_json::Value) {
    if let Some(llm) = updates.get("llm") {
        if let Some(default) = llm.get("default").and_then(|v| v.as_str()) {
            c.llm.default = default.to_string();
        }
        if let Some(providers) = llm.get("providers").and_then(|v| v.as_object()) {
            for (name, p) in providers {
                let entry = c.llm.providers.entry(name.clone()).or_default();
                if let Some(k) = p.get("api_key").and_then(|v| v.as_str()) {
                    entry.api_key = k.to_string();
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
        if let Some(t) = plugins.get("term_explainer") {
            if let Some(e) = t.get("enabled").and_then(|v| v.as_bool()) {
                c.plugins.term_explainer.enabled = e;
            }
        }
        if let Some(t) = plugins.get("translator") {
            if let Some(e) = t.get("enabled").and_then(|v| v.as_bool()) {
                c.plugins.translator.enabled = e;
            }
        }
        if let Some(t) = plugins.get("brief_retriever") {
            if let Some(e) = t.get("enabled").and_then(|v| v.as_bool()) {
                c.plugins.brief_retriever.enabled = e;
            }
        }
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
        if let Some(e) = asr.get("client_engine").and_then(|v| v.as_str()) {
            c.asr.client_engine = e.to_string();
        }
        if let Some(e) = asr.get("user_engine").and_then(|v| v.as_str()) {
            c.asr.user_engine = e.to_string();
        }
        if let Some(b) = asr.get("backend").and_then(|v| v.as_str()) {
            c.asr.backend = b.to_string();
        }
    }
    if let Some(audio) = updates.get("audio") {
        if let Some(vad) = audio.get("vad") {
            if let Some(p) = vad.get("preset").and_then(|v| v.as_str()) {
                c.audio.vad.preset = match p {
                    "sensitive" => talksage_config::VadPreset::Sensitive,
                    "strict" => talksage_config::VadPreset::Strict,
                    _ => talksage_config::VadPreset::Standard,
                };
            }
        }
        if let Some(d) = audio.get("denoise") {
            if let Some(e) = d.get("enabled").and_then(|v| v.as_bool()) {
                c.audio.denoise.enabled = e;
            }
            if let Some(h) = d.get("highpass").and_then(|v| v.as_bool()) {
                c.audio.denoise.highpass = h;
            }
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

/// 删除会话（含段/术语/翻译）。
async fn delete_session_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    match state.sessions.delete_session(id) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
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

#[derive(Deserialize)]
struct TrioBody {
    #[serde(default)]
    meeting_name: Option<String>,
    #[serde(default)]
    meeting_description: Option<String>,
}

/// 读取最近日志（调试窗口用）。
async fn read_logs_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let dir = talksage_logging::log_dir(Some(&state.config.data_dir().to_path_buf()));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (StatusCode::OK, Json(serde_json::json!({ "logs": "" }))).into_response();
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("talksage.log"))
        .collect();
    if files.is_empty() {
        return (StatusCode::OK, Json(serde_json::json!({ "logs": "" }))).into_response();
    }
    files.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH));
    let Some(latest) = files.last() else {
        return (StatusCode::OK, Json(serde_json::json!({ "logs": "" }))).into_response();
    };
    match std::fs::read_to_string(latest.path()) {
        Ok(content) => {
            let tail: Vec<&str> = content.lines().rev().take(200).collect();
            let joined = tail.iter().rev().copied().collect::<Vec<_>>().join("\n");
            (StatusCode::OK, Json(serde_json::json!({ "logs": joined }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
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

/// 三段式智能纪要（概述 / 归属要点 / 行动项；借鉴 Call.md summary-generator）。
async fn generate_trio_notes_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Json(body): Json<TrioBody>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let Some(llm) = build_llm(&state.config) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未配置 LLM" }))).into_response();
    };
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    let gen = talksage_notes::TrioGenerator::new(llm);
    match gen.generate(&detail.segments, body.meeting_name.as_deref(), body.meeting_description.as_deref()) {
        Ok(trio) => {
            let json = serde_json::to_value(&trio).unwrap_or_default();
            let _ = state.sessions.set_trio(id, &json.to_string());
            (StatusCode::OK, Json(json)).into_response()
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
    let cfg = match build_pipeline_config(&state.config, Some(state.engine_pool.clone())) {
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
                    DomainEvent::Segment { text, is_partial: false, speaker_id, speaker_label, ts_ms, duration_ms, rms, .. } => {
                        let _ = sessions.add_segment(
                            sid,
                            &talksage_core::TranscriptSegment {
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

/// 实时调节噪音电平阈值（headless 版）。
async fn set_noise_level_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let level: f32 = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("level").and_then(|l| l.as_f64()).map(|l| l as f32))
        .unwrap_or(0.0);
    match state.pipeline.lock().unwrap().as_ref() {
        Some(p) => {
            p.set_noise_level(level);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true, "level": p.noise_level() }))).into_response()
        }
        None => (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "not listening" }))).into_response(),
    }
}

async fn ws_handler(State(state): State<ServerState>, headers: axum::http::HeaderMap, ws: WebSocketUpgrade) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Html("unauthorized")).into_response();
    }
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

/// 说话人声纹状态（headless 版）。
async fn voiceprint_status_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    // 模型路径探测（与 tauri 一致：models/wespeaker）
    let model_ok = {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(base) = exe.parent() {
                candidates.push(base.join("../../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
                candidates.push(base.join("../../../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
            }
        }
        candidates.push(std::path::PathBuf::from("models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
        candidates.push(std::path::PathBuf::from("../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
        candidates.into_iter().any(|p| p.is_file())
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "model_available": model_ok,
            "enrolled": talksage_pipeline::speaker::owner_enrolled(state.config.data_dir()),
        })),
    )
        .into_response()
}

/// 注册主人声音（headless 版：服务器本机麦克风录制）。
async fn voiceprint_enroll_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let seconds: u32 = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("seconds").and_then(|s| s.as_u64()))
        .unwrap_or(6)
        .max(3) as u32;
    let model = {
        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(base) = exe.parent() {
                candidates.push(base.join("../../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
                candidates.push(base.join("../../../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
            }
        }
        candidates.push(std::path::PathBuf::from("models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
        candidates.push(std::path::PathBuf::from("../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"));
        candidates.into_iter().find(|p| p.is_file())
    };
    let Some(model) = model else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "missing speaker model" }))).into_response();
    };
    let data_dir = state.config.data_dir().to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};
        let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
            model: Some(model.to_string_lossy().into()),
            num_threads: 1,
            debug: false,
            provider: Some("cpu".into()),
        })
        .ok_or_else(|| "声纹模型加载失败".to_string())?;
        let (mut hub, rx) = talksage_audio::AudioHub::new(100);
        hub.start(None).map_err(|e| format!("启动麦克风失败: {e}"))?;
        let mut audio: Vec<f32> = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds as u64);
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(c) => audio.extend_from_slice(&c),
                Err(_) => {}
            }
        }
        hub.stop();
        let stream = extractor.create_stream().ok_or("创建声纹流失败")?;
        stream.accept_waveform(16000, &audio);
        if !extractor.is_ready(&stream) {
            return Err("采集音频太短".to_string());
        }
        let emb = extractor.compute(&stream).ok_or("声纹提取失败")?;
        talksage_pipeline::speaker::save_owner_embedding(&data_dir, &emb)
            .map_err(|e| format!("保存声纹失败: {e}"))?;
        Ok::<Vec<f32>, String>(emb)
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));
    match result {
        Ok(emb) => (StatusCode::OK, Json(serde_json::json!({ "ok": true, "dim": emb.len() }))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))).into_response(),
    }
}

/// 删除主人声纹（headless 版）。
async fn voiceprint_remove_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let _ = talksage_pipeline::speaker::remove_owner_embedding(state.config.data_dir());
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

/// 提供录音文件（历史会话回放）：`GET /api/recordings/<文件名>`。
/// 仅允许录音目录内的文件（文件名白名单，防目录穿越）。
async fn get_recording_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(filename): AxumPath<String>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    // 解析录音目录（与监听时一致）
    let snapshot = state.config.snapshot();
    let rec_dir = snapshot.recording.resolve_dir(state.config.data_dir());
    // 防目录穿越：仅允许文件名（不含分隔符）
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "bad filename" }))).into_response();
    }
    let path = rec_dir.join(&filename);
    if !path.is_file() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not found" }))).into_response();
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "audio/wav".to_string()),
                (axum::http::header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
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

fn build_pipeline_config(config: &ConfigManager, engine_pool: Option<Arc<EnginePool>>) -> Result<LivePipelineConfig> {
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
        vad: snapshot.audio.vad.clone(),
        denoise: snapshot.audio.denoise.clone(),
        asr_threads: 4,
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
        // headless 服务端也支持录音（默认随配置）
        recording_dir: if snapshot.recording.enabled {
            let dir = snapshot.recording.resolve_dir(config.data_dir());
            let _ = std::fs::create_dir_all(&dir);
            Some(dir)
        } else {
            None
        },
        runtime: Arc::new(talksage_pipeline::RuntimeParams::default()),
        // 说话人识别（headless 同启用）
        speaker: {
            let spk_model = model_dir.join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx");
            if spk_model.is_file() {
                let owner = talksage_pipeline::speaker::load_owner_embedding(config.data_dir());
                Some(talksage_pipeline::SpeakerConfig {
                    model: spk_model,
                    owner_embedding: owner,
                    threshold: talksage_pipeline::speaker::DEFAULT_THRESHOLD,
                })
            } else {
                None
            }
        },
        engine_pool,
        // 最短提交时长：短段丢弃（噪音短段抑制）
        min_commit_ms: snapshot.audio.min_segment_ms.unwrap_or(0),
    })
}

// ── OpenAI 兼容 API（/v1/*）──────────────────────────────
// 目标：既有 OpenAI 生态客户端/脚本（whisper 类工具、curl）可直接指向本服务
// 做本地转写，鉴权用标准 `Authorization: Bearer <token>`。

/// `GET /v1/models`：列出可用转写引擎。
async fn models_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok_v1(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let data: Vec<serde_json::Value> = [EngineKind::ParaformerZh, EngineKind::ZipformerEn]
        .iter()
        .map(|k| serde_json::json!({ "id": k.display_name(), "object": "model", "owned_by": "talksage" }))
        .collect();
    Json(serde_json::json!({ "object": "list", "data": data })).into_response()
}

/// 解析引擎路径（VAD + ASR 模型目录）。
fn engine_paths(kind: EngineKind) -> Result<(PathBuf, PathBuf)> {
    let model_dir = resolve_models_dir().ok_or_else(|| anyhow!("未找到 models/ 目录"))?;
    let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
    let engine_dir = model_dir.join(match kind {
        EngineKind::ParaformerZh => "sherpa-onnx-streaming-paraformer-zh",
        EngineKind::ZipformerEn => "sherpa-onnx-streaming-zipformer-en-2023-06-26",
    });
    if !vad_model.is_file() {
        return Err(anyhow!("缺少 VAD 模型: {}", vad_model.display()));
    }
    if !engine_dir.is_dir() {
        return Err(anyhow!("缺少 ASR 模型目录: {}", engine_dir.display()));
    }
    Ok((vad_model, engine_dir))
}

/// 读取任意采样率 PCM wav → 重采样到 16k → 写 16k mono PCM16 文件（管道要求 16k）。
fn normalize_wav(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    let (sr, samples) = talksage_audio::wav::read_wav(src)?;
    let samples = if sr != talksage_audio::TARGET_SAMPLE_RATE {
        talksage_audio::resample_linear(&samples, sr, talksage_audio::TARGET_SAMPLE_RATE)
    } else {
        samples
    };
    let mut rec = talksage_audio::wav::WavRecorder::create(dst, talksage_audio::TARGET_SAMPLE_RATE)?;
    rec.write(&samples)?;
    rec.finish()?;
    Ok(())
}

/// `POST /v1/audio/transcriptions`：multipart（file + model + response_format + language）。
/// 复用实时监听同一条 VAD+ASR 管道（引擎池热启动），返回 OpenAI 兼容 JSON。
async fn transcribe_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    if !token_ok_v1(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }

    // 解析 multipart 字段（OpenAI 兼容：file / model / response_format / language）
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut model = String::new();
    let mut response_format = String::from("json");
    while let Some(field) = match multipart.next_field().await {
        Ok(f) => f,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("multipart 解析失败: {e}") }))).into_response();
        }
    } {
        match field.name().unwrap_or("") {
            "file" => {
                file_bytes = match field.bytes().await {
                    Ok(b) => Some(b.to_vec()),
                    Err(e) => {
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("读取 file 失败: {e}") }))).into_response();
                    }
                };
            }
            "model" => {
                model = field.text().await.unwrap_or_default();
            }
            "response_format" => {
                response_format = field.text().await.unwrap_or_default();
            }
            _ => {
                let _ = field.bytes().await; // language 等字段暂忽略（自动按模型语言）
            }
        }
    }
    let file_bytes = match file_bytes {
        Some(b) if !b.is_empty() => b,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "缺少 file 字段（PCM wav 音频）" }))).into_response();
        }
    };
    if !["json", "text", "verbose_json"].contains(&response_format.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("不支持的 response_format: {response_format}（json|text|verbose_json）") })),
        )
            .into_response();
    }

    // 引擎：model 字段 → EngineKind；缺省用配置的 user_engine
    let kind = EngineKind::from_name(model.trim())
        .unwrap_or_else(|| EngineKind::from_name(&state.config.snapshot().asr.user_engine).unwrap_or(EngineKind::ParaformerZh));
    let (vad_model, engine_dir) = match engine_paths(kind) {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({ "error": e.to_string() }))).into_response();
        }
    };

    // 落盘 → 归一化 16k mono PCM16（文件名带自增序号，避免并发请求时间戳碰撞）
    let tmp_dir = state.config.data_dir().join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_wav = tmp_dir.join(format!("transcribe-{now}-{seq}.wav"));
    let norm_wav = tmp_dir.join(format!("transcribe-{now}-{seq}-16k.wav"));
    let normalized = std::fs::write(&tmp_wav, &file_bytes)
        .map_err(|e| anyhow!("写入上传音频失败: {e}"))
        .and_then(|_| normalize_wav(&tmp_wav, &norm_wav));
    if normalized.is_err() {
        let _ = std::fs::remove_file(&tmp_wav);
        let _ = std::fs::remove_file(&norm_wav);
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "音频解析失败（仅支持 PCM wav，任意采样率，自动重采样到 16k）" })),
        )
            .into_response();
    }
    let audio_secs = talksage_audio::wav::read_wav(&norm_wav)
        .map(|(sr, s)| s.len() as f64 / sr as f64)
        .unwrap_or(0.0);

    // 阻塞转写（模型加载/推理）放到 blocking 线程池
    let pool = state.engine_pool.clone();
    let norm_wav_in = norm_wav.clone();
    let result = tokio::task::spawn_blocking(move || {
        talksage_pipeline::offline::transcribe_file(Some(&pool), kind, &engine_dir, &vad_model, &norm_wav_in)
    })
    .await;
    let _ = std::fs::remove_file(&tmp_wav);
    let _ = std::fs::remove_file(&norm_wav);
    let tr = match result {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("转写失败: {e}") }))).into_response();
        }
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("转写线程异常: {e}") }))).into_response();
        }
    };

    match response_format.as_str() {
        "text" => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("text/plain; charset=utf-8"))],
            tr.text,
        )
            .into_response(),
        "verbose_json" => {
            // 管道 final 段的 ts_ms 是段结束时刻（epoch）；换算成相对音频起点的时间轴
            let base = tr
                .segments
                .first()
                .map(|s| s.ts_ms.saturating_sub(s.duration_ms))
                .unwrap_or(0);
            let segments: Vec<serde_json::Value> = tr
                .segments
                .iter()
                .map(|s| {
                    let start_ms = s.ts_ms.saturating_sub(s.duration_ms).saturating_sub(base);
                    let end_ms = s.ts_ms.saturating_sub(base);
                    serde_json::json!({
                        "text": s.text,
                        "start": start_ms as f64 / 1000.0,
                        "end": end_ms as f64 / 1000.0,
                        "start_ms": start_ms,
                        "duration_ms": s.duration_ms,
                    })
                })
                .collect();
            let rtf = if audio_secs > 0.0 { tr.elapsed_ms / 1000.0 / audio_secs } else { 0.0 };
            Json(serde_json::json!({
                "text": tr.text,
                "duration": audio_secs,
                "elapsed_ms": tr.elapsed_ms,
                "rtf": rtf,
                "first_latency_ms": tr.first_latency_ms,
                "segments": segments,
            }))
            .into_response()
        }
        _ => Json(serde_json::json!({ "text": tr.text })).into_response(),
    }
}
