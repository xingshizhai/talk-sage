//! 说话人识别集成测试（真实 wespeaker 模型）。
//!
//! 用中文语音 0.wav 注册为"我" → 同一音频识别为"我"；英文语音（不同说话人）→ 新建"客户1"并复用。
//! 依赖 models/wespeaker，缺失时跳过（不失败）。

use std::path::PathBuf;

use talksage_pipeline::speaker::SpeakerIdentifier;

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


fn wespeaker_model() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d).join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx");
        if p.is_file() {
            return Some(p);
        }
    }
    let candidates = [
        PathBuf::from("models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"),
        PathBuf::from("../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"),
        PathBuf::from("../../models/wespeaker/wespeaker_zh_cnceleb_resnet34.onnx"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn wav16k(name: &str) -> Option<Vec<f32>> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = PathBuf::from(d).join(name);
        if let Ok((sr, samples)) = talksage_audio::wav::read_wav(&p) {
            if sr == 16000 {
                return Some(samples);
            }
        }
    }
    let candidates = [
        PathBuf::from("models").join(name),
        PathBuf::from("../models").join(name),
        PathBuf::from("../../models").join(name),
    ];
    for p in candidates {
        if let Ok((sr, samples)) = talksage_audio::wav::read_wav(&p) {
            if sr == 16000 {
                return Some(samples);
            }
        }
    }
    None
}

#[test]
fn identifies_owner_and_new_speaker() {
    let Some(model) = wespeaker_model() else {
        return skip("未找到 wespeaker 声纹模型（运行 scripts/download_models.py wespeaker）");
    };
    let Some(zh) = wav16k("sherpa-onnx-streaming-paraformer-zh/0.wav") else {
        return skip("缺少中文测试音频");
    };
    let Some(en) = wav16k("sherpa-onnx-streaming-zipformer-en-2023-06-26/0.wav") else {
        return skip("缺少英文测试音频");
    };

    let spk = SpeakerIdentifier::new(&model, None, 0.5).expect("wespeaker 模型加载失败");
    // 注册主人 = 中文说话人
    let emb = spk.compute_embedding(&zh).expect("中文音频应能提取声纹");
    assert!(spk.add_owner(&emb));
    assert!(spk.has_owner());

    // 同一说话人 → "我"
    assert_eq!(spk.identify(&zh, "我"), "我");

    // 不同说话人（英文）→ 新建 "客户1"，且再次出现复用
    assert_eq!(spk.identify(&en, "客户"), "客户1");
    assert_eq!(spk.identify(&en, "客户"), "客户1");
    assert_eq!(spk.num_speakers(), 2);

    // 中英文都识别人数稳定
    assert_eq!(spk.identify(&zh, "我"), "我");
}
