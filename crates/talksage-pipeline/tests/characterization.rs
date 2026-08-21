//! 特征化测试：把固定语料跑出的事件序列归一化后与 golden 文件比对。
//!
//! 目的是在插件化重构期间锁住行为 —— 它不判断行为「对不对」，只判断
//! 「和重构前一不一样」。预期内的行为变更需显式更新 golden 文件并在
//! 提交信息里说明。
//!
//! 重新生成：TALKSAGE_UPDATE_GOLDEN=1 cargo test -p talksage-pipeline --test characterization

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use talksage_asr::EngineKind;
use talksage_core::DomainEvent;
use talksage_pipeline::{AudioInput, LivePipelineConfig, SessionRuntime, StreamConfig};

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

fn model_root() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    // 候选列表与 pipeline_live.rs 保持一致：否则换个工作目录跑时，
    // 本测试会静默跳过而其余测试照跑 —— 安全网失效且无人察觉。
    for cand in [
        here.join("../../models"),
        here.join("../../../models"),
        PathBuf::from("models"),
        PathBuf::from("../models"),
    ] {
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

/// 事件归一化：只保留对行为有意义的字段。
///
/// 刻意丢弃 ts_ms / rms / revision —— 它们随采样与实现细节浮动，
/// 纳入 golden 会让测试因无关变更而红。
fn normalize(evs: &[DomainEvent]) -> String {
    let mut out = String::new();
    for ev in evs {
        match ev {
            // 只记「有一个 final 段、属于谁」，不记文本与时长。
            //
            // 转写文本与段时长取决于 models/ 里恰好存在哪些模型文件 —— 引擎在
            // 目录含 fp32 时会自动优先用 fp32（见 talksage-asr 的 EngineKind），
            // 而 models/ 是 gitignore 的。把它们写进 golden 会让基线随机器变化，
            // 且失败信息会诱导人「重新生成 golden」，把机器差异洗成新基线。
            //
            // 本测试的职责是「重构前后行为一不一样」，不是「识别得准不准」——
            // 后者归 evaluation/ 语料与 scripts/evaluate.py（CER/WER）。
            DomainEvent::Segment { speaker_label, is_partial: false, .. } => {
                out.push_str(&format!("final\t{speaker_label}\n"));
            }
            // partial 条数随线程调度与模型浮动，连续多条折叠成一个标记：
            // 只锁「这里有增量输出」，不锁具体条数。
            DomainEvent::Segment { is_partial: true, .. } => {
                if !out.ends_with("partial…\n") {
                    out.push_str("partial…\n");
                }
            }
            DomainEvent::Status { stage, .. } => out.push_str(&format!("status\t{stage:?}\n")),
            DomainEvent::Term { status, .. } => out.push_str(&format!("term\t{status:?}\n")),
            DomainEvent::Translation { status, direction, .. } => {
                out.push_str(&format!("translation\t{status:?}\t{direction:?}\n"))
            }
            DomainEvent::Metrics { .. } => out.push_str("metrics\n"),
            DomainEvent::Nudge { .. } => out.push_str("nudge\n"),
            DomainEvent::SessionStats { speaker_label, final_segments, .. } => {
                out.push_str(&format!("stats\t{speaker_label}\t{final_segments}\n"))
            }
            DomainEvent::Level { .. } => {} // 高频且随机，完全忽略
            other => out.push_str(&format!("{}\n", other_kind(other))),
        }
    }
    out
}

fn other_kind(ev: &DomainEvent) -> &'static str {
    match ev {
        DomainEvent::Brief { .. } => "brief",
        DomainEvent::State { .. } => "state",
        DomainEvent::KeyPoint { .. } => "keypoint",
        DomainEvent::Snapshot { .. } => "snapshot",
        _ => "other",
    }
}

fn run_and_collect(cfg: LivePipelineConfig) -> Vec<DomainEvent> {
    let events: Arc<Mutex<Vec<DomainEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let sink: Arc<dyn Fn(DomainEvent) + Send + Sync> =
        Arc::new(move |ev: DomainEvent| sink_events.lock().unwrap().push(ev));

    let mut runtime = SessionRuntime::new(cfg);
    runtime.start(sink).expect("pipeline 启动失败");
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let done = {
            let evs = events.lock().unwrap();
            evs.iter().any(|e| {
                matches!(e, DomainEvent::Status { stage: talksage_core::StatusStage::Idle, .. })
            })
        };
        if done || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // 超时必须显式报错：否则会拿一份不完整的事件列表去比 golden，
    // 失败信息变成「事件序列与 golden 不一致」，把「管道卡住」误报成
    // 「重构改了语义」。
    let timed_out = Instant::now() > deadline;
    assert!(!timed_out, "管道在 150s 内未到达 Idle，事件序列不完整，拒绝与 golden 比对");
    assert!(runtime.stop_with_timeout(Duration::from_secs(5)), "管道应在时限内结束");
    let result = events.lock().unwrap().clone();
    result
}

fn zh_pipeline(root: &Path, wav: &Path, min_commit_ms: u64) -> LivePipelineConfig {
    LivePipelineConfig {
        vad_model: root.join("silero-vad").join("silero_vad.onnx"),
        chunk_ms: 100,
        vad: talksage_config::VadConfig::default(),
        denoise: talksage_config::DenoiseConfig::default(),
        endpoint: talksage_config::EndpointConfig::default(),
        asr_threads: 2,
        input_gain_db: 0.0,
        user: StreamConfig {
            engine_kind: EngineKind::ParaformerZh,
            model_dir: root.join("sherpa-onnx-streaming-paraformer-zh"),
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
        runtime: Arc::new(talksage_pipeline::RuntimeParams::default()),
        speaker: None,
        engine_pool: None,
        hooks: talksage_plugins::build_registry(
            &talksage_plugins::builtin_plugins(),
            &std::collections::HashMap::from([(
                "short_segment".to_string(),
                serde_json::json!({ "min_ms": min_commit_ms }),
            )]),
            &talksage_plugins::PluginContext::new(),
        ),
    }
}

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name)
}

/// 与 golden 比对；设 TALKSAGE_UPDATE_GOLDEN=1 时改为写入。
///
/// 只认 1/true，与 TALKSAGE_REQUIRE_MODELS 的判定一致。用 `is_ok()` 会让
/// `TALKSAGE_UPDATE_GOLDEN=0`（本意是关闭）反而改写 golden —— 那正是这个
/// 测试要防的「回归被静默吞掉」。
fn assert_golden(name: &str, actual: &str) {
    let path = golden_path(name);
    if matches!(
        std::env::var("TALKSAGE_UPDATE_GOLDEN").ok().as_deref(),
        Some("1") | Some("true")
    ) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        eprintln!("已更新 golden: {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "缺少 golden 文件 {}；首次生成请运行：\n  \
             TALKSAGE_UPDATE_GOLDEN=1 cargo test -p talksage-pipeline --test characterization",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "事件序列与 golden 不一致。若为预期内的行为变更，用 TALKSAGE_UPDATE_GOLDEN=1 更新并在提交信息里说明原因。"
    );
}

#[test]
fn zh_single_stream_event_sequence_is_stable() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = root.join("sherpa-onnx-streaming-paraformer-zh").join("0.wav");
    if !wav.is_file() || !root.join("silero-vad").join("silero_vad.onnx").is_file() {
        return skip("模型/VAD/测试音频不完整");
    }
    let evs = run_and_collect(zh_pipeline(&root, &wav, 0));
    assert_golden("zh_single_stream.txt", &normalize(&evs));
}
