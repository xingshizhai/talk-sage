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
        chat: Arc::new(talksage_pipeline::chat::ChatService::with_knowledge(
            config.clone(),
            sessions.clone(),
            service.knowledge(),
        )),
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
        assert!(matches!(m["category"].as_str(), Some("analysis" | "infrastructure" | "knowledge_source")));
        assert!(matches!(m["phase"].as_str(), Some("filter" | "observer" | "finalizer" | "source")));
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
                &talksage_core::TranscriptSegment { id: None,
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
            chat: Arc::new(talksage_pipeline::chat::ChatService::with_knowledge(
                config.clone(),
                sessions.clone(),
                service.knowledge(),
            )),
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

/// PATCH/DELETE /session/{id}/segments/{seg_id}：编辑/删除转写段。
/// 详情 JSON 必须带段 id（前端编辑/删除按 id 定位），编辑/删除后派生的纪要/要点被清除。
#[tokio::test]
async fn segment_edit_and_delete_over_http() {
    let state = test_state();
    let sessions = state.sessions.clone();
    let id = sessions.start_session(1).unwrap();
    sessions
        .add_segment(
            id,
            &talksage_core::TranscriptSegment {
                id: None,
                speaker_id: 1,
                speaker_label: "客户".into(),
                speaker_attribution: None,
                text: "We need NPI samples".into(),
                is_partial: false,
                ts_ms: 500,
                duration_ms: 500,
                rms: 0.2,
            },
        )
        .unwrap();
    sessions.set_notes(id, "旧纪要").unwrap();
    let seg_id = sessions.get_session(id).unwrap().segments[0].id.expect("详情应带段 id");
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));

    // 详情 JSON 里段应带 id 字段（前端编辑/删除的定位依据）
    let detail_resp = router
        .clone()
        .oneshot(Request::builder().uri(format!("/api/session/{id}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(detail_resp.into_body(), usize::MAX).await.unwrap();
    let detail_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(detail_json["segments"][0]["id"], seg_id, "详情段应带数据库 id: {detail_json}");

    // 编辑段文本
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/session/{id}/segments/{seg_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"We need NPI samples by Friday."}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = sessions.get_session(id).unwrap();
    assert_eq!(detail.segments[0].text, "We need NPI samples by Friday.");
    assert!(detail.notes.is_none(), "编辑后旧纪要应清除");

    // 删除段
    let del = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/session/{id}/segments/{seg_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    assert!(sessions.get_session(id).unwrap().segments.is_empty());

    // 不存在的段 → 404（而不是 500）
    let missing = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/session/{id}/segments/99999"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

/// WebSocket 必须在根路径 `/ws`：前端连的就是它，挂进 /api 的 nest 会 404，
/// 浏览器模式下所有实时事件（转写增量、AI 助手回答）都会静默丢失。
#[tokio::test]
async fn websocket_is_served_at_root_path() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/ws")
                .header("connection", "upgrade")
                .header("upgrade", "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // oneshot 里没有真正的连接可升级，axum 的 WebSocketUpgrade 因此返回 426；
    // 关键是它**走到了** WS 提取器 —— 挂错位置时这里是 404。
    assert_eq!(
        resp.status(),
        StatusCode::UPGRADE_REQUIRED,
        "/ws 应命中 WebSocket 处理器（426 = 已进入升级流程）"
    );

    // /api/ws 不再提供（历史上错挂在这里）
    let (status, _) = get("/api/ws").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// AI 助手：话题增删改查（不触发 LLM，纯存储路径）。
#[tokio::test]
async fn chat_thread_crud_over_http() {
    let state = test_state();
    let sessions = state.sessions.clone();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));

    let created = router
        .clone()
        .oneshot(Request::builder().method("POST").uri("/api/chat/threads").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(created.into_body(), usize::MAX).await.unwrap();
    let id = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["id"].as_i64().unwrap();

    sessions.add_chat_message(id, "user", "会议纪要怎么写", 1_000).unwrap();

    let listed = router
        .clone()
        .oneshot(Request::builder().uri("/api/chat/threads").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(listed.into_body(), usize::MAX).await.unwrap();
    let threads: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(threads[0]["id"].as_i64(), Some(id));
    assert_eq!(threads[0]["title"].as_str(), Some("会议纪要怎么写"), "首条提问自动成为标题");
    assert_eq!(threads[0]["message_count"].as_u64(), Some(1));

    let msgs = router
        .clone()
        .oneshot(Request::builder().uri(format!("/api/chat/threads/{id}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(msgs.into_body(), usize::MAX).await.unwrap();
    let msgs: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(msgs[0]["role"].as_str(), Some("user"));

    let renamed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/chat/threads/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"纪要模板"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    assert_eq!(sessions.list_chat_threads(10).unwrap()[0].title.as_deref(), Some("纪要模板"));

    let deleted = router
        .oneshot(Request::builder().method("DELETE").uri(format!("/api/chat/threads/{id}")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert!(sessions.list_chat_threads(10).unwrap().is_empty());
}

/// 没配 LLM key 时提问应给出可读原因，而不是 500 或静默失败。
#[tokio::test]
async fn chat_send_without_llm_key_returns_readable_error() {
    let state = test_state();
    let sessions = state.sessions.clone();
    let id = sessions.create_chat_thread(1).unwrap();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/chat/threads/{id}/messages"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"在吗"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["error"].as_str().unwrap().contains("LLM"), "错误应指向 LLM 配置: {json}");
    assert!(sessions.get_chat_messages(id).unwrap().is_empty(), "失败时不留半条对话");
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
    // 是否上榜由 EngineKind::profile().selectable 决定，直接问库而不是在这里重写
    // 平台判断：`cfg!(feature = "vulkan-gpu")` 在本测试 crate 里指的是 talksage-server
    // 自己的 feature（永远为假），而 talksage-asr 的这个 feature 会被 talksage-app
    // 的依赖在 workspace 构建时统一打开 —— 照抄 cfg 只会写出跑 workspace 必挂的断言。
    let metal_selectable = talksage_asr::EngineKind::WhisperLargeV3TurboMetal.profile().selectable;
    if !metal_selectable {
        assert!(!body.contains("whisper-large-v3-turbo-metal"), "不可选的模型不该上榜: body={body}");
    } else if root.join("whisper.cpp-large-v3-turbo-q5_0").is_dir() {
        // 可选 + 已安装才会列出（与 models_api 的 selectable && is_available 一致）
        assert!(body.contains("whisper-large-v3-turbo-metal"), "body={body}");
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

/// GET /config 上取一份 JSON（复用同一个 router，配置状态才连得上）。
async fn get_config_json(router: &axum::Router) -> serde_json::Value {
    let resp = router
        .clone()
        .oneshot(Request::builder().uri("/api/config").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn post_config(router: &axum::Router, body: &serde_json::Value) -> StatusCode {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/config")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// `GET /config` 必须返回完整的 scene（mode + custom）——
/// 浏览器模式下设置页的「场景模式」全靠它还原；缺字段时前端只能显示默认值，
/// 保存又会把默认值写回去，等于静默覆盖用户的真实场景。
#[tokio::test]
async fn config_endpoint_returns_scene_mode_and_custom() {
    let (status, body) = get("/api/config").await;
    assert_eq!(status, StatusCode::OK);
    let cfg: serde_json::Value = serde_json::from_str(&body).expect("应为 JSON 对象");
    assert_eq!(cfg["scene"]["mode"], "conversation");
    // custom 是自定义模式的完整参数表，前端逐字段回填，不能只给 mode。
    assert!(cfg["scene"]["custom"].is_object(), "缺少 scene.custom: {body}");
    assert!(cfg["scene"]["custom"]["user_engine"].is_string());
    assert!(cfg["scene"]["custom"]["plugin_allowlist"].is_array());
}

/// 设置页每个 tab 读的配置段都得在 —— 少一段，那个 tab 就显示默认值，
/// 保存时再把默认值写回去。scene / recording / quality / network 都这么丢过。
#[tokio::test]
async fn config_endpoint_carries_every_section_the_settings_page_reads() {
    let (status, body) = get("/api/config").await;
    assert_eq!(status, StatusCode::OK);
    let cfg: serde_json::Value = serde_json::from_str(&body).unwrap();
    for section in [
        "asr", "audio", "llm", "plugins", "scene", "recording", "quality",
        "webhooks", "network", "knowledge_base", "server",
    ] {
        assert!(cfg.get(section).is_some(), "GET /config 缺少 `{section}` 段: {body}");
    }
    // plugins 是「默认值 + 用户覆盖」的生效配置，不是用户显式写过的那几个。
    assert!(cfg["plugins"].as_object().is_some_and(|p| !p.is_empty()));
}

/// headless 走网络，且 `/config` 在未设 token 时匿名可读 —— 密钥必须打码。
/// 标识（AccessKey ID / AppKey）不是凭据，保持明文供设置页显示与验签。
#[tokio::test]
async fn config_endpoint_masks_credentials_over_http() {
    let state = test_state();
    state
        .config
        .update(|c| {
            c.llm.default = "deepseek".into();
            c.llm.providers.insert(
                "deepseek".into(),
                talksage_config::LlmProviderConfig {
                    base_url: None,
                    model: "deepseek-chat".into(),
                    api_key: "sk-1234567890abcdef".into(),
                },
            );
            c.asr.aliyun_access_key_id = "LTAI5tSomeKeyId".into();
            c.asr.aliyun_access_key_secret = "verySecretValue123".into();
            c.asr.aliyun_app_key = "app-key-plain".into();
        })
        .unwrap();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));

    let cfg = get_config_json(&router).await;
    let body = cfg.to_string();
    for secret in ["sk-1234567890abcdef", "verySecretValue123"] {
        assert!(!body.contains(secret), "密钥明文出现在 /config 响应里: {secret}");
    }
    assert_eq!(cfg["asr"]["aliyun_access_key_id"], "LTAI5tSomeKeyId");
    assert_eq!(cfg["asr"]["aliyun_app_key"], "app-key-plain");
    // 打码不等于清空：前端要能看出「已配置」。
    assert!(cfg["llm"]["providers"]["deepseek"]["api_key"].as_str().is_some_and(|s| !s.is_empty()));
}

/// 设置页最常见的一次操作：打开设置、改一个开关、点保存 —— 提交的是它读到的
/// 那份快照。快照原样写回后，密钥和各配置段必须一个不少、一个不改。
/// 这条挂了，用户的表现就是「改了个采样开关，LLM key 没了」。
#[tokio::test]
async fn saving_the_snapshot_back_changes_nothing() {
    let state = test_state();
    state
        .config
        .update(|c| {
            c.llm.default = "deepseek".into();
            c.llm.providers.insert(
                "deepseek".into(),
                talksage_config::LlmProviderConfig {
                    base_url: Some("https://api.deepseek.com/v1".into()),
                    model: "deepseek-chat".into(),
                    api_key: "sk-1234567890abcdef".into(),
                },
            );
            c.asr.aliyun_access_key_secret = "verySecretValue123".into();
            c.scene.mode = talksage_config::SceneMode::Meeting;
            c.webhooks.urls = vec!["https://example.com/hook".into()];
            c.network.proxy = "http://127.0.0.1:7890".into();
        })
        .unwrap();
    let router = build_router(state.clone(), &std::path::PathBuf::from("nonexistent-dist"));

    let snapshot = get_config_json(&router).await;
    assert_eq!(post_config(&router, &snapshot).await, StatusCode::OK);

    let after = state.config.snapshot();
    assert_eq!(after.llm.providers["deepseek"].api_key, "sk-1234567890abcdef");
    assert_eq!(after.asr.aliyun_access_key_secret, "verySecretValue123");
    assert_eq!(after.scene.mode, talksage_config::SceneMode::Meeting);
    assert_eq!(after.webhooks.urls, vec!["https://example.com/hook".to_string()]);
    assert_eq!(after.network.proxy, "http://127.0.0.1:7890");
}

/// 保存 → 读回必须闭环：POST 写入的场景要能被 GET 读到，
/// 否则设置页刷新后又退回默认值。
#[tokio::test]
async fn config_endpoint_reflects_saved_scene() {
    let state = test_state();
    let router = build_router(state, &std::path::PathBuf::from("nonexistent-dist"));
    let body = serde_json::json!({
        "scene": { "mode": "meeting", "custom": { "user_engine": "qwen3-asr" } }
    });
    assert_eq!(post_config(&router, &body).await, StatusCode::OK);

    let cfg = get_config_json(&router).await;
    assert_eq!(cfg["scene"]["mode"], "meeting");
    assert_eq!(cfg["scene"]["custom"]["user_engine"], "qwen3-asr");
}

/// recording / quality / network / audio_source 以前只有桌面端那份拷贝认，
/// headless 收到就丢 —— 用户在浏览器里改完设置，保存成功，什么也没变。
#[tokio::test]
async fn save_config_persists_recording_quality_network_and_audio_source() {
    let state = test_state();
    let router = build_router(state.clone(), &std::path::PathBuf::from("nonexistent-dist"));
    let body = serde_json::json!({
        "audio": { "audio_source": "loopback" },
        "recording": { "enabled": false, "dir": "D:/rec" },
        "quality": { "auto_detect": false, "silence_rms": 0.02 },
        "network": { "proxy": "http://127.0.0.1:7890" },
    });
    assert_eq!(post_config(&router, &body).await, StatusCode::OK);

    let snap = state.config.snapshot();
    assert_eq!(snap.audio.audio_source, "loopback");
    assert!(!snap.recording.enabled);
    assert_eq!(snap.recording.dir, "D:/rec");
    assert!(!snap.quality.auto_detect);
    assert_eq!(snap.network.proxy, "http://127.0.0.1:7890");
}

/// 用户在设置页真的换了一把 key：掩码之外的提交必须照写。
#[tokio::test]
async fn save_config_accepts_a_newly_typed_api_key() {
    let state = test_state();
    let router = build_router(state.clone(), &std::path::PathBuf::from("nonexistent-dist"));
    let body = serde_json::json!({
        "llm": { "default": "deepseek", "providers": { "deepseek": { "api_key": "sk-typed-by-user" } } }
    });
    assert_eq!(post_config(&router, &body).await, StatusCode::OK);
    assert_eq!(state.config.snapshot().llm.providers["deepseek"].api_key, "sk-typed-by-user");
}
