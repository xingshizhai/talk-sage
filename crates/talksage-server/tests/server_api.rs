//! server 接口集成测试：tower oneshot 驱动路由（无真实端口/音频）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::sync::broadcast;
use tower::ServiceExt;

use talksage_core::DomainEvent;
use talksage_server::{build_router, ServerState};

fn test_state() -> ServerState {
    let config = Arc::new(talksage_config::ConfigManager::load(None, None).unwrap());
    let sessions = Arc::new(talksage_session::SessionStore::open(":memory:").unwrap());
    let (tx, _rx) = broadcast::channel::<DomainEvent>(16);
    ServerState {
        config,
        sessions,
        events: tx,
        pipeline: Arc::new(std::sync::Mutex::new(None)),
        current_session: Arc::new(std::sync::Mutex::new(None)),
        token: String::new(),
        engine_pool: talksage_asr::EnginePool::new(),
    }
}

fn app() -> axum::Router {
    let state = test_state();
    build_router(state, &std::path::PathBuf::from("nonexistent-dist"))
}

async fn get(path: &str) -> (StatusCode, String) {
    let resp = app()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&body).to_string())
}

#[tokio::test]
async fn health_returns_ok_and_version() {
    let (status, body) = get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\":true"));
    assert!(body.contains("0.1.0"));
}

#[tokio::test]
async fn sessions_list_empty_on_fresh_db() {
    let (status, body) = get("/api/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "[]");
}

#[tokio::test]
async fn search_returns_json_array() {
    let (status, body) = get("/api/search?q=anything").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "[]");
}

#[tokio::test]
async fn missing_session_returns_404() {
    let (status, _) = get("/api/session/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn templates_list_builtin() {
    let (status, body) = get("/api/templates").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("standard_meeting"));
    assert!(body.contains("negotiation"));
}

#[tokio::test]
async fn unknown_api_returns_404() {
    let (status, _) = get("/api/definitely-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── OpenAI 兼容 API（/v1/*）────────────────────────────────

fn models_root() -> Option<std::path::PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = std::path::PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for cand in [
        here.join("../../models"),
        here.join("../../../models"),
        std::path::PathBuf::from("models"),
        std::path::PathBuf::from("../models"),
    ] {
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

const BOUNDARY: &str = "----talksage-test-boundary";

fn multipart_wav(wav_bytes: &[u8], model: &str) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(wav_bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn openai_models_lists_engines() {
    let (status, body) = get("/v1/models").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("paraformer-zh"), "body={body}");
    assert!(body.contains("zipformer-en"), "body={body}");
}

#[tokio::test]
async fn openai_transcribe_rejects_non_wav() {
    let body = multipart_wav(b"this is definitely not a wav file", "paraformer-zh");
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn openai_transcribe_rejects_missing_file() {
    let body = format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nparaformer-zh\r\n--{BOUNDARY}--\r\n").into_bytes();
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn openai_transcribe_wav_returns_text() {
    let Some(root) = models_root() else {
        eprintln!("跳过：未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
        return;
    };
    let wav = root.join("sherpa-onnx-streaming-paraformer-zh").join("0.wav");
    if !wav.is_file() {
        eprintln!("跳过：测试音频不完整");
        return;
    }
    let wav_bytes = std::fs::read(&wav).unwrap();
    let body = multipart_wav(&wav_bytes, "paraformer-zh");
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&bytes).to_string();
    let v: serde_json::Value = serde_json::from_str(&body_str).expect("响应不是 JSON");
    let text = v["text"].as_str().unwrap_or("");
    assert!(!text.trim().is_empty(), "转写结果为空: {body_str}");
    eprintln!("openai 转写结果: {text}");
}
