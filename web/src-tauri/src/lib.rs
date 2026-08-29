//! TalkSage v2 — Tauri 适配器。
//!
//! 职责：把 Rust 核心域暴露给前端（React SPA）：
//!   - command：get_version / get_config / ping / start_listen / stop_listen
//!   - event：领域事件推送（talksage://event 通道，含实时转写）
//!
//! 这是"可插拔传输适配器"之一；删除本 crate 即回到纯 headless（M4 预留 axum 适配器）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use talksage_audio::AudioHub;
use talksage_config::ConfigManager;
use talksage_core::{DomainEvent, StatusStage};
use talksage_pipeline::chat::{ChatEmit, ChatService};
use talksage_pipeline::{AudioInput, ClientCapture, RunningListen, StartListen, TalkSageService};
use talksage_asr::{EngineKind, EnginePool};
use talksage_session::SessionStore;

mod updater;
mod window_state;

/// 应用状态（Tauri managed state）。
pub struct AppState {
    config: Arc<ConfigManager>,
    /// 会话存储（常驻 SQLite）。
    sessions: Arc<SessionStore>,
    /// 共享用例入口（装配 Pipeline / 落库 / 质量评估）。
    service: TalkSageService,
    /// 当前监听（None = 未监听）。Arc 便于移入 spawn_blocking 闭包。
    running: Arc<Mutex<Option<RunningListen>>>,
    /// 进行中的模型下载（引擎 id → 取消标志）。下载开始注册、结束/取消移除。
    downloads: Arc<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
    /// 文件导入取消标志（None = 未在导入中）。
    import_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
    /// AI 助手（多轮对话 + 流式生成）。
    chat: Arc<ChatService>,
}

/// 版本。
#[tauri::command]
fn get_version() -> String {
    talksage_core::VERSION.to_string()
}

/// 配置快照。
///
/// 组装规则在 `talksage_config::ui_config_json`，headless 的 `GET /api/config`
/// 调的是同一个函数 —— 设置页两种 transport 下必须看到同一份数据。
///
/// 桌面端用 `Reveal`：IPC 不出进程，密钥本来就要显示在输入框里；headless 走
/// 网络，那边传 `Mask`。
#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> serde_json::Value {
    let snapshot = state.config.snapshot();
    talksage_config::ui_config_json(
        &snapshot,
        talksage_plugins::effective_plugin_configs(&snapshot.plugins.entries),
        talksage_config::SecretPolicy::Reveal,
    )
}

/// 插件元数据（id / 显示名 / 是否分析类 / 默认配置）。
///
/// 设置页据此**生成**插件表单，因此桌面端与 headless 的 `GET /api/plugins`
/// 必须是同一份数据 —— 两边都直接吐 `plugin_metadata()`，不做各自的加工。
#[tauri::command]
fn list_plugins() -> Vec<serde_json::Value> {
    talksage_plugins::plugin_metadata()
}

#[tauri::command]
fn list_plugin_status(state: tauri::State<'_, AppState>) -> Vec<talksage_plugins::PluginRegistration> {
    state.service.plugin_registrations()
}

/// 把录音目录加入 asset 协议只读范围（会话目录布局 `<data>/sessions/`）。
/// 录音目录来自配置 data_dir（默认 ~/.talksage，或 TALKSAGE_DATA_DIR），
/// 它可能不属于 Tauri 的 `$DATA_DIR`（例如用户自定义目录）。
fn allow_recording_assets(app: &tauri::AppHandle, config: &ConfigManager) -> Result<(), String> {
    let data_dir = config.data_dir();
    // 授权会话目录（<data>/sessions/，递归覆盖各会话的 recordings/）
    let sessions_dir = data_dir.join("sessions");
    std::fs::create_dir_all(&sessions_dir).map_err(|e| format!("创建会话目录失败 {}: {e}", sessions_dir.display()))?;
    app.asset_protocol_scope()
        .allow_directory(&sessions_dir, true)
        .map_err(|e| format!("授权录音播放目录失败 {}: {e}", sessions_dir.display()))?;
    // 兼容旧布局：<data>/recordings/
    let legacy_dir = config.snapshot().recording.resolve_dir(data_dir);
    std::fs::create_dir_all(&legacy_dir).map_err(|e| format!("创建录音目录失败 {}: {e}", legacy_dir.display()))?;
    app.asset_protocol_scope()
        .allow_directory(&legacy_dir, false)
        .map_err(|e| format!("授权录音播放目录失败 {}: {e}", legacy_dir.display()))?;
    log::info!("历史录音播放目录已授权: {} + {}", sessions_dir.display(), legacy_dir.display());
    Ok(())
}

/// ASR 模型目录：包含未安装模型，前端据此禁用选项并展示速度取舍。
#[tauri::command]
fn list_asr_models() -> Vec<serde_json::Value> {
    let root = TalkSageService::resolve_models_dir();
    let mut models: Vec<serde_json::Value> = EngineKind::ALL
        .iter()
        .map(|&kind| {
            let p = kind.profile();
            serde_json::json!({
                "id": kind.display_name(),
                "label": p.label,
                "languages": p.languages,
                "streaming": p.streaming,
                "speed": p.speed,
                "description": p.description,
                "selectable": p.selectable,
                "installed": root.as_ref().is_some_and(|r| kind.is_available(r)),
                "size_mb": root.as_ref().map(|r| talksage_asr::models::installed_size_mb(kind, r)).unwrap_or(0),
                "download_size_mb": talksage_asr::models::download_size_mb(kind),
                "downloading": root.as_ref().is_some_and(|r| talksage_asr::models::is_downloading(kind, r)),
            })
        })
        .collect();
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
    models
}

/// 下载/安装 ASR 引擎（后台线程；进度经 `talksage://event` 推送 ModelProgress）。
/// 下载期间注册到 `state.downloads`，可用 [`cancel_model_download`] 取消。
#[tauri::command]
async fn download_model(
    engine: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let proxy = state.config.snapshot().network.proxy_url().map(str::to_string);
    log::info!("download_model: engine={engine} proxy={:?}", proxy.as_deref().unwrap_or("(直连)"));
    // punct 模型独立处理，不走 EngineKind 查表
    if engine == "punct" {
        let Some(root) = TalkSageService::resolve_models_dir() else {
            return Err("未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）".into());
        };
        if talksage_asr::is_punct_model_installed(&root) {
            return Ok(());
        }
        let punct_id = "punct".to_string();
        let cancel_flag = {
            let mut dl = state.downloads.lock().map_err(|_| "下载注册表锁失败".to_string())?;
            if dl.contains_key(&punct_id) {
                return Err("该模型已在下载中".into());
            }
            let flag = Arc::new(AtomicBool::new(false));
            dl.insert(punct_id.clone(), flag.clone());
            flag
        };
        let app = app.clone();
        let downloads = state.downloads.clone();
        log::info!("桌面模型下载任务已提交: engine=punct root={}", root.display());
        tauri::async_runtime::spawn_blocking(move || {
            let emit_app = app.clone();
            let emit = move |stage: &str, percent: u32, message: &str| {
                let _ = emit_app.emit(
                    "talksage://event",
                    DomainEvent::ModelProgress {
                        engine: "punct".into(),
                        stage: stage.into(),
                        percent,
                        message: message.into(),
                    },
                );
            };
            emit("downloading", 0, "开始下载…");
            let result = talksage_asr::download_punct_model(&root, cancel_flag, None, proxy.as_deref());
            match result {
                Ok(()) => { log::info!("桌面模型下载任务完成: engine=punct"); emit("done", 100, "安装完成") },
                Err(e) if e.downcast_ref::<talksage_asr::models::DownloadCancelled>().is_some() => { log::info!("桌面模型下载任务取消: engine=punct"); emit("cancelled", 0, "已取消") },
                Err(e) => { log::error!("桌面模型下载任务失败: engine=punct error={e}"); emit("error", 0, &e.to_string()) },
            }
            if let Ok(mut registry) = downloads.lock() {
                registry.remove("punct");
            }
        });
        return Ok(());
    }
    let kind = EngineKind::from_name(&engine).ok_or_else(|| format!("未知引擎: {engine}"))?;
    if !kind.is_product_model() {
        return Err(format!("旧模型 `{engine}` 已从产品模型管理移除"));
    }
    if state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?.is_some() {
        return Err("请先停止监听再安装模型".into());
    }
    let Some(root) = TalkSageService::resolve_models_dir() else {
        return Err("未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）".into());
    };
    let engine_id = kind.display_name().to_string();
    // 注册取消标志：同一引擎已在下载则拒绝重复启动
    let cancel_flag = {
        let mut dl = state.downloads.lock().map_err(|_| "下载注册表锁失败".to_string())?;
        if dl.contains_key(&engine_id) {
            return Err("该模型已在下载中".into());
        }
        let flag = Arc::new(AtomicBool::new(false));
        dl.insert(engine_id.clone(), flag.clone());
        flag
    };
    let app = app.clone();
    // 外层（注册表清理）单独持有一份 engine_id
    let cleanup_engine = engine_id.clone();
    log::info!("桌面模型下载任务已提交: engine={} root={}", engine_id, root.display());
    let result = tauri::async_runtime::spawn_blocking(move || {
        let emit_app = app.clone();
        let emit_engine = engine_id.clone();
        let emit = move |stage: &str, percent: u32, message: &str| {
            let _ = emit_app.emit(
                "talksage://event",
                DomainEvent::ModelProgress {
                    engine: emit_engine.clone(),
                    stage: stage.into(),
                    percent,
                    message: message.into(),
                },
            );
        };
        emit("downloading", 0, "开始下载…");
        // 进度闭包自持 AppHandle 克隆，避免借用 emit
        let progress_app = app.clone();
        let progress_engine = engine_id.clone();
        // CDN 不一定返回 Content-Length；用已知预估大小兜底，避免进度条永远停在 0%
        let fallback_bytes = talksage_asr::models::download_size_mb(kind) * 1024 * 1024;
        let progress = move |received: u64, total: u64| {
            let effective_total = if total > 0 { total } else { fallback_bytes };
            let percent = if effective_total > 0 {
                ((received as f64 / effective_total as f64) * 100.0).min(99.0) as u32
            } else { 0 };
            let _ = progress_app.emit(
                "talksage://event",
                DomainEvent::ModelProgress {
                    engine: progress_engine.clone(),
                    stage: "downloading".into(),
                    percent,
                    message: String::new(),
                },
            );
        };
        let result = talksage_asr::models::download_engine(kind, &root, Some(&progress), Some(&cancel_flag), proxy.as_deref());
        match result {
            Ok(()) => {
                log::info!("桌面模型下载任务完成: engine={engine_id}");
                emit("done", 100, "安装完成");
                Ok(())
            }
            Err(e) => {
                // 用户主动取消：发"已取消"而非"失败"
                if e.downcast_ref::<talksage_asr::models::DownloadCancelled>().is_some() {
                    log::info!("桌面模型下载任务取消: engine={engine_id}");
                    emit("cancelled", 0, "已取消");
                    Ok(())
                } else {
                    log::error!("桌面模型下载任务失败: engine={engine_id} error={e}");
                    emit("error", 0, &e.to_string());
                    Err(e.to_string())
                }
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    // 无论成功/失败/取消，下载结束都要从注册表移除
    if let Ok(mut dl) = state.downloads.lock() {
        dl.remove(&cleanup_engine);
    }
    result
}

/// 取消正在进行的模型下载（置位取消标志；下载线程会尽快停止并清理临时文件）。
#[tauri::command]
fn cancel_model_download(engine: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let dl = state.downloads.lock().map_err(|_| "下载注册表锁失败".to_string())?;
    match dl.get(&engine) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            log::info!("桌面模型下载收到取消请求: engine={engine}");
            Ok(())
        }
        None => Err("该模型没有正在进行的下载".into()),
    }
}

/// 删除 ASR 引擎模型目录。
#[tauri::command]
fn remove_model(engine: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    // punct 模型独立处理
    if engine == "punct" {
        let Some(root) = TalkSageService::resolve_models_dir() else {
            return Err("未找到 models/ 目录".into());
        };
        return talksage_asr::remove_punct_model(&root).map_err(|e| format!("删除失败: {e}"));
    }
    let kind = EngineKind::from_name(&engine).ok_or_else(|| format!("未知引擎: {engine}"))?;
    if !kind.is_product_model() {
        return Err(format!("旧模型 `{engine}` 已从产品模型管理移除"));
    }
    if state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?.is_some() {
        return Err("请先停止监听再删除模型".into());
    }
    let Some(root) = TalkSageService::resolve_models_dir() else {
        return Err("未找到 models/ 目录".into());
    };
    talksage_asr::models::remove_engine(kind, &root).map_err(|e| format!("删除失败: {e}"))
}

/// 保存配置（前端设置面板提交，写入 talksage.toml）。
#[tauri::command]
fn save_config(
    updates: serde_json::Value,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if let Some(plugins) = updates.get("plugins") {
        let issues = talksage_plugins::validate_plugin_updates(plugins);
        if !issues.is_empty() {
            let details = issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("；");
            return Err(format!("插件配置无效：{details}"));
        }
    }
    // 记录本次保存涉及的顶层 key，方便排查配置未生效问题
    let keys: Vec<&str> = updates.as_object().map(|m| m.keys().map(String::as_str).collect()).unwrap_or_default();
    log::info!("save_config: 收到配置更新 keys={keys:?}");
    state
        .config
        .update(|c| {
            talksage_config::apply_updates(c, &updates);
        })
        .map_err(|e| format!("保存配置失败: {e}"))?;
    // 记录保存后实际生效的代理，验证 network 字段是否正确写入
    let proxy_after = state.config.snapshot().network.proxy_url().map(str::to_string).unwrap_or_default();
    log::info!("save_config: 配置已写入磁盘 proxy={proxy_after:?}");
    state.service.knowledge().invalidate();
    state.service.knowledge().refresh();
    // 录音目录可在设置页修改，保存后同步刷新 asset scope。
    allow_recording_assets(&app, &state.config)?;
    Ok(())
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
///
/// async + spawn_blocking：`service.start` 要加载模型/装配管道，可能耗时数秒；
/// 同步 command 会占住 Tauri 主线程（窗口消息循环冻结，UI 假死）。
#[tauri::command]
async fn start_listen(
    pinned_note_paths: Option<Vec<String>>,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    log::info!("收到开始实时监听请求");
    let app = app.clone();
    let service = state.service.clone();
    let running = state.running.clone();
    let cfg = state.config.snapshot();
    let (user_input, client) = match cfg.audio.audio_source.as_str() {
        // 回环模式：user 流采集扬声器输出，client 流关闭（否则两路都开回环会冲突）
        "loopback" => (AudioInput::Loopback, ClientCapture::Off),
        _ => (AudioInput::Mic(None), ClientCapture::Auto),
    };
    tauri::async_runtime::spawn_blocking(move || {
        // 检查 + start + 写入整体持锁，语义与同步版本一致；但这是在阻塞线程池
        // 里，不占主线程，窗口照常响应。
        let mut guard = running.lock().map_err(|_| "pipeline 锁失败".to_string())?;
        if guard.is_some() {
            return Err("已在监听中".into());
        }
        let req = StartListen {
            user_input,
            client,
            pinned_note_paths: pinned_note_paths.unwrap_or_default(),
            ..StartListen::desktop()
        };
        let started = service
            .start(
                req,
                Arc::new(move |ev: DomainEvent| {
                    let _ = app.emit("talksage://event", ev);
                }),
            )
            .map_err(|e| {
                log::error!("开始实时监听失败: {e:#}");
                e.to_string()
            })?;
        *guard = Some(started);
        log::info!("实时监听启动成功");
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn list_knowledge_documents(state: tauri::State<'_, AppState>) -> Vec<serde_json::Value> {
    state
        .service
        .knowledge()
        .list_documents()
        .into_iter()
        .map(|d| serde_json::json!({ "path": d.path, "title": d.title, "text": d.text }))
        .collect()
}

/// 停止实时监听。
///
/// async + spawn_blocking：`finish` 是重活（join 管道线程 ≤5s + 落库 + 双流主录音
/// 生成 + finalizer 可能网络等待），同步 command 会冻结窗口 → 前端「停止并退出」
/// 的 destroy() 永远执行不到（监听停了但程序不退、再点关闭无效）。移出主线程后
/// invoke 立即返回，前端可继续销毁窗口。
#[tauri::command]
async fn stop_listen(state: tauri::State<'_, AppState>) -> Result<(), String> {
    // 文件会话由它的完成监视器统一收尾，避免这里与自然 EOF 竞争 take/finish。
    let import_flag = state.import_cancel.lock().map_err(|_| "导入锁失败".to_string())?.clone();
    if let Some(flag) = import_flag {
        flag.store(true, Ordering::SeqCst);
        let import_cancel = state.import_cancel.clone();
        return tauri::async_runtime::spawn_blocking(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if import_cancel.lock().map_err(|_| "导入锁失败".to_string())?.is_none() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err("停止文件转写超时，请查看日志".to_string())
        }).await.map_err(|e| e.to_string())?;
    }
    let running = state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?.take();
    let Some(running) = running else {
        return Ok(());
    };
    let service = state.service.clone();
    tauri::async_runtime::spawn_blocking(move || {
        service.finish(running).map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 暂停或继续当前监听；会话与模型保持存活。
#[tauri::command]
fn set_listen_paused(paused: bool, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?;
    match guard.as_ref() {
        Some(running) => {
            running.set_paused(paused);
            Ok(())
        }
        None => Err("未在监听中".into()),
    }
}

/// 调整导入媒体的处理速度；0 表示受保护的最高速度。
#[tauri::command]
fn set_file_playback_speed(speed: f32, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?;
    match guard.as_ref() {
        Some(running) => {
            running.set_playback_speed(speed);
            log::info!("文件转写速度已调整为 {speed}x（0=极速）");
            Ok(())
        }
        None => Err("当前没有活动会话".into()),
    }
}

/// 手动触发要点聚合：通知 key_point_llm observer 立即处理当前 buffer。
#[tauri::command]
fn flush_key_points(state: tauri::State<'_, AppState>) -> Result<String, String> {
    log::info!("flush_key_points: 收到手动触发请求");
    let guard = state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?;
    match guard.as_ref() {
        Some(running) => {
            let msg = running.flush_key_points();
            log::info!("flush_key_points: {msg}");
            Ok(msg)
        }
        None => {
            log::warn!("flush_key_points: 未在监听中");
            Err("未在监听中".into())
        }
    }
}

/// 手动查询一个专业术语（用户点名要问的词，不做专业度筛选）。
///
/// 监听中走会话的事件通道（顺带入库）；未监听时直接问 LLM 并单独发事件，
/// 界面两种情况下拿到的都是同一种 Term 事件。
#[tauri::command]
fn explain_term(term: String, app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<String, String> {
    if let Ok(guard) = state.running.lock() {
        if let Some(running) = guard.as_ref() {
            return running.explain_term(&term).map_err(|e| e.to_string());
        }
    }
    let llm = TalkSageService::build_llm(&state.config)
        .ok_or_else(|| "LLM 未配置（请在设置→LLM 填写 API Key）".to_string())?;
    let content = talksage_plugins::term_explainer::lookup_term(llm.as_ref(), &term, "")
        .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "talksage://event",
        DomainEvent::Term {
            result_id: format!(
                "term-manual-{}",
                SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
            ),
            status: talksage_core::ResultStatus::Final,
            content: content.clone(),
        },
    );
    Ok(content)
}

/// 实时调节噪音电平阈值（0 = 关闭；无需停止监听，下一音频块即生效）。
#[tauri::command]
fn set_noise_level(level: f32, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?;
    match guard.as_ref() {
        Some(p) => {
            p.set_noise_level(level);
            Ok(())
        }
        None => Err("未在监听中".into()),
    }
}

/// 说话人声纹状态：模型是否可用、主人是否已注册。
#[tauri::command]
fn get_voiceprint_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
    let model_available = speaker_model_path().is_file();
    let enrolled = talksage_pipeline::speaker::owner_enrolled(state.config.data_dir());
    serde_json::json!({
        "model_available": model_available,
        "enrolled": enrolled,
    })
}

/// 注册主人声音：录制麦克风 `seconds` 秒 → 提取声纹 → 保存。
///
/// async + spawn_blocking：录制循环 + 声纹提取可能耗时数秒到十几秒，同步 command
/// 会冻结窗口（与 stop_listen 同类的"假死"）。
#[tauri::command]
async fn enroll_voice(seconds: u32, state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    // 正在监听时不允许注册（麦克风被占用）
    if state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?.is_some() {
        return Err("请先停止监听再录制声音".into());
    }
    let model = speaker_model_path();
    if !model.is_file() {
        return Err(format!("缺少声纹模型: {}", model.display()));
    }
    let gain_db = state.config.snapshot().audio.input_gain_db;
    let data_dir = state.config.data_dir().to_path_buf();
    let seconds = seconds.max(3);
    tauri::async_runtime::spawn_blocking(move || {
        let identifier = talksage_pipeline::speaker::SpeakerIdentifier::new(
            &model,
            None,
            talksage_pipeline::speaker::DEFAULT_THRESHOLD,
        )
        .ok_or("声纹模型加载失败")?;

        let (mut hub, rx) = AudioHub::new_with_gain(100, gain_db);
        hub.start(None).map_err(|e| format!("启动麦克风失败: {e}"))?;
        log::info!("声纹注册：录制 {seconds} 秒…");
        let mut audio: Vec<f32> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(seconds as u64);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(c) => audio.extend_from_slice(&c),
                Err(_) => {}
            }
        }
        hub.stop();

        let (emb, voiced_samples, windows) = identifier.enrollment_profile(&audio).ok_or(
            "有效人声不足或录音质量较差；请在安静环境连续朗读，避免长时间停顿",
        )?;
        talksage_pipeline::speaker::save_owner_embedding(&data_dir, &emb)
            .map_err(|e| format!("保存声纹失败: {e}"))?;
        log::info!("声纹注册完成: dim={} samples={}", emb.len(), audio.len());
        Ok::<serde_json::Value, String>(serde_json::json!({
            "ok": true,
            "dim": emb.len(),
            "voiced_ms": voiced_samples * 1000 / 16000,
            "windows": windows,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 删除已注册的主人声纹。
#[tauri::command]
fn remove_voiceprint(state: tauri::State<'_, AppState>) -> Result<(), String> {
    talksage_pipeline::speaker::remove_owner_embedding(state.config.data_dir())
        .map_err(|e| format!("删除声纹失败: {e}"))
}

/// 声纹模型路径（models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx）。
fn speaker_model_path() -> PathBuf {
    TalkSageService::resolve_models_dir()
        .unwrap_or_else(|| PathBuf::from("models"))
        .join("wespeaker")
        .join("wespeaker_zh_cnceleb_resnet34.onnx")
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

/// 重命名会话；空串 = 清除自定义名，列表回到 "#id · 时间"。
#[tauri::command]
fn rename_session(session_id: i64, title: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.sessions.set_session_title(session_id, &title).map_err(|e| e.to_string())
}

/// 删除会话（含段/术语/翻译）。
#[tauri::command]
fn delete_session(session_id: i64, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.sessions.delete_session(session_id).map_err(|e| e.to_string())
}

// ── AI 助手 ───────────────────────────────────────────────────────────

/// 话题列表（最近活跃在前）。
#[tauri::command]
fn list_chat_threads(state: tauri::State<'_, AppState>) -> Result<Vec<talksage_session::ChatThread>, String> {
    state.sessions.list_chat_threads(200).map_err(|e| e.to_string())
}

/// 新建话题，返回 id。
#[tauri::command]
fn create_chat_thread(state: tauri::State<'_, AppState>) -> Result<i64, String> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    state.sessions.create_chat_thread(now).map_err(|e| e.to_string())
}

/// 话题内的全部消息。
#[tauri::command]
fn get_chat_messages(
    thread_id: i64,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<talksage_session::ChatMessageRecord>, String> {
    state.sessions.get_chat_messages(thread_id).map_err(|e| e.to_string())
}

/// 重命名话题；空串 = 清除自定义名。
#[tauri::command]
fn rename_chat_thread(thread_id: i64, title: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.sessions.set_chat_thread_title(thread_id, &title).map_err(|e| e.to_string())
}

/// 删除话题及其消息。
#[tauri::command]
fn delete_chat_thread(thread_id: i64, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.sessions.delete_chat_thread(thread_id).map_err(|e| e.to_string())
}

/// 提交提问：立即返回两条消息 id，回答正文随后经 `talksage://event` 的
/// ChatDelta 逐段推送。
#[tauri::command]
fn send_chat_message(
    thread_id: i64,
    text: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let chat = state.chat.clone();
    let emit_app = app.clone();
    let emit: ChatEmit = Arc::new(move |ev: DomainEvent| {
        let _ = emit_app.emit("talksage://event", &ev);
    });
    let sent = chat.send(thread_id, &text, emit).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "user_message_id": sent.user_message_id,
        "assistant_message_id": sent.assistant_message_id,
    }))
}

/// 停止正在生成的回答（已生成的部分保留）。
#[tauri::command]
fn cancel_chat_message(message_id: i64, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.chat.cancel(message_id);
    Ok(())
}

/// 读取最近日志（调试窗口用）。
#[tauri::command]
fn read_logs(state: tauri::State<'_, AppState>, lines: Option<usize>) -> Result<String, String> {
    let n = lines.unwrap_or(200);
    let dir = talksage_logging::log_dir(Some(&state.config.data_dir().to_path_buf()));
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("读取日志目录失败: {e}"))?
        .flatten()
        .filter(|e| {
            let fname = e.file_name();
            let name = fname.to_string_lossy();
            name.starts_with("talksage") && name.ends_with(".log")
        })
        .collect();
    if files.is_empty() {
        return Ok("（暂无日志）".to_string());
    }
    files.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH));
    let latest = files.last().ok_or("无日志文件")?;
    let content = std::fs::read_to_string(latest.path()).map_err(|e| format!("读取日志失败: {e}"))?;
    let tail: Vec<&str> = content.lines().rev().take(n).collect();
    Ok(tail.iter().rev().copied().collect::<Vec<_>>().join("\n"))
}

/// 内置纪要模板列表。
#[tauri::command]
fn list_notes_templates() -> Vec<serde_json::Value> {
    talksage_notes::builtin_templates()
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "name": t.name,
                "description": t.description,
            })
        })
        .collect()
}

/// 按模板生成纪要并保存到会话。
#[tauri::command]
fn generate_notes(session_id: i64, template_id: String, state: tauri::State<'_, AppState>) -> Result<String, String> {
    let Some(llm) = TalkSageService::build_llm(&state.config) else {
        return Err("未配置 LLM（请设置 llm.providers.<provider>.api_key）".into());
    };
    let Some(template) = talksage_notes::get_template(&template_id) else {
        return Err(format!("未知模板: {template_id}"));
    };
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let knowledge = {
        let q = talksage_notes::build_knowledge_query(
            detail.title.as_deref(),
            None,
            &detail.key_points,
            &detail.segments,
        );
        state.service.knowledge().block_for_query(&q, 8)
    };
    let gen = talksage_notes::NotesGenerator::new(llm);
    let notes = gen
        .generate(&detail.segments, &detail.terms, &detail.translations, &detail.key_points, &template, &knowledge)
        .map_err(|e| format!("纪要生成失败: {e}"))?;
    state
        .sessions
        .set_notes(session_id, &notes)
        .map_err(|e| format!("保存纪要失败: {e}"))?;
    Ok(notes)
}

/// 三段式智能纪要（概述 / 归属要点 / 行动项；借鉴 Call.md summary-generator），保存到会话。
#[tauri::command]
fn generate_trio_notes(session_id: i64, meeting_name: Option<String>, meeting_description: Option<String>, state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let Some(llm) = TalkSageService::build_llm(&state.config) else {
        return Err("未配置 LLM（请设置 llm.providers.<provider>.api_key）".into());
    };
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let knowledge = {
        let q = talksage_notes::build_knowledge_query(
            meeting_name.as_deref().or(detail.title.as_deref()),
            meeting_description.as_deref(),
            &detail.key_points,
            &detail.segments,
        );
        state.service.knowledge().block_for_query(&q, 8)
    };
    let gen = talksage_notes::TrioGenerator::new(llm);
    let trio = gen
        .generate(&detail.segments, &detail.key_points, meeting_name.as_deref(), meeting_description.as_deref(), &knowledge)
        .map_err(|e| format!("智能纪要生成失败: {e}"))?;
    let json = serde_json::to_value(&trio).map_err(|e| e.to_string())?;
    state
        .sessions
        .set_trio(session_id, &json.to_string())
        .map_err(|e| format!("保存智能纪要失败: {e}"))?;
    Ok(json)
}

/// 导出会话为 Markdown 单文件（转写 + 纪要 + 指标 + 质量；借鉴 Call.md markdown-export），
/// 写入 `<data_dir>/sessions/{id}/exports/session-{id}.md` 并返回内容。
#[tauri::command]
fn export_session_markdown(session_id: i64, state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let content = talksage_session::export_markdown(&detail);
    let dir = talksage_config::session_exports_dir(state.config.data_dir(), session_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败: {e}"))?;
    let path = dir.join(format!("session-{session_id}.md"));
    std::fs::write(&path, &content).map_err(|e| format!("写入导出文件失败: {e}"))?;
    Ok(serde_json::json!({ "path": path.display().to_string(), "content": content }))
}

/// 导出会话为纯文本转写（无 Markdown 标记），写入
/// `<data_dir>/sessions/{id}/exports/session-{id}.txt` 并返回内容。
#[tauri::command]
fn export_session_text(session_id: i64, state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let content = talksage_session::export_transcript_text(&detail);
    let dir = talksage_config::session_exports_dir(state.config.data_dir(), session_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败: {e}"))?;
    let path = dir.join(format!("session-{session_id}.txt"));
    std::fs::write(&path, &content).map_err(|e| format!("写入导出文件失败: {e}"))?;
    Ok(serde_json::json!({ "path": path.display().to_string(), "content": content }))
}

/// 导出会话完整录音（master 双声道，单流时复用分轨），复制到
/// `<data_dir>/sessions/{id}/exports/session-{id}.wav` 并返回路径。无录音时返回可读错误。
#[tauri::command]
fn export_session_audio(session_id: i64, state: tauri::State<'_, AppState>) -> Result<String, String> {
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let master = detail
        .meta
        .as_ref()
        .and_then(|m| m.master_recording.clone())
        .ok_or_else(|| "该会话没有完整录音（可能未开启录音，或录音文件缺失）".to_string())?;
    let src = std::path::PathBuf::from(&master);
    if !src.is_file() {
        return Err(format!("录音文件不存在: {}", src.display()));
    }
    let dir = talksage_config::session_exports_dir(state.config.data_dir(), session_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败: {e}"))?;
    let dst = dir.join(format!("session-{session_id}.wav"));
    std::fs::copy(&src, &dst).map_err(|e| format!("复制录音失败: {e}"))?;
    Ok(dst.display().to_string())
}

/// GPU 后端状态（加速后端探测）。
#[tauri::command]
fn get_gpu_status(state: tauri::State<'_, AppState>) -> serde_json::Value {
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
            "ASR 运行状态不可用: physical_gpu={} runtime_backend={} mode={} configured_backend={} error={} note={}",
            talksage_asr::GpuBackend::hardware_candidate(),
            gpu.display_name(),
            cfg.asr.asr_mode,
            cfg.asr.backend,
            error,
            talksage_asr::GpuBackend::availability_note(),
        );
    } else {
        log::info!(
            "ASR 运行状态: physical_gpu={} runtime_backend={} accelerated={} route={} note={}",
            talksage_asr::GpuBackend::hardware_candidate(),
            gpu.display_name(),
            gpu.is_accelerated(),
            route.as_ref().map(|r| r.display_name()).unwrap_or("unknown"),
            talksage_asr::GpuBackend::availability_note(),
        );
    }
    serde_json::json!({
        "backend": gpu.provider_str(),
        "display_name": gpu.display_name(),
        "hardware_candidate": talksage_asr::GpuBackend::hardware_candidate(),
        "availability_note": talksage_asr::GpuBackend::availability_note(),
        "is_accelerated": gpu.is_accelerated(),
        "effective_route": route.as_ref().ok().map(|r| r.display_name()),
        "route_error": route_error,
    })
}

/// 验证阿里云 ASR 凭据（设置页「检查」按钮）：向阿里云 NLS 请求一个
/// AccessToken（CreateToken，HMAC-SHA1 签名）。成功返回 token 有效期秒数，
/// 失败返回可读错误（InvalidAccessKeyId / SignatureDoesNotMatch 等）。
/// 支持传入表单未保存的覆盖值（留 None 用已保存配置）。
#[tauri::command]
async fn test_aliyun_asr(
    access_key_id: Option<String>,
    access_key_secret: Option<String>,
    app_key: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let cfg = state.config.snapshot();
    let key_id = access_key_id.unwrap_or(cfg.asr.aliyun_access_key_id.clone());
    let key_secret = access_key_secret.unwrap_or(cfg.asr.aliyun_access_key_secret.clone());
    let app_key = app_key.unwrap_or(cfg.asr.aliyun_app_key.clone());
    if key_id.trim().is_empty() || key_secret.trim().is_empty() {
        return Err("请先填写 AccessKey ID 和 AccessKey Secret".into());
    }
    let key_id = key_id.trim().to_string();
    let key_secret = key_secret.trim().to_string();
    let expire = talksage_asr::aliyun::verify_aliyun_credentials(&key_id, &key_secret)
        .await
        .map_err(|e| format!("阿里云 ASR 验证失败: {e}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let valid_for = expire.saturating_sub(now);
    log::info!("阿里云 ASR 凭据验证通过: token 有效期剩余 {valid_for}s app_key={app_key}");
    Ok(serde_json::json!({
        "ok": true,
        "expire_at": expire,
        "valid_for_secs": valid_for,
        "app_key": app_key,
    }))
}

/// 打开系统文件对话框，选择支持的录音/会议媒体文件。
#[tauri::command]
fn pick_audio_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("录音与会议媒体", &["wav", "mp3", "mp4", "m4a"])
        .set_title("选择录音或会议媒体文件")
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned())
}

/// 打开系统文件夹对话框，选择知识库 / Obsidian 仓库目录。用户取消时返回 null。
#[tauri::command]
fn pick_folder() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("选择 Obsidian 仓库或知识库文件夹")
        .pick_folder()
        .map(|p| p.to_string_lossy().into_owned())
}

/// 启动本地媒体会话。它与麦克风会话共用事件、暂停、分析插件和落库链路；
/// 命令在管线启动后立即返回 session_id，完成结果由 MediaCompleted 推送。
#[tauri::command]
async fn start_file_import(
    path: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<i64, String> {
    // 实时监听中禁止同时导入
    {
        let g = state.running.lock().map_err(|_| "pipeline 锁失败".to_string())?;
        if g.is_some() {
            return Err("实时监听中，请先停止再导入文件".into());
        }
    }
    // 已有导入任务在跑
    {
        let g = state.import_cancel.lock().map_err(|_| "导入锁失败".to_string())?;
        if g.is_some() {
            return Err("已有文件在导入中".into());
        }
    }

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut g = state.import_cancel.lock().map_err(|_| "导入锁失败".to_string())?;
        *g = Some(cancel.clone());
    }

    let service = state.service.clone();
    let path_buf = PathBuf::from(&path);
    let running_slot = state.running.clone();
    let import_slot = state.import_cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let done = Arc::new(AtomicBool::new(false));
        let done_for_events = done.clone();
        let event_app = app.clone();
        let started = match service.start(
            StartListen::import_file(path_buf, "说话人".into()),
            Arc::new(move |ev: DomainEvent| {
                if matches!(&ev, DomainEvent::Status { stage: StatusStage::Idle, .. }) {
                    done_for_events.store(true, Ordering::SeqCst);
                }
                let _ = event_app.emit("talksage://event", &ev);
            }),
        ) {
            Ok(value) => value,
            Err(error) => {
                *import_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
                log::error!("启动文件转写失败 {}: {error:#}", path);
                return Err(error.to_string());
            }
        };
        let sid = match started.session_id() {
            Some(sid) => sid,
            None => {
                let _ = service.finish(started);
                *import_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return Err("文件会话未创建".to_string());
            }
        };
        *running_slot.lock().map_err(|_| "pipeline 锁失败".to_string())? = Some(started);

        let monitor_service = service.clone();
        let monitor_running = running_slot.clone();
        let monitor_import = import_slot.clone();
        let monitor_app = app.clone();
        let monitor_result = std::thread::Builder::new().name("talksage-file-session".into()).spawn(move || {
            while !done.load(Ordering::SeqCst) && !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let cancelled = cancel.load(Ordering::SeqCst);
            let running = monitor_running.lock().unwrap_or_else(|e| e.into_inner()).take();
            let result = running.map(|r| monitor_service.finish(r)).transpose();
            *monitor_import.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let (session_id, error) = match result {
                Ok(id) => (id.flatten(), None),
                Err(error) => {
                    log::error!("文件会话收尾失败: {error:#}");
                    (None, Some(error.to_string()))
                }
            };
            let _ = monitor_app.emit("talksage://event", DomainEvent::MediaCompleted {
                session_id,
                cancelled,
                error,
            });
        });
        if let Err(error) = monitor_result {
            if let Some(running) = running_slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = service.finish(running);
            }
            *import_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(format!("创建文件会话监视器失败: {error}"));
        }
        Ok::<i64, String>(sid)
    }).await.map_err(|e| e.to_string())?
}

/// 整理会中已落库要点（历史详情；无 LLM 时返回错误，前端提示）。
#[tauri::command]
fn generate_highlights(session_id: i64, state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    let Some(llm) = TalkSageService::build_llm(&state.config) else {
        return Err("未配置 LLM（请设置 llm.providers.<provider>.api_key）".into());
    };
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    talksage_notes::generate_highlights(&detail.key_points, &detail.segments, &llm).map_err(|e| format!("要点整理失败: {e}"))
}

/// 验证 LLM 连接（设置页「检查」按钮）：用指定 provider（可带表单未保存的
/// 覆盖值）发一个最小请求，返回 401/网络等可读错误。不写入配置。
#[tauri::command]
fn test_llm(
    provider: String,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let snapshot = state.config.snapshot();
    let cfg = snapshot
        .llm
        .providers
        .get(&provider)
        .ok_or_else(|| format!("未知 provider: {provider}"))?;
    let proxy = snapshot.network.proxy_url().map(str::to_string);
    let llm = talksage_llm::OpenAICompatProvider::new(
        api_key
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| cfg.api_key.clone()),
        model.unwrap_or_else(|| cfg.model.clone()),
        base_url.unwrap_or_else(|| cfg.base_url.clone().unwrap_or_else(|| "https://api.deepseek.com/v1".to_string())),
    ).with_proxy(proxy);
    llm.test_connection().map_err(|e| format!("LLM 检查失败: {e}"))
}

/// 代理连通性测试：向目标地址发 HEAD 请求，检查代理是否可达。
#[tauri::command]
fn test_proxy(proxy_url: String, _state: tauri::State<'_, AppState>) -> Result<String, String> {
    if proxy_url.trim().is_empty() {
        return Err("代理地址不能为空".into());
    }
    let proxy_cfg = ureq::Proxy::new(&proxy_url).map_err(|e| format!("代理地址格式错误: {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .proxy(proxy_cfg)
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(10))
        .build();
    match agent.head("https://www.google.com").call() {
        Ok(resp) => Ok(format!("代理可用（HTTP {}）", resp.status())),
        Err(e) => Err(format!("代理测试失败: {e}")),
    }
}

// suppress unused import warning when test_proxy is the only user
#[allow(unused_imports)]
use std::sync::Arc as _Arc;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // whisper.cpp 1.8.3 在 macOS 15+ 的 residency-set 全局析构阶段可能因
    // Tauri `process::exit` 跳过 Rust Drop 而触发 GGML_ASSERT/SIGABRT。该优化
    // 只负责延长资源驻留，不决定是否使用 Metal；禁用后 Metal/融合/并发仍启用。
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    std::env::set_var("GGML_METAL_NO_RESIDENCY", "1");
    let config = Arc::new(ConfigManager::load(None, None).expect("加载配置失败"));
    let data_dir = config.data_dir().to_path_buf();
    let _log_guard = talksage_logging::init(Some(&data_dir));
    log::info!("TalkSage 桌面应用启动，数据目录: {}", data_dir.display());
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    log::info!("Metal 稳定性保护已启用: GGML_METAL_NO_RESIDENCY=1");
    let startup_gpu = talksage_asr::GpuBackend::detect();
    log::info!(
        "启动时 ASR 硬件诊断: physical_gpu={} runtime_backend={} accelerated={} note={}",
        talksage_asr::GpuBackend::hardware_candidate(),
        startup_gpu.display_name(),
        startup_gpu.is_accelerated(),
        talksage_asr::GpuBackend::availability_note(),
    );
    let sessions = Arc::new(
        SessionStore::open(&data_dir.join("sessions.db").to_string_lossy()).expect("打开会话库失败"),
    );
    let service = TalkSageService::new(config.clone(), Some(sessions.clone()), EnginePool::new());
    // 上次异常退出的残留（未完成录音 + 未结束会话），在窗口起来前先收拾干净。
    service.recover_on_startup();
    let chat = Arc::new(ChatService::with_knowledge(
        config.clone(),
        sessions.clone(),
        service.knowledge(),
    ));

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState {
            config: config.clone(),
            sessions: sessions.clone(),
            service,
            chat,
            running: Arc::new(Mutex::new(None)),
            downloads: Arc::new(Mutex::new(std::collections::HashMap::new())),
            import_cancel: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            get_version,
            get_config,
            list_plugins,
            list_plugin_status,
            list_asr_models,
            download_model,
            cancel_model_download,
            remove_model,
            test_proxy,
            save_config,
            ping,
            start_listen,
            list_knowledge_documents,
            stop_listen,
            set_listen_paused,
            set_file_playback_speed,
            flush_key_points,
            set_noise_level,
            get_voiceprint_status,
            enroll_voice,
            remove_voiceprint,
            minimize_to_tray,
            quit_app,
            list_sessions,
            search_sessions,
            get_session,
            rename_session,
            delete_session,
            explain_term,
            list_chat_threads,
            create_chat_thread,
            get_chat_messages,
            rename_chat_thread,
            delete_chat_thread,
            send_chat_message,
            cancel_chat_message,
            list_notes_templates,
            generate_notes,
            generate_trio_notes,
            export_session_markdown,
            export_session_text,
            export_session_audio,
            generate_highlights,
            test_llm,
            read_logs,
            get_gpu_status,
            test_aliyun_asr,
            pick_audio_file,
            pick_folder,
            start_file_import,
            updater::check_for_updates,
            updater::pick_upgrade_package,
            updater::install_offline_upgrade
        ])
        .setup(move |app| {
            if let Err(e) = std::fs::create_dir_all(&data_dir) {
                eprintln!("创建数据目录失败 {}: {e}", data_dir.display());
            }
            if let Err(e) = allow_recording_assets(app.handle(), &app.state::<AppState>().config) {
                log::error!("{e}");
            }
            // 窗口偏好：恢复上次的位置/尺寸（物理像素），并在拖动/缩放时持久化（节流 1s）。
            // 注意：保存/恢复均为物理单位，避免高 DPI 下逻辑→物理转换导致窗口巨大。
            let win_path = data_dir.join("window.json");
            if let Some(window) = app.get_webview_window("main") {
                if let Some(mut ws) = window_state::load(&win_path) {
                    // 钳制到主显示器工作区（防止异常保存值/DPI 变化导致窗口超出屏幕）
                    if let Ok(Some(m)) = app.primary_monitor() {
                        let size = m.size();
                        let pos = m.position();
                        window_state::clamp_to_work_area(&mut ws, (size.width, size.height), (pos.x, pos.y));
                    }
                    let _ = window.set_position(tauri::PhysicalPosition::new(ws.x, ws.y));
                    let _ = window.set_size(tauri::PhysicalSize::new(ws.width, ws.height));
                }
                let win = window.clone();
                static LAST_SAVE: AtomicU64 = AtomicU64::new(0);
                window.on_window_event(move |event| {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if now.saturating_sub(LAST_SAVE.load(Ordering::Relaxed)) < 1 {
                        return; // 节流：每秒最多写一次
                    }
                    // 最大化/全屏状态不保存：保持上次的正常窗口尺寸
                    if win.is_maximized().unwrap_or(false) || win.is_fullscreen().unwrap_or(false) {
                        return;
                    }
                    let (pos, size) = match event {
                        WindowEvent::Resized(s) => (win.outer_position().ok(), Some(*s)),
                        WindowEvent::Moved(p) => (Some(*p), win.outer_size().ok()),
                        _ => (None, None),
                    };
                    if let (Some(p), Some(s)) = (pos, size) {
                        let mut ws = window_state::WindowState {
                            x: p.x,
                            y: p.y,
                            width: s.width,
                            height: s.height,
                        };
                        if ws.is_valid() {
                            // 钳制到当前显示器工作区（防止保存到屏幕外/超大的值）
                            if let Ok(Some(m)) = win.current_monitor() {
                                let msize = m.size();
                                let mpos = m.position();
                                window_state::clamp_to_work_area(&mut ws, (msize.width, msize.height), (mpos.x, mpos.y));
                            }
                            let _ = window_state::save(&win_path, &ws);
                            LAST_SAVE.store(now, Ordering::Relaxed);
                        }
                    }
                });
            }
            // 系统托盘 / 菜单栏图标（Windows 右下角托盘；macOS 菜单栏状态项，遵循各平台惯例）
            let tray_icon = app
                .default_window_icon()
                .map(|i| i.clone())
                .unwrap_or_else(|| tauri::image::Image::new_owned(vec![0, 0, 0, 0], 1, 1));
            let show_item = MenuItem::with_id(app, "show", "显示 / 隐藏窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            let tray = TrayIconBuilder::with_id("main-tray")
                .icon(tray_icon)
                .tooltip("拓思者 · AI 会议助理")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键单击：切换窗口显示/隐藏
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            // 持有句柄，防止托盘图标被销毁
            app.manage(tray);
            log::info!("系统托盘图标已就绪（Windows 托盘 / macOS 菜单栏）");

            let _ = app.emit(
                "talksage://event",
                DomainEvent::Status {
                    stage: StatusStage::Starting,
                    message: "TalkSage 已启动".into(),
                },
            );
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building TalkSage");
    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            prepare_exit(app_handle);
        }
    });
}

/// Tauri `App::run` 最终使用 `process::exit`，不会执行 managed state 的 Drop。
/// 因此退出事件中显式停止管道并释放 Metal 模型；幂等，兼容托盘和前端退出。
fn prepare_exit(app: &AppHandle) {
    static EXIT_CLEANUP_STARTED: AtomicBool = AtomicBool::new(false);
    if EXIT_CLEANUP_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    log::info!("应用退出清理开始");
    let state = app.state::<AppState>();
    let running = state.running.lock().ok().and_then(|mut guard| guard.take());
    if let Some(running) = running {
        if let Err(error) = state.service.finish(running) {
            log::error!("应用退出时停止监听失败: {error:#}");
        }
    }
    state.service.clear_engines();
    log::info!("应用退出清理完成");
}

/// 显示并聚焦主窗口（从托盘/菜单栏恢复）。
fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 隐藏主窗口到托盘（Windows：前端检测到最小化后调用）。
#[tauri::command]
fn minimize_to_tray(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        w.hide().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

/// 退出应用（不依赖窗口 close/destroy 的 ACL 权限，最可靠的退出路径）。
///
/// 前端「停止并退出」确认后调用；`exit(0)` 直接结束进程，绕过
/// close-requested 守卫，也不会因 window destroy 权限缺失而卡住。
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// 切换主窗口显示/隐藏（托盘左键点击）。
fn toggle_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let visible = w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false);
        if visible {
            let _ = w.hide();
        } else {
            show_main_window(app);
        }
    }
}
