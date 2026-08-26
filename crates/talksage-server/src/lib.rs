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
use talksage_notes::NotesGenerator;
use talksage_pipeline::{RunningListen, StartListen, TalkSageService};
use talksage_session::SessionStore;

/// 共享服务状态。
#[derive(Clone)]
pub struct ServerState {
    pub config: Arc<ConfigManager>,
    pub sessions: Arc<SessionStore>,
    /// 领域事件广播（前端 /ws 订阅）。
    pub events: broadcast::Sender<DomainEvent>,
    /// 当前监听。
    pub running: Arc<Mutex<Option<RunningListen>>>,
    /// 进行中的模型下载（引擎 id → 取消标志）。
    pub downloads: Arc<Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>>>,
    /// 可选鉴权 token（空 = 不鉴权）。
    pub token: String,
    /// 共享用例入口（装配 / 落库 / 引擎池）。
    pub service: TalkSageService,
}

/// 启动 headless 服务（阻塞运行）。
pub async fn run(host: &str, port: u16, token: &str, web_dist: &PathBuf) -> Result<()> {
    let _log_guard = talksage_logging::init(None);
    log::info!("headless 服务启动，host={host} port={port} token={}", if token.is_empty() { "none" } else { "set" });
    let config = Arc::new(ConfigManager::load(None, None).map_err(|e| anyhow!("配置加载失败: {e}"))?);
    let sessions = Arc::new(
        SessionStore::open(&config.data_dir().join("sessions.db").to_string_lossy()).map_err(|e| anyhow!("会话库: {e}"))?,
    );
    let (tx, _rx) = broadcast::channel::<DomainEvent>(256);
    let service = TalkSageService::new(config.clone(), Some(sessions.clone()), EnginePool::new());
    // 上次异常退出的残留（未完成录音 + 未结束会话），在对外服务前先收拾干净。
    service.recover_on_startup();
    let state = ServerState {
        config,
        sessions,
        events: tx,
        running: Arc::new(Mutex::new(None)),
        downloads: Arc::new(Mutex::new(std::collections::HashMap::new())),
        token: token.to_string(),
        service,
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
        .route("/plugins", get(list_plugins_api))
        .route("/plugins/status", get(plugin_status_api))
        .route("/asr/models", get(asr_models_api))
        .route("/asr/models/{engine}/download", axum::routing::post(download_model_api))
        .route("/asr/models/{engine}/download/cancel", axum::routing::post(cancel_model_download_api))
        .route("/asr/models/{engine}/remove", axum::routing::post(remove_model_api))
        .route("/sessions", get(list_sessions_api))
        .route("/search", get(search_api))
        .route("/session/{id}", get(get_session_api).delete(delete_session_api))
        .route("/templates", get(list_templates_api))
        .route("/session/{id}/notes", axum::routing::post(generate_notes_api))
        .route("/session/{id}/trio-notes", axum::routing::post(generate_trio_notes_api))
        .route("/session/{id}/export", get(export_session_api))
        .route("/session/{id}/export-text", get(export_session_text_api))
        .route("/session/{id}/export-audio", get(export_session_audio_api))
        .route("/session/{id}/highlights", axum::routing::post(generate_highlights_api))
        .route("/llm/test", axum::routing::post(test_llm_api))
        .route("/logs", get(read_logs_api))
        .route("/listen/start", axum::routing::post(start_listen_api))
        .route("/listen/stop", axum::routing::post(stop_listen_api))
        .route("/listen/pause", axum::routing::post(pause_listen_api))
        .route("/noise_level", axum::routing::post(set_noise_level_api))
        .route("/asr/gpu_status", get(gpu_status_handler))
        .route("/asr/test", axum::routing::post(test_aliyun_asr_api))
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

async fn gpu_status_handler(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let gpu = talksage_asr::GpuBackend::detect();
    let cfg = state.config.snapshot();
    let route = talksage_asr::resolve_asr_route(
        &cfg.asr.asr_mode,
        &cfg.asr.backend,
        gpu,
        talksage_asr::CloudCredentials {
            access_key_id: &cfg.asr.aliyun_access_key_id,
            access_key_secret: &cfg.asr.aliyun_access_key_secret,
            app_key: &cfg.asr.aliyun_app_key,
        },
    );
    let route_error = route.as_ref().err().map(ToString::to_string);
    if let Some(error) = &route_error {
        log::warn!(
            "ASR 状态查询发现路由不可用: physical_gpu={} runtime_backend={} mode={} configured_backend={} error={} note={}",
            talksage_asr::GpuBackend::hardware_candidate(), gpu.display_name(), cfg.asr.asr_mode,
            cfg.asr.backend, error, talksage_asr::GpuBackend::availability_note()
        );
    }
    Json(serde_json::json!({
        "backend": gpu.provider_str(),
        "display_name": gpu.display_name(),
        "hardware_candidate": talksage_asr::GpuBackend::hardware_candidate(),
        "availability_note": talksage_asr::GpuBackend::availability_note(),
        "is_accelerated": gpu.is_accelerated(),
        "effective_route": route.as_ref().ok().map(|r| r.display_name()),
        "route_error": route_error,
    }))
    .into_response()
}

/// 验证阿里云 ASR 凭据（设置页「检查」按钮）。body 可选覆盖 AccessKey/AppKey
/// （表单未保存时验证），不写配置。成功返回 token 有效期。
#[derive(serde::Deserialize)]
struct TestAliyunBody {
    #[serde(default)]
    access_key_id: Option<String>,
    #[serde(default)]
    access_key_secret: Option<String>,
    #[serde(default)]
    app_key: Option<String>,
}

async fn test_aliyun_asr_api(State(state): State<ServerState>, headers: axum::http::HeaderMap, body: axum::Json<TestAliyunBody>) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let cfg = state.config.snapshot();
    let key_id = body.access_key_id.clone().unwrap_or(cfg.asr.aliyun_access_key_id.clone());
    let key_secret = body.access_key_secret.clone().unwrap_or(cfg.asr.aliyun_access_key_secret.clone());
    let app_key = body.app_key.clone().unwrap_or(cfg.asr.aliyun_app_key.clone());
    if key_id.trim().is_empty() || key_secret.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "请先填写 AccessKey ID 和 AccessKey Secret" }))).into_response();
    }
    let key_id = key_id.trim().to_string();
    let key_secret = key_secret.trim().to_string();
    match talksage_asr::aliyun::verify_aliyun_credentials(&key_id, &key_secret).await {
        Ok(expire) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let valid_for = expire.saturating_sub(now);
            (StatusCode::OK, Json(serde_json::json!({
                "ok": true, "expire_at": expire, "valid_for_secs": valid_for, "app_key": app_key,
            }))).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("阿里云 ASR 验证失败: {e}") }))).into_response(),
    }
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "ok": true, "version": talksage_core::VERSION }))
}

async fn get_config_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let cfg = state.config.snapshot();
    let body = serde_json::json!({
        "asr": {
            "engine_en": cfg.asr.engine_en,
            "engine_zh": cfg.asr.engine_zh,
            "backend": cfg.asr.backend,
            "punct_enabled": cfg.asr.punct_enabled,
            "asr_mode": cfg.asr.asr_mode,
            "aliyun_access_key_id": cfg.asr.aliyun_access_key_id,
            "aliyun_access_key_secret": cfg.asr.aliyun_access_key_secret,
            "aliyun_app_key": cfg.asr.aliyun_app_key,
            "terminology": cfg.asr.terminology,
        },
        "audio": cfg.audio,
        // 通用表：每个插件的默认值 + 用户覆盖，键就是插件 id。
        // 前端不需要预先知道有哪些插件（Task 4 的 /plugins 端点给出元数据）。
        "plugins": talksage_plugins::effective_plugin_configs(&cfg.plugins.entries),
        "knowledge_base": {
            "enabled": cfg.knowledge_base.enabled,
            "folder": cfg.knowledge_base.folder,
        },
        "server": { "host": cfg.server.host, "port": cfg.server.port },
    });
    (StatusCode::OK, Json(body)).into_response()
}

/// 插件元数据（设置页据此生成表单）。
///
/// 与 `/config` 一样走 `token_ok`：这里枚举的是「本机装了哪些插件、默认配置
/// 长什么样」，属于配置面，不该匿名可读。
async fn list_plugins_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    Json(talksage_plugins::plugin_metadata()).into_response()
}

async fn plugin_status_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    Json(state.service.plugin_registrations()).into_response()
}

async fn asr_models_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let root = resolve_models_dir();
    let mut models: Vec<_> = EngineKind::ALL.iter().map(|&kind| {
        let p = kind.profile();
        serde_json::json!({
            "id": kind.display_name(), "label": p.label, "languages": p.languages,
            "streaming": p.streaming, "speed": p.speed, "description": p.description,
            "selectable": p.selectable,
            "installed": root.as_ref().is_some_and(|r| kind.is_available(r)),
            "size_mb": root.as_ref().map(|r| talksage_asr::models::installed_size_mb(kind, r)).unwrap_or(0),
            "download_size_mb": talksage_asr::models::download_size_mb(kind),
            "downloading": root.as_ref().is_some_and(|r| talksage_asr::models::is_downloading(kind, r)),
        })
    }).collect();
    models.push(serde_json::json!({
        "id": "punct",
        "label": "标点恢复模型",
        "languages": ["zh", "en"],
        "streaming": true,
        "speed": "fast",
        "description": "CT-Transformer 中英文标点预测，用于流式引擎语义分句",
        "selectable": false,
        "installed": root.as_ref().is_some_and(|r| talksage_asr::is_punct_model_installed(r)),
        "size_mb": 0,
        "download_size_mb": talksage_asr::punct_download_size_mb(),
        "downloading": false,
    }));
    Json(models).into_response()
}

/// 下载/安装 ASR 引擎（后台线程；进度经 WS 广播 ModelProgress）。
async fn download_model_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(engine): AxumPath<String>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    // punct 模型独立处理，不走 EngineKind 查表
    if engine == "punct" {
        let Some(root) = resolve_models_dir() else {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未找到 models/ 目录" }))).into_response();
        };
        if talksage_asr::is_punct_model_installed(&root) {
            return (StatusCode::OK, Json(serde_json::json!({ "ok": true, "already_installed": true }))).into_response();
        }
        let punct_id = "punct".to_string();
        let cancel_flag = {
            let mut dl = state.downloads.lock().unwrap();
            if dl.contains_key(&punct_id) {
                return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "该模型已在下载中" }))).into_response();
            }
            let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            dl.insert(punct_id.clone(), flag.clone());
            flag
        };
        let downloads = state.downloads.clone();
        let events = state.events.clone();
        log::info!("服务端模型下载任务已提交: engine=punct root={}", root.display());
        tokio::task::spawn_blocking(move || {
            let emit_events = events.clone();
            let emit = move |stage: &str, percent: u32, message: &str| {
                let _ = emit_events.send(DomainEvent::ModelProgress {
                    engine: "punct".into(),
                    stage: stage.into(),
                    percent,
                    message: message.into(),
                });
            };
            emit("downloading", 0, "开始下载…");
            let result = talksage_asr::download_punct_model(&root, cancel_flag, None);
            match result {
                Ok(()) => { log::info!("服务端模型下载任务完成: engine=punct"); emit("done", 100, "安装完成") },
                Err(e) if e.downcast_ref::<talksage_asr::models::DownloadCancelled>().is_some() => { log::info!("服务端模型下载任务取消: engine=punct"); emit("cancelled", 0, "已取消") },
                Err(e) => { log::error!("服务端模型下载任务失败: engine=punct error={e}"); emit("error", 0, &e.to_string()) },
            }
            if let Ok(mut dl) = downloads.lock() {
                dl.remove("punct");
            }
        });
        return (StatusCode::ACCEPTED, Json(serde_json::json!({ "ok": true }))).into_response();
    }
    let kind = match EngineKind::from_name(&engine) {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("未知引擎: {engine}") }))).into_response(),
    };
    if !kind.is_product_model() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("旧模型 `{engine}` 已从产品模型管理移除") }))).into_response();
    }
    if state.running.lock().unwrap().is_some() {
        return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "请先停止监听再安装模型" }))).into_response();
    }
    let Some(root) = resolve_models_dir() else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未找到 models/ 目录" }))).into_response();
    };
    let engine_id = kind.display_name().to_string();
    let events = state.events.clone();
    // 注册取消标志（同一引擎已在下载则拒绝）
    let cancel_flag = {
        let mut dl = state.downloads.lock().unwrap();
        if dl.contains_key(&engine_id) {
            return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "该模型已在下载中" }))).into_response();
        }
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        dl.insert(engine_id.clone(), flag.clone());
        flag
    };
    let downloads = state.downloads.clone();
    log::info!("服务端模型下载任务已提交: engine={} root={}", engine_id, root.display());
    tokio::task::spawn_blocking(move || {
        let emit_events = events.clone();
        let emit_engine = engine_id.clone();
        let emit = move |stage: &str, percent: u32, message: &str| {
            let _ = emit_events.send(DomainEvent::ModelProgress {
                engine: emit_engine.clone(),
                stage: stage.into(),
                percent,
                message: message.into(),
            });
        };
        emit("downloading", 0, "开始下载…");
        // 进度闭包自持发送器克隆，避免借用 emit
        let progress_events = events.clone();
        let progress_engine = engine_id.clone();
        let progress = move |received: u64, total: u64| {
            let percent = if total > 0 { ((received as f64 / total as f64) * 100.0) as u32 } else { 0 };
            let _ = progress_events.send(DomainEvent::ModelProgress {
                engine: progress_engine.clone(),
                stage: "downloading".into(),
                percent,
                message: String::new(),
            });
        };
        let result = talksage_asr::models::download_engine(kind, &root, Some(&progress), Some(cancel_flag.as_ref()));
        match result {
            Ok(()) => { log::info!("服务端模型下载任务完成: engine={engine_id}"); emit("done", 100, "安装完成") },
            Err(e) if e.downcast_ref::<talksage_asr::models::DownloadCancelled>().is_some() => {
                log::info!("服务端模型下载任务取消: engine={engine_id}");
                emit("cancelled", 0, "已取消");
            }
            Err(e) => { log::error!("服务端模型下载任务失败: engine={engine_id} error={e}"); emit("error", 0, &e.to_string()) },
        }
        // 下载结束（成功/失败/取消）移除注册
        if let Ok(mut dl) = downloads.lock() {
            dl.remove(&engine_id);
        }
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "ok": true }))).into_response()
}

/// 取消正在进行的模型下载。
async fn cancel_model_download_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(engine): AxumPath<String>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let dl = state.downloads.lock().unwrap();
    match dl.get(&engine) {
        Some(flag) => {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
            log::info!("服务端模型下载收到取消请求: engine={engine}");
            (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "该模型没有正在进行的下载" }))).into_response(),
    }
}

/// 删除 ASR 引擎模型目录。
async fn remove_model_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(engine): AxumPath<String>,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    // punct 模型独立处理
    if engine == "punct" {
        let Some(root) = resolve_models_dir() else {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未找到 models/ 目录" }))).into_response();
        };
        return match talksage_asr::remove_punct_model(&root) {
            Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
        };
    }
    let kind = match EngineKind::from_name(&engine) {
        Some(k) => k,
        None => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("未知引擎: {engine}") }))).into_response(),
    };
    if !kind.is_product_model() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("旧模型 `{engine}` 已从产品模型管理移除") }))).into_response();
    }
    if state.running.lock().unwrap().is_some() {
        return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "请先停止监听再删除模型" }))).into_response();
    }
    let Some(root) = resolve_models_dir() else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未找到 models/ 目录" }))).into_response();
    };
    match talksage_asr::models::remove_engine(kind, &root) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
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
    if let Some(plugins) = updates.get("plugins") {
        let issues = talksage_plugins::validate_plugin_updates(plugins);
        if !issues.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid plugin config", "issues": issues })),
            )
                .into_response();
        }
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
            c.asr.aliyun_access_key_secret = v.trim().to_string();
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
        if let Some(v) = audio.get("input_gain_db").and_then(|v| v.as_f64()) {
            c.audio.input_gain_db = (v as f32).clamp(0.0, 24.0);
        }
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
    // 场景模式
    if let Some(scene) = updates.get("scene") {
        if let Some(m) = scene.get("mode").and_then(|v| v.as_str()) {
            c.scene.mode = match m {
                "dictation" => talksage_config::SceneMode::Dictation,
                "conversation" => talksage_config::SceneMode::Conversation,
                "translation" | "bilingual" => talksage_config::SceneMode::Bilingual,
                "live_translation" => talksage_config::SceneMode::LiveTranslation,
                "meeting" => talksage_config::SceneMode::Meeting,
                "lecture" => talksage_config::SceneMode::Lecture,
                "custom" => talksage_config::SceneMode::Custom,
                _ => talksage_config::SceneMode::Conversation,
            };
        }
        if let Some(cu) = scene.get("custom") {
            talksage_config::apply_scene_params(&mut c.scene.custom, cu);
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
        .filter(|e| {
            let fname = e.file_name();
            let name = fname.to_string_lossy();
            name.starts_with("talksage") && name.ends_with(".log")
        })
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
    let Some(llm) = TalkSageService::build_llm(&state.config) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未配置 LLM" }))).into_response();
    };
    let Some(template) = talksage_notes::get_template(&body.template_id) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未知模板" }))).into_response();
    };
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    let gen = NotesGenerator::new(llm);
    match gen.generate(&detail.segments, &detail.terms, &detail.translations, &detail.key_points, &template) {
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
    let Some(llm) = TalkSageService::build_llm(&state.config) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未配置 LLM" }))).into_response();
    };
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    let gen = talksage_notes::TrioGenerator::new(llm);
    match gen.generate(&detail.segments, &detail.key_points, body.meeting_name.as_deref(), body.meeting_description.as_deref()) {
        Ok(trio) => {
            let json = serde_json::to_value(&trio).unwrap_or_default();
            let _ = state.sessions.set_trio(id, &json.to_string());
            (StatusCode::OK, Json(json)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 导出会话为 Markdown 单文件（转写 + 纪要 + 指标 + 质量；借鉴 Call.md markdown-export）。
async fn export_session_api(State(state): State<ServerState>, headers: axum::http::HeaderMap, AxumPath(id): AxumPath<i64>) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    let md = talksage_session::export_markdown(&detail);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("text/markdown; charset=utf-8"))],
        md,
    )
        .into_response()
}

/// 导出纯文本转写（无 Markdown 标记）。
async fn export_session_text_api(State(state): State<ServerState>, headers: axum::http::HeaderMap, AxumPath(id): AxumPath<i64>) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    let text = talksage_session::export_transcript_text(&detail);
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("text/plain; charset=utf-8"))],
        text,
    )
        .into_response()
}

/// 导出完整录音（master wav）为文件下载。
async fn export_session_audio_api(State(state): State<ServerState>, headers: axum::http::HeaderMap, AxumPath(id): AxumPath<i64>) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    let Some(master) = detail.meta.as_ref().and_then(|m| m.master_recording.clone()) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "该会话没有完整录音（可能未开启录音，或录音文件缺失）" }))).into_response();
    };
    let src = std::path::PathBuf::from(&master);
    if !src.is_file() {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("录音文件不存在: {}", src.display()) }))).into_response();
    }
    let filename = format!("session-{id}.wav");
    match tokio::fs::read(&src).await {
        Ok(bytes) => {
            let mut resp = axum::response::Response::new(axum::body::Body::from(bytes));
            resp.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("audio/wav"),
            );
            resp.headers_mut().insert(
                axum::http::header::CONTENT_DISPOSITION,
                axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")).unwrap(),
            );
            resp.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("读取录音失败: {e}") }))).into_response(),
    }
}

/// 整理会中已落库要点（历史详情；无 LLM 配置时 400）。
async fn generate_highlights_api(State(state): State<ServerState>, headers: axum::http::HeaderMap, AxumPath(id): AxumPath<i64>) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let Some(llm) = TalkSageService::build_llm(&state.config) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "未配置 LLM" }))).into_response();
    };
    let Ok(detail) = state.sessions.get_session(id) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "会话不存在" }))).into_response();
    };
    match talksage_notes::generate_highlights(&detail.key_points, &detail.segments, &llm) {
        Ok(points) => (StatusCode::OK, Json(serde_json::json!({ "points": points }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

/// 验证 LLM 连接（设置页「检查」按钮）：body 可选 provider/base_url/model/api_key
/// 覆盖（用于表单未保存时验证）。不写入配置。
#[derive(serde::Deserialize)]
struct TestLlmBody {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

async fn test_llm_api(State(state): State<ServerState>, headers: axum::http::HeaderMap, body: axum::Json<TestLlmBody>) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let snapshot = state.config.snapshot();
    let provider = body.provider.clone().unwrap_or(snapshot.llm.default.clone());
    let Some(cfg) = snapshot.llm.providers.get(&provider) else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("未知 provider: {provider}") }))).into_response();
    };
    let llm = talksage_llm::OpenAICompatProvider::new(
        body.api_key
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| cfg.api_key.clone()),
        body.model.clone().unwrap_or_else(|| cfg.model.clone()),
        body.base_url.clone().unwrap_or_else(|| cfg.base_url.clone().unwrap_or_else(|| "https://api.deepseek.com/v1".to_string())),
    );
    // 网络调用放进阻塞线程池，别占 tokio worker
    match tokio::task::spawn_blocking(move || llm.test_connection()).await {
        Ok(Ok(())) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": format!("LLM 检查失败: {e}") }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": format!("检查线程失败: {e}") }))).into_response(),
    }
}

async fn start_listen_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    // start 要加载模型/装配管道（可能数秒），放进阻塞线程池，别占 tokio worker。
    let events = state.events.clone();
    let service = state.service.clone();
    let running = state.running.clone();
    match tokio::task::spawn_blocking(move || {
        let mut guard = running.lock().unwrap();
        if guard.is_some() {
            return Err(anyhow::anyhow!("已在监听中"));
        }
        let started = service.start(
            StartListen::desktop(),
            Arc::new(move |ev| {
                let _ = events.send(ev);
            }),
        )?;
        *guard = Some(started);
        Ok::<(), anyhow::Error>(())
    })
    .await
    {
        Ok(Ok(())) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}

async fn stop_listen_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let running = state.running.lock().unwrap().take();
    if let Some(running) = running {
        // finish 是重活（join ≤5s + 落库 + 主录音 + finalizer），放阻塞线程池，
        // 避免占住 tokio worker 拖慢其他请求。
        let service = state.service.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Err(e) = service.finish(running) {
                log::warn!("停止监听收尾失败: {e}");
            }
        })
        .await;
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

async fn pause_listen_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if !token_ok(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let paused = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("paused").and_then(|v| v.as_bool()))
        .unwrap_or(true);
    match state.running.lock().unwrap().as_ref() {
        Some(running) => {
            running.set_paused(paused);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true, "paused": running.is_paused() }))).into_response()
        }
        None => (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "not listening" }))).into_response(),
    }
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
    match state.running.lock().unwrap().as_ref() {
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
    let gain_db = state.config.snapshot().audio.input_gain_db;
    let result = tokio::task::spawn_blocking(move || {
        let identifier = talksage_pipeline::speaker::SpeakerIdentifier::new(
            &model,
            None,
            talksage_pipeline::speaker::DEFAULT_THRESHOLD,
        )
        .ok_or_else(|| "声纹模型加载失败".to_string())?;
        let (mut hub, rx) = talksage_audio::AudioHub::new_with_gain(100, gain_db);
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
        let (emb, voiced_samples, windows) = identifier
            .enrollment_profile(&audio)
            .ok_or("有效人声不足或录音质量较差，请连续朗读并避免长时间停顿")?;
        talksage_pipeline::speaker::save_owner_embedding(&data_dir, &emb)
            .map_err(|e| format!("保存声纹失败: {e}"))?;
        Ok::<(Vec<f32>, usize, usize), String>((emb, voiced_samples, windows))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));
    match result {
        Ok((emb, voiced_samples, windows)) => (StatusCode::OK, Json(serde_json::json!({
            "ok": true,
            "dim": emb.len(),
            "voiced_ms": voiced_samples * 1000 / 16000,
            "windows": windows,
        }))).into_response(),
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
#[derive(Deserialize)]
struct RecordingQuery {
    token: Option<String>,
}

async fn get_recording_api(
    State(state): State<ServerState>,
    headers: axum::http::HeaderMap,
    AxumPath(filename): AxumPath<String>,
    Query(query): Query<RecordingQuery>,
) -> impl IntoResponse {
    // HTMLAudioElement 不能附加 X-Talksage-Token，因此该只读媒体端点额外接受
    // URL 查询令牌。文件目标仍受下面的录音目录白名单约束。
    let query_token_ok = !state.token.is_empty() && query.token.as_deref() == Some(state.token.as_str());
    if !token_ok(&state, &headers) && !query_token_ok {
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
    let snap = state.running.lock().ok().and_then(|g| g.as_ref().map(|r| r.snapshot()));
    if let Some(s) = snap {
        let ev = talksage_core::DomainEvent::Snapshot {
            revision: s.revision,
            committed: s.committed,
            hypothesis: s.hypothesis,
            processed_until_sample: s.processed_until_sample,
            committed_until_sample: s.committed_until_sample,
            stage: s.stage,
        };
        if let Ok(text) = serde_json::to_string(&ev) {
            if socket.send(Message::Text(text.into())).await.is_err() {
                return;
            }
        }
    }
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

fn resolve_models_dir() -> Option<PathBuf> {
    TalkSageService::resolve_models_dir()
}

// ── OpenAI 兼容 API（/v1/*）──────────────────────────────
// 目标：既有 OpenAI 生态客户端/脚本（whisper 类工具、curl）可直接指向本服务
// 做本地转写，鉴权用标准 `Authorization: Bearer <token>`。

/// `GET /v1/models`：列出可用转写引擎。
async fn models_api(State(state): State<ServerState>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    if !token_ok_v1(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "unauthorized" }))).into_response();
    }
    let root = resolve_models_dir();
    let data: Vec<serde_json::Value> = EngineKind::ALL
        .iter()
        .filter(|&&k| k.profile().selectable && root.as_ref().is_some_and(|r| k.is_available(r)))
        .map(|k| serde_json::json!({ "id": k.display_name(), "object": "model", "owned_by": "talksage" }))
        .collect();
    Json(serde_json::json!({ "object": "list", "data": data })).into_response()
}

/// 解析引擎路径（VAD + ASR 模型目录）。
fn engine_paths(kind: EngineKind) -> Result<(PathBuf, PathBuf)> {
    let model_dir = resolve_models_dir().ok_or_else(|| anyhow!("未找到 models/ 目录"))?;
    let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
    let engine_dir = model_dir.join(kind.model_dir_name());
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
        .unwrap_or_else(|| EngineKind::from_name(&state.config.snapshot().asr.engine_zh).unwrap_or(EngineKind::ParaformerZh));
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
    let pool = state.service.engines().clone();
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
