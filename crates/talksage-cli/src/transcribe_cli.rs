//! 文件转写：`transcribe`（默认不落库）与 `import`（`--save` 别名）。

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use talksage_asr::{EngineKind, EnginePool};
use talksage_core::DomainEvent;
use talksage_pipeline::{StartListen, TalkSageService};

pub fn run(path: &str, engine: &str, save: bool, speaker: &str, json: bool) -> ExitCode {
    let kind = match EngineKind::from_name(engine) {
        Some(k) => k,
        None => {
            return fail(
                json,
                format!(
                    "未知引擎: {engine}（可选 qwen3-asr | whisper-large-v3-turbo-metal | whisper-medium-metal | aliyun-cloud）"
                ),
            );
        }
    };
    let model_dir = match TalkSageService::resolve_models_dir() {
        Some(d) => d,
        None => return fail(json, "未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）".into()),
    };
    let engine_dir = model_dir.join(kind.model_dir_name());
    let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
    if kind != EngineKind::AliyunCloud && (!vad_model.is_file() || !engine_dir.is_dir()) {
        return fail(
            json,
            "模型不完整（VAD 或 ASR 模型缺失），请先 `talksage models download` 或 scripts/download_models.py".into(),
        );
    }
    let audio_path = std::path::PathBuf::from(path);
    if !audio_path.is_file() {
        return fail(json, format!("文件不存在: {path}"));
    }

    if !json {
        println!("转写: {path}（{}）{}", kind.display_name(), if save { "，完成后落库" } else { "" });
    }

    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => Arc::new(m),
        Err(e) => return fail(json, format!("配置加载失败: {e}")),
    };
    let sessions = if save {
        match talksage_session::SessionStore::open(&mgr.data_dir().join("sessions.db").to_string_lossy()) {
            Ok(s) => Some(Arc::new(s)),
            Err(e) => return fail(json, format!("打开会话库失败: {e}")),
        }
    } else {
        None
    };
    let service = TalkSageService::new(mgr, sessions, EnginePool::new());
    let segments = Arc::new(Mutex::new(Vec::new()));
    let done = Arc::new(AtomicBool::new(false));
    let segs_for_sink = segments.clone();
    let done_for_sink = done.clone();
    let sink: talksage_pipeline::EventSink = Arc::new(move |ev| match &ev {
        DomainEvent::Segment {
            text,
            is_partial: false,
            speaker_id,
            speaker_label,
            speaker_attribution,
            ts_ms,
            duration_ms,
            rms,
            ..
        } => {
            segs_for_sink.lock().unwrap().push(talksage_core::TranscriptSegment {
                speaker_id: *speaker_id,
                speaker_label: speaker_label.clone(),
                speaker_attribution: speaker_attribution.clone(),
                text: text.clone(),
                is_partial: false,
                ts_ms: *ts_ms,
                duration_ms: *duration_ms,
                rms: *rms,
            });
        }
        DomainEvent::Status {
            stage: talksage_core::StatusStage::Idle,
            ..
        } => {
            done_for_sink.store(true, Ordering::SeqCst);
        }
        _ => {}
    });
    let mut req = StartListen::import_file(audio_path, kind, speaker.to_string());
    req.persist = save;
    let running = match service.start(req, sink) {
        Ok(r) => r,
        Err(e) => return fail(json, format!("启动失败: {e}")),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while !done.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let sid = match service.finish(running) {
        Ok(s) => s,
        Err(e) => return fail(json, format!("结束转写失败: {e}")),
    };

    let segs = segments.lock().unwrap();
    if segs.is_empty() {
        return fail(json, "未识别到语音内容".into());
    }
    succeed(
        json,
        json!({
            "ok": true,
            "engine": kind.display_name(),
            "session_id": sid,
            "segments": segs.iter().map(|s| json!({
                "speaker_label": s.speaker_label,
                "text": s.text,
                "ts_ms": s.ts_ms,
                "duration_ms": s.duration_ms,
            })).collect::<Vec<_>>(),
        }),
        || {
            if let Some(sid) = sid {
                println!("\n已保存会话 #{sid}（{} 段）", segs.len());
            }
            println!("转写结果（{} 段）:", segs.len());
            for s in segs.iter() {
                println!("  [{}] {}", s.speaker_label, s.text);
            }
        },
    )
}

fn fail(json: bool, msg: String) -> ExitCode {
    if json {
        eprintln!("{}", json!({"ok": false, "error": msg}));
    } else {
        eprintln!("{msg}");
    }
    ExitCode::FAILURE
}

fn succeed(json: bool, value: serde_json::Value, text: impl FnOnce()) -> ExitCode {
    if json {
        println!("{value}");
    } else {
        text();
    }
    ExitCode::SUCCESS
}
