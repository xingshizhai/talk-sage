//! 说话人识别集成测试（真实 wespeaker 模型）。
//!
//! 用中文语音 0.wav 注册为"我" → 同一音频识别为"我"；英文语音（不同说话人）→ 新建"客户1"并复用。
//! 依赖 models/wespeaker，缺失时跳过（不失败）。

use std::path::PathBuf;
use std::sync::Mutex;

use talksage_pipeline::speaker::{SpeakerDecision, SpeakerIdentifier};

mod common;
use common::skip;

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
    // 编号按角色域独立，回环域从 1 开始。
    let client_side_owner_audio = spk.query_for_role(&zh, "客户", false);
    assert_eq!(client_side_owner_audio.label(), "客户1", "回环域应从 1 开始编号");
    assert_ne!(
        client_side_owner_audio.decision(),
        SpeakerDecision::OwnerMatch,
        "回环通道不得被主人声纹改写为“我”"
    );

    // 新声音在**查询时**就拿到会话内编号：编号必须无锁原子预留，不能等确认，
    // 否则两条 ASR 线程会撞号。麦克风域与回环域各自编号 —— 所以这里是
    // “讲话者1”（麦克风域的第一次查询）而不是被回环域序列带偏的“讲话者2”。
    let first = spk.query(&en, "客户");
    assert_eq!(first.label(), "讲话者1", "麦克风域新声音应为本域 1 号");
    assert_eq!(first.decision(), SpeakerDecision::CandidateStarted);
    spk.commit(&first);

    // 第二个相似片段确认候选并注册；标签必须保持不变，
    // 不再出现“讲话者 → 客户1”这类身份跳变。
    let second = spk.query(&en, "客户");
    assert_eq!(second.label(), first.label(), "确认不得改变显示身份");
    assert_eq!(second.decision(), SpeakerDecision::CandidateConfirmed);
    spk.commit(&second);

    let third = spk.query(&en, "客户");
    assert_eq!(third.label(), first.label());
    assert!(matches!(third.decision(), SpeakerDecision::ExistingMatch | SpeakerDecision::GrayZoneReuse));
    spk.commit(&third);
    assert_eq!(spk.num_speakers(), 2);

    // 中英文都识别人数稳定
    assert_eq!(spk.identify(&zh, "我"), "我");
}

#[test]
fn alternating_speakers_keep_independent_candidates() {
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

    let a1 = spk.query_for_role(&zh, "客户", false);
    assert_eq!(a1.decision(), SpeakerDecision::CandidateStarted);
    spk.commit(&a1);
    let b1 = spk.query_for_role(&en, "客户", false);
    assert_eq!(b1.decision(), SpeakerDecision::CandidateStarted);
    spk.commit(&b1);

    let a2 = spk.query_for_role(&zh, "客户", false);
    assert_eq!(a2.label(), "客户1");
    assert_eq!(a2.decision(), SpeakerDecision::CandidateConfirmed);
    spk.commit(&a2);
    let b2 = spk.query_for_role(&en, "客户", false);
    assert_eq!(b2.label(), "客户2");
    assert_eq!(b2.decision(), SpeakerDecision::CandidateConfirmed);
    spk.commit(&b2);
    assert_eq!(spk.num_speakers(), 2);
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
    assert_eq!(dropped.label(), "讲话者1", "麦克风流的新声音用“讲话者”前缀");
    assert!(dropped.is_new());
    // 这一段随后被 filter 吞掉 → 不 commit，不得留下任何注册副作用
    drop(dropped);
    assert_eq!(spk.num_speakers(), before, "被吞掉的段注册了幻影说话人");

    // 下一段没被吞掉。它仍应是 CandidateStarted —— 这证明上一段**没有留下候选**，
    // 也就是本测试要守的回归（查询不得自带注册副作用）。
    let kept = spk.query(&en, "客户");
    assert_eq!(kept.decision(), SpeakerDecision::CandidateStarted, "被吞掉的段不得留下候选");
    assert_ne!(
        kept.label(),
        "讲话者1",
        "被吞掉的段仍会消耗本域编号（编号在查询时无锁预留、不可回收）——刻意的取舍"
    );
    assert!(!spk.commit(&kept), "首个放行片段只建立候选");
    assert_eq!(spk.num_speakers(), before, "候选不能计为正式说话人");

    // 第二个相似片段确认后才正式注册，且复用同一个标签。
    let confirmed = spk.query(&en, "客户");
    assert_eq!(confirmed.label(), kept.label(), "确认不得改变显示身份");
    assert_eq!(confirmed.decision(), SpeakerDecision::CandidateConfirmed);
    assert!(spk.commit(&confirmed), "第二个相似片段应确认候选");
    assert_eq!(spk.num_speakers(), before + 1);
    // 注册之后同一说话人应被复用，而不是再开一个编号
    let again = spk.query(&en, "客户");
    assert_eq!(again.label(), kept.label());
    assert!(!again.is_new(), "已注册的说话人不应再被当成新说话人");
}

/// 说话人编号按角色域独立：麦克风「讲话者N」与回环「客户N」各自从 1 开始。
///
/// 共用一条编号序列时，双流交错查询会产生「讲话者1 / 客户2 / 讲话者3」这类
/// 跳号：用户看到"客户2"却没有"客户1"，而文档承诺的是两个域各自编号。
#[test]
fn speaker_numbering_is_independent_per_role_domain() {
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

    // 交错查询两个域：各自从 1 开始，互不影响对方的序列。
    let mic_a = spk.query(&zh, "讲话者");
    let loop_a = spk.query_for_role(&en, "客户", false);
    let mic_b = spk.query(&en, "讲话者");
    let loop_b = spk.query_for_role(&zh, "客户", false);

    assert_eq!(mic_a.label(), "讲话者1");
    assert_eq!(mic_b.label(), "讲话者2");
    assert_eq!(loop_a.label(), "客户1");
    assert_eq!(loop_b.label(), "客户2");

    // 跨域隔离：在麦克风域注册一个说话人中心后，回环域用同一段音频查询
    // 不得命中它（两个域共享提取器与编号逻辑，但不共享聚类中心）。
    spk.commit(&mic_a);
    let mic_a_confirmed = spk.query(&zh, "讲话者");
    assert_eq!(mic_a_confirmed.label(), "讲话者1", "确认不得改变显示身份");
    assert_eq!(mic_a_confirmed.decision(), SpeakerDecision::CandidateConfirmed);
    spk.commit(&mic_a_confirmed);

    let cross = spk.query_for_role(&zh, "客户", false);
    assert_eq!(
        cross.decision(),
        SpeakerDecision::CandidateStarted,
        "回环域不得命中麦克风域已注册的说话人"
    );
    assert!(cross.label().starts_with("客户"), "跨域查询仍应落在回环域的编号空间");
}
