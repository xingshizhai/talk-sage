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
