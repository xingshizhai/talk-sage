//! pipeline 集成测试：wav 文件输入 → VAD → 流式 ASR → 事件序列断言。
//!
//! 依赖仓库内模型（models/），缺失时打印提示并跳过（不失败）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use talksage_core::DomainEvent;
use talksage_pipeline::{AudioInput, LivePipeline, LivePipelineConfig, StreamConfig};
use talksage_asr::EngineKind;

/// 解析模型根目录（TALKSAGE_MODELS_DIR 优先，其次相对 CARGO_MANIFEST_DIR 探测）。
fn model_root() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
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

fn zh_model_dir(root: &Path) -> PathBuf {
    root.join("sherpa-onnx-streaming-paraformer-zh")
}

fn en_model_dir(root: &Path) -> PathBuf {
    root.join("sherpa-onnx-streaming-zipformer-en-2023-06-26")
}

fn vad_model(root: &Path) -> PathBuf {
    root.join("silero-vad").join("silero_vad.onnx")
}

/// 运行管道收集事件，直到 Idle 或超时。
fn run_and_collect(cfg: LivePipelineConfig) -> Vec<DomainEvent> {
    let events: Arc<Mutex<Vec<DomainEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let sink: Arc<dyn Fn(DomainEvent) + Send + Sync> = Arc::new(move |ev: DomainEvent| {
        sink_events.lock().unwrap().push(ev);
    });

    let mut pipeline = LivePipeline::new(cfg);
    pipeline.start(sink).expect("pipeline 启动失败");

    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let done = {
            let evs = events.lock().unwrap();
            evs.iter().any(|e| matches!(e, DomainEvent::Status { stage: talksage_core::StatusStage::Idle, .. }))
        };
        if done || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    pipeline.stop();
    let result = events.lock().unwrap().clone();
    result
}

fn zh_file_pipeline(root: &Path, wav: &Path) -> LivePipelineConfig {
    LivePipelineConfig {
        vad_model: vad_model(root),
        chunk_ms: 100,
        vad: talksage_config::VadConfig::default(),
        denoise: talksage_config::DenoiseConfig::default(),
        asr_threads: 2,
        user: StreamConfig {
            engine_kind: EngineKind::ParaformerZh,
            model_dir: zh_model_dir(root),
            input: AudioInput::File(wav.to_path_buf()),
            speaker_id: 0,
            speaker_label: "我".into(),
        },
        client: None,
        plugins: Vec::new(),
        plugin_ctx: talksage_plugins::PluginContext::new(),
    }
}

#[test]
fn file_input_produces_status_and_segments() {
    let Some(root) = model_root() else {
        eprintln!("跳过：未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
        return;
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        eprintln!("跳过：模型/VAD/测试音频不完整");
        return;
    }

    let evs = run_and_collect(zh_file_pipeline(&root, &wav));

    // 1. 状态事件链包含 ASR 就绪
    assert!(
        evs.iter().any(|e| matches!(e, DomainEvent::Status { stage: talksage_core::StatusStage::AsrReady, .. })),
        "缺少 AsrReady 状态事件，实际事件: {evs:?}"
    );

    // 2. 至少一个 final 转写段且文本非空
    let finals: Vec<&String> = evs
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Segment { text, is_partial: false, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert!(!finals.is_empty(), "未产生 final 转写段，实际事件: {evs:?}");
    let joined = finals.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" | ");
    assert!(!joined.trim().is_empty(), "final 转写文本为空");
    eprintln!("pipeline 集成测试识别结果: {joined}");
}

#[test]
fn file_input_partial_events_precede_final() {
    let Some(root) = model_root() else {
        eprintln!("跳过：未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
        return;
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        eprintln!("跳过：模型/VAD/测试音频不完整");
        return;
    }

    let evs = run_and_collect(zh_file_pipeline(&root, &wav));
    let mut saw_partial = false;
    let mut saw_final = false;
    for e in &evs {
        match e {
            DomainEvent::Segment { is_partial: true, .. } => saw_partial = true,
            DomainEvent::Segment { is_partial: false, .. } => saw_final = true,
            _ => {}
        }
    }
    assert!(saw_partial, "缺少 partial 增量事件: {evs:?}");
    assert!(saw_final, "缺少 final 事件: {evs:?}");
}

/// 双流：user（中文文件）+ client（英文文件）→ 两个 speaker 都产生事件。
#[test]
fn dual_stream_user_and_client_both_produce_segments() {    let Some(root) = model_root() else {
        eprintln!("跳过：未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
        return;
    };
    let zh_wav = zh_model_dir(&root).join("0.wav");
    let en_wav = en_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !zh_wav.is_file() || !en_wav.is_file() {
        eprintln!("跳过：模型/VAD/测试音频不完整");
        return;
    }
    if !en_model_dir(&root).is_dir() {
        eprintln!("跳过：缺少英文模型");
        return;
    }

    let mut cfg = zh_file_pipeline(&root, &zh_wav);
    cfg.client = Some(StreamConfig {
        engine_kind: EngineKind::ZipformerEn,
        model_dir: en_model_dir(&root),
        input: AudioInput::File(en_wav),
        speaker_id: 1,
        speaker_label: "客户".into(),
    });

    let evs = run_and_collect(cfg);

    let mut user_finals = 0;
    let mut client_finals = 0;
    for e in &evs {
        if let DomainEvent::Segment {
            speaker_id,
            is_partial: false,
            ..
        } = e
        {
            if *speaker_id == 0 {
                user_finals += 1;
            } else if *speaker_id == 1 {
                client_finals += 1;
            }
        }
    }
    assert!(user_finals > 0, "user 流未产生 final 转写: {evs:?}");
    assert!(client_finals > 0, "client 流未产生 final 转写: {evs:?}");
    eprintln!("双流测试：user_finals={user_finals}, client_finals={client_finals}");
}

/// 插件集成：英文客户文件 + term_explainer（mock LLM）+ translator → Term/Translation 事件。
#[test]
fn plugins_emit_term_and_translation_events() {
    let Some(root) = model_root() else {
        eprintln!("跳过：未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
        return;
    };
    let en_wav = en_model_dir(&root).join("0.wav");
    let en_model = en_model_dir(&root);
    if !en_model.is_dir() || !en_wav.is_file() || !vad_model(&root).is_file() {
        eprintln!("跳过：模型/VAD/测试音频不完整");
        return;
    }

    // mock LLM：术语解释返回固定文本；翻译返回固定文本
    let mock = talksage_llm::MockProvider {
        response: "NPI = New Product Introduction（新产品导入）\n翻译：在早夜降临后……".into(),
    };
    let ctx = talksage_plugins::PluginContext {
        kb: None,
        llm: Some(Arc::new(mock)),
    };
    let plugins: Vec<Arc<dyn talksage_plugins::AnalyzerPlugin>> = vec![
        Arc::new(talksage_plugins::term_explainer::TermExplainerPlugin::new(0.0)),
        Arc::new(talksage_plugins::translator::TranslatorPlugin::new()),
    ];

    let cfg = LivePipelineConfig {
        vad_model: vad_model(&root),
        chunk_ms: 100,
        vad: talksage_config::VadConfig::default(),
        denoise: talksage_config::DenoiseConfig::default(),
        asr_threads: 2,
        user: StreamConfig {
            engine_kind: EngineKind::ZipformerEn,
            model_dir: en_model,
            input: AudioInput::File(en_wav),
            speaker_id: 1,
            speaker_label: "客户".into(),
        },
        client: None,
        plugins,
        plugin_ctx: ctx,
    };

    let evs = run_and_collect(cfg);
    let mut saw_translation = false;
    for e in &evs {
        if matches!(e, DomainEvent::Translation { .. }) {
            saw_translation = true;
        }
    }
    // term 触发的缩写判定由 plugins 单测覆盖（真实英文识别为全大写，不触发缩写）
    assert!(saw_translation, "未产生 Translation 事件: {evs:?}");
    eprintln!("插件集成测试：Translation 事件产生（真实识别链路）");
}
