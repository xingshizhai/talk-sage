//! ASR 引擎集成测试：真实模型加载 + 流式识别（模型存在时）。

use std::path::{Path, PathBuf};

use sherpa_onnx::Wave;
use talksage_asr::{EngineKind, SegmentEngine, SherpaStreamingEngine, StreamingASREngine};

/// 缺资源时是否必须失败（而不是跳过）。`env` 为 `TALKSAGE_REQUIRE_MODELS` 的值。
fn must_fail_on_missing(env: Option<&str>) -> bool {
    matches!(env, Some("1") | Some("true"))
}

/// 资源缺失：默认打印并跳过；`TALKSAGE_REQUIRE_MODELS=1` 时直接失败，
/// 避免 CI 上「因跳过而全绿」掩盖回归。
fn skip(reason: &str) {
    let env = std::env::var("TALKSAGE_REQUIRE_MODELS").ok();
    assert!(
        !must_fail_on_missing(env.as_deref()),
        "集成测试资源缺失（TALKSAGE_REQUIRE_MODELS=1 要求必须真实运行）: {reason}"
    );
    eprintln!("跳过：{reason}");
}

/// 可选引擎（whisper / qwen3 等，不在 `deps` 必需集里）缺失：始终跳过，
/// 不受 `TALKSAGE_REQUIRE_MODELS` 影响 —— CI 不为可选引擎拉数百 MB 模型。
fn skip_optional(reason: &str) {
    eprintln!("跳过（可选引擎）：{reason}");
}

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
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let model_dir = root.join("sherpa-onnx-streaming-paraformer-zh");
    let wav = model_dir.join("0.wav");
    if !model_dir.is_dir() || !wav.is_file() {
        return skip("模型或测试音频不完整");
    }

    let mut engine = SherpaStreamingEngine::new(EngineKind::ParaformerZh, &model_dir, 2)
        .expect("创建 paraformer-zh 引擎失败");

    let wave = Wave::read(&wav.to_string_lossy()).expect("读取 wav 失败");
    assert_eq!(wave.sample_rate(), 16000);

    // 流式喂入前 ~3s（块 200ms），断言增量出字
    let chunk_size = 16000 * 200 / 1000;
    let mut collected = String::new();
    let mut chunks = wave.samples().chunks(chunk_size);
    for chunk in chunks.by_ref().take(15) {
        if let Some(text) = StreamingASREngine::accept(&mut engine, chunk) {
            collected = text;
        }
    }
    assert!(
        !collected.trim().is_empty(),
        "paraformer-zh 流式识别无输出（前 3s）"
    );
    eprintln!("asr 集成测试识别: {collected}");

    // 继续送完整段并结束。固定语料末尾是“星期三”；这个断言专门防止
    // 流式模型缺少右侧上下文时把最后一个“三”吞掉。
    for chunk in chunks {
        let _ = StreamingASREngine::accept(&mut engine, chunk);
    }
    let final_text = SegmentEngine::finish(&mut engine);
    assert!(
        final_text.trim().ends_with('三'),
        "流式结束必须保留句尾字符，实际: {final_text}"
    );
}

#[test]
fn engine_kind_from_name_covers_configured_values() {
    // 与配置默认值一致（talksage-config defaults）
    assert_eq!(EngineKind::from_name("paraformer-zh"), Some(EngineKind::ParaformerZh));
    assert_eq!(EngineKind::from_name("zipformer-en"), Some(EngineKind::ZipformerEn));
    assert_eq!(EngineKind::from_name("whisper-base"), Some(EngineKind::WhisperBase));
    assert_eq!(EngineKind::from_name("whisper-small"), Some(EngineKind::WhisperSmall));
    assert_eq!(EngineKind::from_name("qwen3-asr"), Some(EngineKind::Qwen3Asr));
    assert!(!EngineKind::WhisperBase.is_streaming());
    assert!(EngineKind::ParaformerZh.is_streaming());
}

/// 离线段级引擎（whisper-base）：整段喂入后 finish 返回文本。
#[test]
fn whisper_offline_segment_engine_recognizes_audio() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let model_dir = root.join("sherpa-onnx-whisper-base");
    let wav = root.join("sherpa-onnx-streaming-paraformer-zh").join("0.wav");
    if !model_dir.is_dir() || !wav.is_file() {
        return skip_optional("whisper-base 模型未下载（download_models.py whisper-base）");
    }
    let mut engine = talksage_asr::OfflineSegmentEngine::new(EngineKind::WhisperBase, &model_dir, 2)
        .expect("创建 whisper-base 引擎失败");
    let wave = Wave::read(&wav.to_string_lossy()).expect("读取 wav 失败");
    // 段级：全部喂入（不产生 partial），finish 后应返回文本
    assert_eq!(engine.accept(wave.samples()), None, "离线段级不应产生 partial");
    let text = engine.finish();
    assert!(!text.trim().is_empty(), "whisper-base 离线识别无输出");
    eprintln!("whisper-base 离线识别: {text}");
}
