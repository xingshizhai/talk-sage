//! server 接口集成测试：tower oneshot 驱动路由（无真实端口/音频）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::sync::broadcast;
use tower::ServiceExt;

use talksage_core::DomainEvent;
use talksage_server::{build_router, ServerState};

/// 资源缺失：默认打印并跳过；`TALKSAGE_REQUIRE_MODELS=1` 时直接失败，
/// 避免 CI 上「因跳过而全绿」掩盖回归。
fn skip(reason: &str) {
    let require = matches!(
        std::env::var("TALKSAGE_REQUIRE_MODELS").ok().as_deref(),
        Some("1") | Some("true")
    );
    assert!(
        !require,
        "集成测试资源缺失（TALKSAGE_REQUIRE_MODELS=1 要求必须真实运行）: {reason}"
    );
    eprintln!("跳过：{reason}");
}

fn test_state() -> ServerState {
    // API 转写会在 data_dir/tmp 落中间 WAV；测试不能依赖用户主目录可写。
    let data_dir = std::env::temp_dir().join(format!(
        "talksage-server-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&data_dir).unwrap();
    let config = Arc::new(talksage_config::ConfigManager::from_config(
        talksage_config::Config::default(),
        data_dir,
    ));
    let sessions = Arc::new(talksage_session::SessionStore::open(":memory:").unwrap());
    let (tx, _rx) = broadcast::channel::<DomainEvent>(16);
    let service = talksage_pipeline::TalkSageService::new(
        config.clone(),
        Some(sessions.clone()),
        talksage_asr::EnginePool::new(),
    );
    ServerState {
        config,
        sessions,
        events: tx,
        running: Arc::new(std::sync::Mutex::new(None)),
        downloads: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        token: String::new(),
        service,
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
    // 版本号随发布递增，这里只断言存在 version 字段，不锁死具体值
    assert!(body.contains("\"version\""), "应返回 version 字段: {body}");
}

#[tokio::test]
async fn config_and_asr_status_expose_routing_state() {
    let (status, body) = get("/api/config").await;
    assert_eq!(status, StatusCode::OK);
    let config: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(config["asr"]["asr_mode"], "auto");
    assert!(config["asr"].get("punct_enabled").is_some());
    assert!(config["asr"].get("aliyun_access_key_id").is_some());

    let (status, body) = get("/api/asr/gpu_status").await;
    assert_eq!(status, StatusCode::OK);
    let runtime: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(runtime.get("backend").is_some());
    assert!(runtime.get("hardware_candidate").is_some());
    assert!(runtime.get("availability_note").is_some());
    assert!(runtime.get("effective_route").is_some());
    assert!(runtime.get("route_error").is_some());
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
async fn recording_query_token_supports_native_audio_playback() {
    let mut state = test_state();
    state.token = "history-audio-token".to_string();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/recordings/missing.wav")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    // HTMLAudioElement 无法设置自定义鉴权头；正确的查询令牌应通过鉴权，
    // 随后才因测试文件不存在返回 404。
    let authorized = router
        .oneshot(
            Request::builder()
                .uri("/api/recordings/missing.wav?token=history-audio-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn templates_list_builtin() {
    let (status, body) = get("/api/templates").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("standard_meeting"));
    assert!(body.contains("negotiation"));
}

/// `/plugins` 是设置页生成表单的唯一来源：必须列出全部内置插件，
/// 且每项带 id / label / schema.enabled。
#[tokio::test]
async fn plugins_endpoint_lists_every_builtin_plugin() {
    let (status, body) = get("/api/plugins").await;
    assert_eq!(status, StatusCode::OK);
    let metas: Vec<serde_json::Value> = serde_json::from_str(&body).expect("应为 JSON 数组");
    assert_eq!(metas.len(), talksage_plugins::builtin_plugins().len());
    for m in &metas {
        assert!(m["id"].as_str().is_some_and(|s| !s.is_empty()), "缺少 id: {m}");
        assert!(m["label"].as_str().is_some_and(|s| !s.is_empty()), "缺少 label: {m}");
        assert!(m["description"].as_str().is_some_and(|s| !s.is_empty()), "缺少 description: {m}");
        assert!(matches!(m["category"].as_str(), Some("analysis" | "infrastructure")));
        assert!(matches!(m["phase"].as_str(), Some("filter" | "observer" | "finalizer")));
        assert!(m["capabilities"].is_array());
        assert!(m["after"].is_array());
        assert!(m["schema"]["enabled"].is_boolean(), "缺少 schema.enabled: {m}");
        assert_eq!(m["config_schema"]["additionalProperties"], false);
        assert_eq!(m["config_schema"]["properties"]["enabled"]["type"], "boolean");
    }
}

#[tokio::test]
async fn save_config_rejects_invalid_plugin_config() {
    let state = test_state();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"plugins":{"term_explainer":{"enabled":"yes"}}}"#))
                .unwrap(),
        )
        .await        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "invalid plugin config");
    assert_eq!(json["issues"][0]["path"], "term_explainer.enabled");
}

/// 端点枚举的是配置面信息，必须与 `/config` 同样受 token 保护 ——
/// 匿名可读就是一次真实的信息泄露回归。
#[tokio::test]
async fn plugins_endpoint_requires_the_token() {
    let mut state = test_state();
    state.token = "plugins-token".to_string();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));

    let anonymous = router
        .clone()
        .oneshot(Request::builder().uri("/api/plugins").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let authorized = router
        .oneshot(
            Request::builder()
                .uri("/api/plugins")
                .header("x-talksage-token", "plugins-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);
}

#[tokio::test]
async fn plugin_status_reports_current_capability_availability() {
    let (status, body) = get("/api/plugins/status").await;
    assert_eq!(status, StatusCode::OK);
    let registrations: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
    assert_eq!(registrations.len(), talksage_plugins::builtin_plugins().len());
    let term = registrations.iter().find(|item| item["id"] == "term_explainer").unwrap();
    assert_eq!(term["status"], "unavailable");
    assert!(term["missing_capabilities"].as_array().unwrap().contains(&serde_json::json!("llm")));
    let quality = registrations.iter().find(|item| item["id"] == "session_quality").unwrap();
    assert_eq!(quality["status"], "active", "server 有会话存储宿主");
}

#[tokio::test]
async fn unknown_api_returns_404() {
    let (status, _) = get("/api/definitely-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn export_returns_markdown_for_session() {
    // 造一个带数据的会话：通过 sessions API 无法直接插入，用 SessionStore 直接写
    let state = {
        let config = Arc::new(talksage_config::ConfigManager::load(None, None).unwrap());
        let sessions = Arc::new(talksage_session::SessionStore::open(":memory:").unwrap());
        let id = sessions.start_session(1).unwrap();
        sessions
            .add_segment(
                id,
                &talksage_core::TranscriptSegment {
                    speaker_id: 1,
                    speaker_label: "客户".into(),
                    speaker_attribution: None,
                    text: "We need NPI samples by Friday.".into(),
                    is_partial: false,
                    ts_ms: 500,
                    duration_ms: 500,
                    rms: 0.2,
                },
            )
            .unwrap();
        sessions.set_notes(id, "# 纪要").unwrap();
        let (tx, _rx) = broadcast::channel::<DomainEvent>(16);
        let service = talksage_pipeline::TalkSageService::new(
            config.clone(),
            Some(sessions.clone()),
            talksage_asr::EnginePool::new(),
        );
        ServerState {
            config,
            sessions,
            events: tx,
            running: Arc::new(std::sync::Mutex::new(None)),
            downloads: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            token: String::new(),
            service,
        }
    };
    let resp = build_router(state, &std::path::PathBuf::from("nonexistent-dist"))
        .oneshot(Request::builder().uri("/api/session/1/export").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let md = String::from_utf8_lossy(&bytes).to_string();
    assert!(md.contains("# 会议记录"), "导出应为 Markdown: {md}");
    assert!(md.contains("[客户]"), "应含说话人标签: {md}");
    assert!(md.contains("We need NPI samples by Friday."));
}

/// PATCH /session/{id}：重命名会话，空串清除自定义名。
#[tokio::test]
async fn patch_session_renames_and_clears_title() {
    let state = test_state();
    let sessions = state.sessions.clone();
    let id = sessions.start_session(1).unwrap();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));

    let patch = |body: &'static str| {
        router.clone().oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/session/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
    };

    let resp = patch(r#"{"title":"周三 NPI 评审"}"#).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(sessions.get_session(id).unwrap().title.as_deref(), Some("周三 NPI 评审"));

    let resp = patch(r#"{"title":""}"#).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(sessions.get_session(id).unwrap().title, None, "空串应清除自定义名");
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
    assert!(body.contains("\"object\":\"list\""), "body={body}");
    // 该端点只列「已安装」引擎：无模型时列表为空是正确行为；有模型时
    // 应包含引擎（与 models_root 判定一致，避免在 CI 无模型环境误报）。
    let Some(root) = models_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    if root.join("sherpa-onnx-qwen3-asr-0.6b").is_dir() {
        assert!(body.contains("qwen3-asr"), "body={body}");
    }
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert!(body.contains("whisper-large-v3-turbo-metal"));
    } else {
        assert!(!body.contains("whisper-large-v3-turbo-metal"));
    }
}

#[tokio::test]
async fn openai_transcribe_rejects_non_wav() {
    // 非法 wav 的校验发生在「引擎路径就绪」之后：无模型时先返回 503，
    // 该测试在无模型环境跳过（与项目模型测试约定一致）。
    let Some(root) = models_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    if !root.join("silero-vad").join("silero_vad.onnx").is_file()
        || !root.join("sherpa-onnx-streaming-paraformer-zh").is_dir()
    {
        return skip("模型不完整（需要 VAD + paraformer-zh）");
    }
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
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = root.join("sherpa-onnx-streaming-paraformer-zh").join("0.wav");
    if !wav.is_file() {
        return skip("测试音频不完整");
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
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&bytes).to_string();
    assert_eq!(status, StatusCode::OK, "转写请求失败: {body_str}");
    let v: serde_json::Value = serde_json::from_str(&body_str).expect("响应不是 JSON");
    let text = v["text"].as_str().unwrap_or("");
    assert!(!text.trim().is_empty(), "转写结果为空: {body_str}");
    eprintln!("openai 转写结果: {text}");
}

/// 设置页保存的 ASR 引擎应持久化：POST /config 写 asr.engine_zh 后，
/// 内存快照立即反映新值（前端保存后 getConfig 应读到它）。
#[tokio::test]
async fn save_config_persists_asr_engine_choice() {
    let state = test_state();
    let router = build_router(state.clone(), &std::path::PathBuf::from("nonexistent-dist"));
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"asr":{"engine_zh":"qwen3-asr"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(state.config.snapshot().asr.engine_zh, "qwen3-asr");
}

#[tokio::test]
async fn save_config_persists_gpu_cloud_routing_fields() {
    let state = test_state();
    let router = build_router(state.clone(), &std::path::PathBuf::from("nonexistent-dist"));
    let body = serde_json::json!({
        "asr": {
            "asr_mode": "cloud",
            "backend": "cuda",
            "punct_enabled": false,
            "aliyun_access_key_id": "id",
            "aliyun_access_key_secret": "secret",
            "aliyun_app_key": "app"
        }
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let asr = state.config.snapshot().asr;
    assert_eq!(asr.asr_mode, "cloud");
    assert_eq!(asr.backend, "cuda");
    assert!(!asr.punct_enabled);
    assert_eq!(asr.aliyun_access_key_id, "id");
    assert_eq!(asr.aliyun_access_key_secret, "secret");
    assert_eq!(asr.aliyun_app_key, "app");
}

/// 场景自定义里的引擎同样应持久化（pipeline 实际按场景引擎运行）。
#[tokio::test]
async fn save_config_persists_scene_custom_engine() {
    let state = test_state();
    let router = build_router(state.clone(), &std::path::PathBuf::from("nonexistent-dist"));
    let body = serde_json::json!({
        "scene": {
            "mode": "custom",
            "custom": { "user_engine": "qwen3-asr", "client_engine": "qwen3-asr" }
        }
    });
    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let snap = state.config.snapshot();
    assert_eq!(snap.scene.mode, talksage_config::SceneMode::Custom);
    assert_eq!(snap.scene.custom.user_engine, "qwen3-asr");
    assert_eq!(snap.scene.custom.client_engine, "qwen3-asr");
}
