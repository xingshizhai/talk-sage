//! 说话人识别集成测试（真实 wespeaker 模型）。
//!
//! 用中文语音 0.wav 注册为"我" → 同一音频识别为"我"；英文语音（不同说话人）→ 新建"客户1"并复用。
//! 依赖 models/wespeaker，缺失时跳过（不失败）。

use std::path::PathBuf;
use std::sync::Mutex;

use talksage_pipeline::speaker::{SpeakerDecision, SpeakerIdentifier};

mod common;
use common::{model_root, skip};

/// 同一进程里并发创建 `SpeakerEmbeddingExtractor` 会让 onnxruntime 抛出
/// `Ort::Exception` 并 abort（整个测试进程挂掉，SIGABRT）。用真实模型的测试
/// 因此必须串行 —— 与被测逻辑无关，纯粹是 ORT 的进程级限制。
static ORT_MODEL: Mutex<()> = Mutex::new(());

/// 取得模型串行锁；前一个测试 panic 时不因中毒而连带失败。
fn model_guard() -> std::sync::MutexGuard<'static, ()> {
    ORT_MODEL.lock().unwrap_or_else(|e| e.into_inner())
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

    let _serial = model_guard();
    let spk = SpeakerIdentifier::new(&model, None, 0.5).expect("wespeaker 模型加载失败");
    // 注册主人 = 中文说话人
    let emb = spk
        .enrollment_embedding(&zh)
        .expect("中文音频应通过多窗口注册质量检查");
    assert!(spk.add_owner(&emb));
    assert!(spk.has_owner());

    // 同一说话人 → "我"
    let owner = spk.query(&zh, "我");
    assert_eq!(owner.label(), "我");
    assert_eq!(owner.decision(), SpeakerDecision::OwnerMatch);
    assert!(owner.similarity().is_some());

    // 双流的回环通道不能被主人声纹改写为“我”；业务角色由通道决定。
    assert_eq!(spk.query_for_role(&zh, "客户", false).label(), "客户");

    // 不同说话人第一次只建立候选，第二个相似片段才确认“客户1”。
    let first = spk.query(&en, "客户");
    assert_eq!(first.label(), "客户");
    assert_eq!(first.decision(), SpeakerDecision::CandidateStarted);
    spk.commit(&first);
    let second = spk.query(&en, "客户");
    assert_eq!(second.label(), "客户1");
    assert_eq!(second.decision(), SpeakerDecision::CandidateConfirmed);
    spk.commit(&second);
    let third = spk.query(&en, "客户");
    assert_eq!(third.label(), "客户1");
    assert!(matches!(third.decision(), SpeakerDecision::ExistingMatch | SpeakerDecision::GrayZoneReuse));
    spk.commit(&third);
    assert_eq!(spk.num_speakers(), 2);

    // 中英文都识别人数稳定
    assert_eq!(spk.identify(&zh, "我"), "我");
}

/// 回归网：被 filter 吞掉的段绝不能注册出「幻影说话人」。
///
/// 复现的是 `StreamWorker::finish_speech` 的顺序：先查询标签（事件与跨流去重
/// 都要用），再过 filter 链。若查询自带注册副作用（重构前的 `identify`），
/// 被丢弃的段会永久占掉一个"客户N"，之后真实的段可能匹配上这个幻影。
#[test]
fn a_filtered_out_segment_registers_no_speaker() {
    let Some(model) = wespeaker_model() else {
        return skip("未找到 wespeaker 声纹模型（运行 scripts/download_models.py wespeaker）");
    };
    let Some(en) = wav16k("sherpa-onnx-streaming-zipformer-en-2023-06-26/0.wav") else {
        return skip("缺少英文测试音频");
    };
    let _serial = model_guard();
    let spk = SpeakerIdentifier::new(&model, None, 0.5).expect("wespeaker 模型加载失败");
    let before = spk.num_speakers();

    // 产生点：先查询标签（>0.5s 音频，足以算出声纹）
    let dropped = spk.query(&en, "客户");
    assert_eq!(dropped.label(), "客户", "首次出现只保留通用角色标签");
    assert!(dropped.is_new());
    // 这一段随后被 filter 吞掉 → 不 commit，不得留下任何注册副作用
    drop(dropped);
    assert_eq!(spk.num_speakers(), before, "被吞掉的段注册了幻影说话人");

    // 下一段没被吞掉：编号未被上一段消耗，commit 后只建立候选。
    let kept = spk.query(&en, "客户");
    assert_eq!(kept.label(), "客户", "被吞掉的段不应消耗客户编号");
    assert!(!spk.commit(&kept), "首个放行片段只建立候选");
    assert_eq!(spk.num_speakers(), before, "候选不能计为正式说话人");

    // 第二个相似片段确认后才正式注册。
    let confirmed = spk.query(&en, "客户");
    assert_eq!(confirmed.label(), "客户1");
    assert!(spk.commit(&confirmed), "第二个相似片段应确认候选");
    assert_eq!(spk.num_speakers(), before + 1);
    // 注册之后同一说话人应被复用，而不是再开一个编号
    let again = spk.query(&en, "客户");
    assert_eq!(again.label(), "客户1");
    assert!(!again.is_new(), "已注册的说话人不应再被当成新说话人");
}
