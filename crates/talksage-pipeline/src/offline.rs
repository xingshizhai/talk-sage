//! 离线文件转写助手：单文件流式转写，引擎池热启动复用。
//!
//! 供两处共用：
//! - CLI `talksage bench`（固定语料评测，逐文件转写）
//! - headless OpenAI 兼容转写 API `POST /v1/audio/transcriptions`
//!
//! 复用 LivePipeline 的文件输入路径（AudioInput::File），与实时监听完全同一套
//! VAD 分段 + 流式 ASR + 领域事件逻辑，保证评测/API 结果与真实使用一致。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;

use talksage_asr::{EngineKind, EnginePool};
use talksage_core::{DomainEvent, StatusStage};

use crate::runtime::SessionRuntime;
use crate::{AudioInput, EventSink, LivePipelineConfig, RuntimeParams, StreamConfig};

/// 单个 final 段信息（verbose_json 输出用）。
#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub text: String,
    pub ts_ms: u64,
    pub duration_ms: u64,
}

/// 单文件转写结果。
#[derive(Debug, Clone)]
pub struct FileTranscription {
    /// final 段文本拼接（段间空格分隔）。
    pub text: String,
    /// 处理耗时（管道启动 → Idle 或超时），ms。
    pub elapsed_ms: f64,
    /// 首词延迟（管道启动 → 首个 final 段），ms；无 final 段时 None。
    pub first_latency_ms: Option<f64>,
    /// final 段明细。
    pub segments: Vec<SegmentInfo>,
}

/// 对单个 wav 跑流式转写（16kHz mono PCM；非 16k 请先用 `talksage_audio::resample_linear` 归一化）。
///
/// - 引擎从 `pool` 借用（None = 每次新建），结束后归还 → 多文件/多请求热启动复用。
/// - 收到 Idle 或超时（300s）结束；超时/异常时返回已收集文本。
pub fn transcribe_file(
    pool: Option<&Arc<EnginePool>>,
    engine_kind: EngineKind,
    model_dir: &Path,
    vad_model: &Path,
    wav: &Path,
) -> Result<FileTranscription> {
    let cfg = LivePipelineConfig {
        vad_model: vad_model.to_path_buf(),
        chunk_ms: 100,
        vad: talksage_config::VadConfig::default(),
        denoise: talksage_config::DenoiseConfig::default(),
        endpoint: talksage_config::EndpointConfig::default(),
        asr_threads: 2,
        input_gain_db: 0.0,
        user: StreamConfig {
            engine_kind,
            model_dir: model_dir.to_path_buf(),
            input: AudioInput::File(wav.to_path_buf()),
            speaker_id: 0,
            speaker_label: "我".into(),
            engine_options: Default::default(),
            terminology: Default::default(),
        },
        client: None,
        plugins: Vec::new(),
        plugin_ctx: talksage_plugins::PluginContext::new(),
        recording_dir: None,
        runtime: Arc::new(RuntimeParams::default()),
        speaker: None,
        engine_pool: pool.cloned(),
        hooks: talksage_plugins::build_registry(
            &talksage_plugins::builtin_plugins(),
            &std::collections::HashMap::from([(
                "short_segment".to_string(),
                serde_json::json!({ "min_ms": 0 }),
            )]),
        ),
    };

    let start = Instant::now();
    let done = Arc::new(AtomicBool::new(false));
    let first_latency = Arc::new(Mutex::new(None::<f64>));
    let texts = Arc::new(Mutex::new(String::new()));
    let segs = Arc::new(Mutex::new(Vec::<SegmentInfo>::new()));
    {
        let done_sink = done.clone();
        let first = first_latency.clone();
        let texts = texts.clone();
        let segs = segs.clone();
        let start = start;
        let sink: EventSink = Arc::new(move |ev| {
            match &ev {
                DomainEvent::Segment {
                    text,
                    is_partial: false,
                    ts_ms,
                    duration_ms,
                    ..
                } => {
                    let mut f = first.lock().unwrap();
                    if f.is_none() {
                        *f = Some(start.elapsed().as_millis() as f64);
                    }
                    drop(f);
                    let mut t = texts.lock().unwrap();
                    if !t.is_empty() {
                        t.push(' ');
                    }
                    t.push_str(text.trim());
                    segs.lock().unwrap().push(SegmentInfo {
                        text: text.trim().to_string(),
                        ts_ms: *ts_ms,
                        duration_ms: *duration_ms,
                    });
                }
                DomainEvent::Status { stage: StatusStage::Idle, .. } => {
                    done_sink.store(true, Ordering::SeqCst);
                }
                _ => {}
            }
        });
        let mut runtime = SessionRuntime::new(cfg);
        runtime.start(sink)?;
        let deadline = Instant::now() + std::time::Duration::from_secs(300);
        while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        runtime.stop();
    }
    let elapsed = start.elapsed().as_millis() as f64;
    let text = texts.lock().unwrap().clone();
    let latency = first_latency.lock().unwrap().clone();
    let segments = segs.lock().unwrap().clone();
    Ok(FileTranscription {
        text,
        elapsed_ms: elapsed,
        first_latency_ms: latency,
        segments,
    })
}
