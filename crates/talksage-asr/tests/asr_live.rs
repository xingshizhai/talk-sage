//! ASR 引擎集成测试：真实模型加载 + 流式识别（模型存在时）。

use std::path::{Path, PathBuf};

use sherpa_onnx::Wave;
use talksage_asr::{EngineKind, SherpaStreamingEngine, StreamingASREngine};

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

#[test]
fn paraformer_zh_streaming_recognizes_chinese_audio() {
    let Some(root) = model_root() else {
        eprintln!("跳过：未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
        return;
    };
    let model_dir = root.join("sherpa-onnx-streaming-paraformer-zh");
    let wav = model_dir.join("0.wav");
    if !model_dir.is_dir() || !wav.is_file() {
        eprintln!("跳过：模型或测试音频不完整");
        return;
    }

    let mut engine = SherpaStreamingEngine::new(EngineKind::ParaformerZh, &model_dir, 2)
        .expect("创建 paraformer-zh 引擎失败");

    let wave = Wave::read(&wav.to_string_lossy()).expect("读取 wav 失败");
    assert_eq!(wave.sample_rate(), 16000);

    // 流式喂入前 ~3s（块 200ms），断言增量出字
    let chunk_size = 16000 * 200 / 1000;
    let mut collected = String::new();
    for chunk in wave.samples().chunks(chunk_size).take(15) {
        if let Some(text) = engine.accept(chunk) {
            collected = text;
        }
    }
    assert!(
        !collected.trim().is_empty(),
        "paraformer-zh 流式识别无输出（前 3s）"
    );
    eprintln!("asr 集成测试识别: {collected}");
}

#[test]
fn engine_kind_from_name_covers_configured_values() {
    // 与配置默认值一致（talksage-config defaults）
    assert_eq!(EngineKind::from_name("paraformer-zh"), Some(EngineKind::ParaformerZh));
    assert_eq!(EngineKind::from_name("zipformer-en"), Some(EngineKind::ZipformerEn));
}
