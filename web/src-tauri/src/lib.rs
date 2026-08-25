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
use talksage_pipeline::{RunningListen, StartListen, TalkSageService};
use talksage_asr::{EngineKind, EnginePool};
use talksage_session::SessionStore;

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
    downloads: Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
}

/// 版本。
#[tauri::command]
fn get_version() -> String {
    talksage_core::VERSION.to_string()
}

/// 配置快照。
///
/// `plugins` 单独换成**生效配置**（插件默认值 + 用户覆盖）。原样序列化的话
/// 通用表里只有用户显式写过的插件，设置页读 `plugins.<id>.enabled` 会拿到
/// undefined —— 默认值归插件所有，宿主在出口处替前端补齐。
#[tauri::command]
fn get_config(state: tauri::State<'_, AppState>) -> serde_json::Value {
    let snapshot = state.config.snapshot();
    let mut value =
        serde_json::to_value(&snapshot).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        let mut plugins =
            talksage_plugins::effective_plugin_configs(&snapshot.plugins.entries);
        plugins.insert(
            "notes".into(),
            serde_json::json!({ "template": snapshot.plugins.notes.template }),
        );
        obj.insert("plugins".into(), serde_json::Value::Object(plugins));
    }
    value
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

/// 把配置解析后的真实录音目录加入 asset 协议只读范围。
/// 开发脚本会把数据放在仓库 `.tools/data`，它不属于 Tauri `$DATA_DIR`。
fn allow_recording_assets(app: &tauri::AppHandle, config: &ConfigManager) -> Result<(), String> {
    let dir = config.snapshot().recording.resolve_dir(config.data_dir());
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建录音目录失败 {}: {e}", dir.display()))?;
    app.asset_protocol_scope()
        .allow_directory(&dir, false)
        .map_err(|e| format!("授权录音播放目录失败 {}: {e}", dir.display()))?;
    log::info!("历史录音播放目录已授权: {}", dir.display());
    Ok(())
}

/// ASR 模型目录：包含未安装模型，前端据此禁用选项并展示速度取舍。
#[tauri::command]
fn list_asr_models() -> Vec<serde_json::Value> {
    let root = TalkSageService::resolve_models_dir();
    EngineKind::ALL
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
                "installed": root.as_ref().is_some_and(|r| kind.is_available(r)),
                "size_mb": root.as_ref().map(|r| talksage_asr::models::installed_size_mb(kind, r)).unwrap_or(0),
                "download_size_mb": talksage_asr::models::download_size_mb(kind),
                "downloading": root.as_ref().is_some_and(|r| talksage_asr::models::is_downloading(kind, r)),
            })
        })
        .collect()
}

/// 下载/安装 ASR 引擎（后台线程；进度经 `talksage://event` 推送 ModelProgress）。
/// 下载期间注册到 `state.downloads`，可用 [`cancel_model_download`] 取消。
#[tauri::command]
async fn download_model(
    engine: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let kind = EngineKind::from_name(&engine).ok_or_else(|| format!("未知引擎: {engine}"))?;
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
    tauri::async_runtime::spawn_blocking(move || {
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
        let progress = move |received: u64, total: u64| {
            let percent = if total > 0 { ((received as f64 / total as f64) * 100.0) as u32 } else { 0 };
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
        let result = talksage_asr::models::download_engine(kind, &root, Some(&progress), Some(&cancel_flag));
        match result {
            Ok(()) => {
                emit("done", 100, "安装完成");
                Ok(())
            }
            Err(e) => {
                // 用户主动取消：发"已取消"而非"失败"
                if e.downcast_ref::<talksage_asr::models::DownloadCancelled>().is_some() {
                    emit("cancelled", 0, "已取消");
                    Ok(())
                } else {
                    emit("error", 0, &e.to_string());
                    Err(e.to_string())
                }
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|_| {
        // 无论成功/失败/取消，下载结束都要从注册表移除
        if let Ok(mut dl) = state.downloads.lock() {
            dl.remove(&cleanup_engine);
        }
    })
}

/// 取消正在进行的模型下载（置位取消标志；下载线程会尽快停止并清理临时文件）。
#[tauri::command]
fn cancel_model_download(engine: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let dl = state.downloads.lock().map_err(|_| "下载注册表锁失败".to_string())?;
    match dl.get(&engine) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => Err("该模型没有正在进行的下载".into()),
    }
}

/// 删除 ASR 引擎模型目录。
#[tauri::command]
fn remove_model(engine: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let kind = EngineKind::from_name(&engine).ok_or_else(|| format!("未知引擎: {engine}"))?;
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
    state
        .config
        .update(|c| {
            apply_config_updates(c, &updates);
        })
        .map_err(|e| format!("保存配置失败: {e}"))?;
    // 录音目录可在设置页修改，保存后同步刷新 asset scope。
    allow_recording_assets(&app, &state.config)?;
    Ok(())
}

/// 把前端提交的更新应用到配置。
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
        // 通用表：逐插件逐键合并，宿主不认识具体插件的配置结构。
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
            if let Some(t) = vad.get("threshold").and_then(|v| v.as_f64()) {
                c.audio.vad.threshold = Some(t as f32);
            }
        }
        if let Some(d) = audio.get("denoise") {
            if let Some(e) = d.get("enabled").and_then(|v| v.as_bool()) {
                c.audio.denoise.enabled = e;
            }
            if let Some(g) = d.get("gate_threshold").and_then(|v| v.as_f64()) {
                c.audio.denoise.gate_threshold = g as f32;
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
    if let Some(rec) = updates.get("recording") {
        if let Some(e) = rec.get("enabled").and_then(|v| v.as_bool()) {
            c.recording.enabled = e;
        }
        if let Some(d) = rec.get("dir").and_then(|v| v.as_str()) {
            c.recording.dir = d.to_string();
        }
        if let Some(cs) = rec.get("clean_silence").and_then(|v| v.as_bool()) {
            c.recording.clean_silence = cs;
        }
    }
    // quality：null → 恢复默认；否则按字段更新
    match updates.get("quality") {
        Some(serde_json::Value::Null) => {
            c.quality = talksage_config::QualityConfig::default();
        }
        Some(q) => {
            if let Some(a) = q.get("auto_detect").and_then(|v| v.as_bool()) {
                c.quality.auto_detect = a;
            }
            if let Some(t) = q.get("text_noise_threshold").and_then(|v| v.as_f64()) {
                c.quality.text_noise_threshold = t as f32;
            }
            if let Some(v) = q.get("min_speech_ratio").and_then(|v| v.as_f64()) {
                c.quality.min_speech_ratio = v as f32;
            }
            if let Some(v) = q.get("max_speech_ratio").and_then(|v| v.as_f64()) {
                c.quality.max_speech_ratio = v as f32;
            }
            if let Some(v) = q.get("silence_rms").and_then(|v| v.as_f64()) {
                c.quality.silence_rms = v as f32;
            }
            if let Some(v) = q.get("high_rms").and_then(|v| v.as_f64()) {
                c.quality.high_rms = v as f32;
            }
        }
        None => {}
    }
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
async fn start_listen(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let app = app.clone();
    let service = state.service.clone();
    let running = state.running.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // 检查 + start + 写入整体持锁，语义与同步版本一致；但这是在阻塞线程池
        // 里，不占主线程，窗口照常响应。
        let mut guard = running.lock().map_err(|_| "pipeline 锁失败".to_string())?;
        if guard.is_some() {
            return Err("已在监听中".into());
        }
        let started = service
            .start(
                StartListen::desktop(),
                Arc::new(move |ev: DomainEvent| {
                    let _ = app.emit("talksage://event", ev);
                }),
            )
            .map_err(|e| e.to_string())?;
        *guard = Some(started);
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 停止实时监听。
///
/// async + spawn_blocking：`finish` 是重活（join 管道线程 ≤5s + 落库 + 双流主录音
/// 生成 + finalizer 可能网络等待），同步 command 会冻结窗口 → 前端「停止并退出」
/// 的 destroy() 永远执行不到（监听停了但程序不退、再点关闭无效）。移出主线程后
/// invoke 立即返回，前端可继续销毁窗口。
#[tauri::command]
async fn stop_listen(state: tauri::State<'_, AppState>) -> Result<(), String> {
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

/// 删除会话（含段/术语/翻译）。
#[tauri::command]
fn delete_session(session_id: i64, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.sessions.delete_session(session_id).map_err(|e| e.to_string())
}

/// 读取最近日志（调试窗口用）。
#[tauri::command]
fn read_logs(state: tauri::State<'_, AppState>, lines: Option<usize>) -> Result<String, String> {
    let n = lines.unwrap_or(200);
    let dir = talksage_logging::log_dir(Some(&state.config.data_dir().to_path_buf()));
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("读取日志目录失败: {e}"))?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("talksage.log"))
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
    let gen = talksage_notes::NotesGenerator::new(llm);
    let notes = gen
        .generate(&detail.segments, &detail.terms, &detail.translations, &detail.key_points, &template)
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
    let gen = talksage_notes::TrioGenerator::new(llm);
    let trio = gen
        .generate(&detail.segments, &detail.key_points, meeting_name.as_deref(), meeting_description.as_deref())
        .map_err(|e| format!("智能纪要生成失败: {e}"))?;
    let json = serde_json::to_value(&trio).map_err(|e| e.to_string())?;
    state
        .sessions
        .set_trio(session_id, &json.to_string())
        .map_err(|e| format!("保存智能纪要失败: {e}"))?;
    Ok(json)
}

/// 导出会话为 Markdown 单文件（转写 + 纪要 + 指标 + 质量；借鉴 Call.md markdown-export），
/// 写入 `<data_dir>/exports/session-{id}.md` 并返回内容。
#[tauri::command]
fn export_session_markdown(session_id: i64, state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let content = talksage_session::export_markdown(&detail);
    let dir = state.config.data_dir().join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败: {e}"))?;
    let path = dir.join(format!("session-{session_id}.md"));
    std::fs::write(&path, &content).map_err(|e| format!("写入导出文件失败: {e}"))?;
    Ok(serde_json::json!({ "path": path.display().to_string(), "content": content }))
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = Arc::new(ConfigManager::load(None, None).expect("加载配置失败"));
    let data_dir = config.data_dir().to_path_buf();
    let _log_guard = talksage_logging::init(Some(&data_dir));
    log::info!("TalkSage 桌面应用启动，数据目录: {}", data_dir.display());
    let sessions = Arc::new(
        SessionStore::open(&data_dir.join("sessions.db").to_string_lossy()).expect("打开会话库失败"),
    );
    let service = TalkSageService::new(config.clone(), Some(sessions.clone()), EnginePool::new());
    // 上次异常退出的残留（未完成录音 + 未结束会话），在窗口起来前先收拾干净。
    service.recover_on_startup();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            config,
            sessions,
            service,
            running: Arc::new(Mutex::new(None)),
            downloads: Mutex::new(std::collections::HashMap::new()),
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
            save_config,
            ping,
            start_listen,
            stop_listen,
            set_listen_paused,
            set_noise_level,
            get_voiceprint_status,
            enroll_voice,
            remove_voiceprint,
            minimize_to_tray,
            quit_app,
            list_sessions,
            search_sessions,
            get_session,
            delete_session,
            list_notes_templates,
            generate_notes,
            generate_trio_notes,
            export_session_markdown,
            generate_highlights,
            read_logs
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
        .run(tauri::generate_context!())
        .expect("error while running TalkSage");
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
