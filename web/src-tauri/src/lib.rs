//! TalkSage v2 — Tauri 适配器。
//!
//! 职责：把 Rust 核心域暴露给前端（React SPA）：
//!   - command：get_version / get_config / ping / start_listen / stop_listen
//!   - event：领域事件推送（talksage://event 通道，含实时转写）
//!
//! 这是"可插拔传输适配器"之一；删除本 crate 即回到纯 headless（M4 预留 axum 适配器）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sherpa_onnx::{
    SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use talksage_audio::AudioHub;
use talksage_config::ConfigManager;
use talksage_core::{DomainEvent, ResultStatus, StatusStage};
use talksage_pipeline::{AudioInput, LivePipeline, LivePipelineConfig, StreamConfig};
use talksage_asr::{EngineKind, EnginePool};
use talksage_llm::{LLMProvider, OpenAICompatProvider};
use talksage_plugins::{brief_retriever::BriefRetrieverPlugin, term_explainer::TermExplainerPlugin, translator::TranslatorPlugin, PluginContext};
use talksage_session::SessionStore;

mod window_state;

/// 应用状态（Tauri managed state）。
pub struct AppState {
    config: ConfigManager,
    /// 当前监听管道（None = 未监听）。
    pipeline: Mutex<Option<LivePipeline>>,
    /// 会话存储（常驻 SQLite）。
    sessions: Arc<SessionStore>,
    /// 当前会话 id（监听期间有效）。
    current_session: Arc<Mutex<Option<i64>>>,
    /// 当前会话的流统计（SessionStats 事件收集，stop 时评估质量落库）。
    session_stats: Arc<Mutex<Vec<talksage_session::StreamMeta>>>,
    /// 当前会话 final 段文本（质量评估用）。
    session_texts: Arc<Mutex<Vec<String>>>,
    /// ASR 引擎池（常驻，跨监听复用 → 热启动）。
    engine_pool: Arc<talksage_asr::EnginePool>,
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

/// 保存配置（前端设置面板提交，写入 talksage.toml）。
#[tauri::command]
fn save_config(updates: serde_json::Value, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .config
        .update(|c| {
            apply_config_updates(c, &updates);
        })
        .map_err(|e| format!("保存配置失败: {e}"))?;
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
        if let Some(t) = plugins.get("term_explainer") {
            if let Some(e) = t.get("enabled").and_then(|v| v.as_bool()) {
                c.plugins.term_explainer.enabled = e;
            }
            if let Some(cd) = t.get("cooldown_seconds").and_then(|v| v.as_f64()) {
                c.plugins.term_explainer.cooldown_seconds = cd as f32;
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
        // 最短提交时长（ms）：0/null = 不限制
        if let Some(m) = audio.get("min_segment_ms") {
            if let Some(v) = m.as_u64() {
                c.audio.min_segment_ms = if v == 0 { None } else { Some(v) };
            } else if m.is_null() {
                c.audio.min_segment_ms = None;
            }
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

    let snapshot = state.config.snapshot();
    // 录音目录（配置开启时，监听期间保存原始音频）
    let recording_dir = if snapshot.recording.enabled {
        let dir = snapshot.recording.resolve_dir(state.config.data_dir());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("创建录音目录失败（本次不录音）: {e}");
            None
        } else {
            Some(dir)
        }
    } else {
        None
    };
    let cfg = LivePipelineConfig {
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
        recording_dir,
        runtime: Arc::new(talksage_pipeline::RuntimeParams::default()),
        // 说话人识别：wespeaker 模型 + 已注册的主人声纹
        speaker: {
            let model_dir = model_dir.clone();
            let spk_model = model_dir.join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx");
            if spk_model.is_file() {
                let owner = talksage_pipeline::speaker::load_owner_embedding(state.config.data_dir());
                Some(talksage_pipeline::SpeakerConfig {
                    model: spk_model,
                    owner_embedding: owner,
                    threshold: talksage_pipeline::speaker::DEFAULT_THRESHOLD,
                })
            } else {
                None
            }
        },
        // 引擎池：常驻复用（热启动，参考 WhisperLiveKit 引擎单例）
        engine_pool: Some(state.engine_pool.clone()),
        // 最短提交时长：短段丢弃（噪音短段抑制）
        min_commit_ms: snapshot.audio.min_segment_ms.unwrap_or(0),
    };

    let mut pipeline = LivePipeline::new(cfg);
    let sessions = state.sessions.clone();
    let current_session = state.current_session.clone();
    // 统计收集（会话质量评估）
    *state.session_stats.lock().unwrap() = Vec::new();
    *state.session_texts.lock().unwrap() = Vec::new();
    let session_stats = state.session_stats.clone();
    let session_texts = state.session_texts.clone();
    let sink: Arc<dyn Fn(DomainEvent) + Send + Sync> = Arc::new(move |ev: DomainEvent| {
        // 会话落库（监听期间）
        if let Ok(guard) = current_session.lock() {
            if let Some(sid) = *guard {
                match &ev {
                    DomainEvent::Segment { text, is_partial: false, speaker_id, speaker_label, ts_ms, duration_ms, rms, .. } => {
                        if let Ok(mut t) = session_texts.lock() {
                            t.push(text.clone());
                        }
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
        // 会话统计收集
        if let DomainEvent::SessionStats {
            speaker_label,
            total_ms,
            speech_ms,
            final_segments,
            samples: _,
            avg_rms,
            max_rms,
            non_speech_avg_rms,
            recording,
            vad_preset,
            vad_threshold,
            words,
            questions,
        } = &ev
        {
            if let Ok(mut sm) = session_stats.lock() {
                sm.push(talksage_session::StreamMeta {
                    speaker_label: speaker_label.clone(),
                    total_ms: *total_ms,
                    speech_ms: *speech_ms,
                    final_segments: *final_segments,
                    avg_rms: *avg_rms,
                    max_rms: *max_rms,
                    non_speech_avg_rms: *non_speech_avg_rms,
                    recording: recording.clone(),
                    vad_preset: vad_preset.clone(),
                    vad_threshold: *vad_threshold,
                    words: *words,
                    questions: *questions,
                });
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
    }    // 结束会话
    if let Some(sid) = state.current_session.lock().unwrap().take() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let _ = state.sessions.end_session(sid, now);
        // 质量评估落库（stats + 段文本）
        let stats = state.session_stats.lock().unwrap().clone();
        let texts = state.session_texts.lock().unwrap().clone();
        if !stats.is_empty() {
            let params = talksage_session::QualityParams::from_config(&state.config.snapshot().quality);
            let meta = talksage_session::SessionMeta::evaluate(stats, &texts, now, &params);
            if let Err(e) = state.sessions.set_session_meta(sid, &meta) {
                log::warn!("保存会话元数据失败: {e}");
            }
            log::info!(
                "会话 #{sid} 质量评估: {}（时长 {}s，语音占比 {:.0}%，文本噪音 {:.2}，跳过下游分析={}）",
                meta.quality_label(),
                meta.duration_ms / 1000,
                meta.speech_ratio * 100.0,
                meta.text_noise,
                meta.skipped_analysis,
            );
        }
    }
    Ok(())
}

/// 实时调节噪音电平阈值（0 = 关闭；无需停止监听，下一音频块即生效）。
#[tauri::command]
fn set_noise_level(level: f32, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state.pipeline.lock().map_err(|_| "pipeline 锁失败".to_string())?;
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
#[tauri::command]
fn enroll_voice(seconds: u32, state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    // 正在监听时不允许注册（麦克风被占用）
    if state.pipeline.lock().unwrap().is_some() {
        return Err("请先停止监听再录制声音".into());
    }
    let model = speaker_model_path();
    if !model.is_file() {
        return Err(format!("缺少声纹模型: {}", model.display()));
    }
    let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
        model: Some(model.to_string_lossy().into()),
        num_threads: 1,
        debug: false,
        provider: Some("cpu".into()),
    })
    .ok_or("声纹模型加载失败")?;

    let (mut hub, rx) = AudioHub::new(100);
    hub.start(None).map_err(|e| format!("启动麦克风失败: {e}"))?;
    log::info!("声纹注册：录制 {} 秒…", seconds);
    let mut audio: Vec<f32> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(seconds.max(3) as u64);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(c) => audio.extend_from_slice(&c),
            Err(_) => {}
        }
    }
    hub.stop();

    let stream = extractor.create_stream().ok_or("创建声纹流失败")?;
    stream.accept_waveform(16000, &audio);
    if !extractor.is_ready(&stream) {
        return Err("采集到的音频太短，无法提取声纹（请保持安静并正常说话）".into());
    }
    let emb = extractor.compute(&stream).ok_or("声纹提取失败")?;
    talksage_pipeline::speaker::save_owner_embedding(state.config.data_dir(), &emb)
        .map_err(|e| format!("保存声纹失败: {e}"))?;
    log::info!("声纹注册完成: dim={} samples={}", emb.len(), audio.len());
    Ok(serde_json::json!({ "ok": true, "dim": emb.len() }))
}

/// 删除已注册的主人声纹。
#[tauri::command]
fn remove_voiceprint(state: tauri::State<'_, AppState>) -> Result<(), String> {
    talksage_pipeline::speaker::remove_owner_embedding(state.config.data_dir())
        .map_err(|e| format!("删除声纹失败: {e}"))
}

/// 声纹模型路径（models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx）。
fn speaker_model_path() -> PathBuf {
    resolve_models_dir().join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx")
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
    let Some(llm) = build_llm(&state.config) else {
        return Err("未配置 LLM（请设置 llm.providers.<provider>.api_key）".into());
    };
    let Some(template) = talksage_notes::get_template(&template_id) else {
        return Err(format!("未知模板: {template_id}"));
    };
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let gen = talksage_notes::NotesGenerator::new(llm);
    let notes = gen
        .generate(&detail.segments, &detail.terms, &detail.translations, &template)
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
    let Some(llm) = build_llm(&state.config) else {
        return Err("未配置 LLM（请设置 llm.providers.<provider>.api_key）".into());
    };
    let detail = state.sessions.get_session(session_id).map_err(|e| e.to_string())?;
    let gen = talksage_notes::TrioGenerator::new(llm);
    let trio = gen
        .generate(&detail.segments, meeting_name.as_deref(), meeting_description.as_deref())
        .map_err(|e| format!("智能纪要生成失败: {e}"))?;
    let json = serde_json::to_value(&trio).map_err(|e| e.to_string())?;
    state
        .sessions
        .set_trio(session_id, &json.to_string())
        .map_err(|e| format!("保存智能纪要失败: {e}"))?;
    Ok(json)
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
    let _log_guard = talksage_logging::init(Some(&data_dir));
    log::info!("TalkSage 桌面应用启动，数据目录: {}", data_dir.display());
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
            session_stats: Arc::new(Mutex::new(Vec::new())),
            session_texts: Arc::new(Mutex::new(Vec::new())),
            engine_pool: EnginePool::new(),
        })
        .invoke_handler(tauri::generate_handler![
            get_version,
            get_config,
            save_config,
            ping,
            start_listen,
            stop_listen,
            set_noise_level,
            get_voiceprint_status,
            enroll_voice,
            remove_voiceprint,
            minimize_to_tray,
            list_sessions,
            search_sessions,
            get_session,
            delete_session,
            list_notes_templates,
            generate_notes,
            generate_trio_notes,
            read_logs
        ])
        .setup(move |app| {
            if let Err(e) = std::fs::create_dir_all(&data_dir) {
                eprintln!("创建数据目录失败 {}: {e}", data_dir.display());
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
