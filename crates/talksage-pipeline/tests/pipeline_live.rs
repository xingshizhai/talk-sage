//! pipeline 集成测试：wav 文件输入 → VAD → 流式 ASR → 事件序列断言。
//!
//! 依赖仓库内模型（models/）。默认缺失时打印提示并跳过；CI 设
//! `TALKSAGE_REQUIRE_MODELS=1` 时缺失即失败，避免「因跳过而全绿」掩盖回归。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use talksage_asr::EngineKind;
use talksage_core::DomainEvent;
use talksage_pipeline::{AudioInput, LivePipelineConfig, SessionRuntime, StreamConfig};

/// 内置插件钩子（短段阈值可调）。整份注册表只建一次，两条流共享同一批
/// filter 实例 —— 跨流去重靠的就是这份共享历史。
fn hooks_with_min_commit_ms(min_ms: u64) -> talksage_plugins::HookRegistry {
    talksage_plugins::build_registry(
        &talksage_plugins::builtin_plugins(),
        &std::collections::HashMap::from([(
            "short_segment".to_string(),
            serde_json::json!({ "min_ms": min_ms }),
        )]),
    )
}

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

#[test]
fn require_models_flag_controls_skip_vs_fail() {
    assert!(!must_fail_on_missing(None), "未设置时应跳过");
    assert!(!must_fail_on_missing(Some("0")), "0 应跳过");
    assert!(!must_fail_on_missing(Some("")), "空值应跳过");
    assert!(must_fail_on_missing(Some("1")), "1 应失败");
    assert!(must_fail_on_missing(Some("true")), "true 应失败");
}

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

    let mut runtime = SessionRuntime::new(cfg);
    runtime.start(sink).expect("pipeline 启动失败");

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
    let stopped = runtime.stop_with_timeout(Duration::from_secs(5));
    assert!(stopped, "正常文件管道应在时限内结束");
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
        recording_dir: None,
        runtime: std::sync::Arc::new(talksage_pipeline::RuntimeParams::default()),
        speaker: None,
        engine_pool: None,
        hooks: hooks_with_min_commit_ms(0),
    }
}

#[test]
fn file_input_produces_status_and_segments() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
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

    // 3. 会话指标事件（借鉴 Call.md conversation-metrics）随 final 段推送
    let metrics_events: Vec<&talksage_core::ConversationMetrics> = evs
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Metrics { metrics } => Some(metrics),
            _ => None,
        })
        .collect();
    assert!(!metrics_events.is_empty(), "未产生 Metrics 事件: {evs:?}");
    let last = metrics_events.last().unwrap();
    assert!(last.has_data(), "Metrics 应有段数据: {last:?}");
    assert!(last.segment_count_me + last.segment_count_them >= 1);
    assert!((last.talk_ratio_me + last.talk_ratio_them - 1.0).abs() < 0.05, "发言占比应合计≈1: {last:?}");
    eprintln!("pipeline 集成测试指标: 占比 我={:.2} 语速={:.0}WPM 提问={} 健康分={}", last.talk_ratio_me, last.pace_wpm, last.questions_me, last.health_score);

    // 4. 单流不应出现重复段（自动测试：同说话人相似段 = VAD 重复识别 → 检测报错）
    let final_segs: Vec<talksage_core::TranscriptSegment> = evs
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Segment { speaker_id, speaker_label, text, is_partial: false, ts_ms, duration_ms, .. } => Some(talksage_core::TranscriptSegment {
                speaker_id: *speaker_id,
                speaker_label: speaker_label.clone(),
                text: text.clone(),
                is_partial: false,
                ts_ms: *ts_ms,
                duration_ms: *duration_ms,
                rms: 0.0,
            }),
            _ => None,
        })
        .collect();
    let dups = talksage_session::find_duplicate_segments(&final_segs);
    assert!(dups.is_empty(), "单流出现疑似重复段（VAD 重复识别）: {dups:?}");
}

#[test]
fn file_input_partial_events_precede_final() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
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
fn dual_stream_user_and_client_both_produce_segments() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let zh_wav = zh_model_dir(&root).join("0.wav");
    let en_wav = en_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !zh_wav.is_file() || !en_wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
    }
    if !en_model_dir(&root).is_dir() {
        return skip("缺少英文模型");
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

/// 跨流回显去重：同一 wav 同时喂 user 与 client 流（模拟麦克风 + 系统回环双录），
/// 相同内容只保留一条——同一 final 文本不应同时出现在两个说话人。
#[test]
fn cross_stream_echo_dedup_keeps_single_copy() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
    }
    // 控制组：单流（该音频应有的段数）
    let single = run_and_collect(zh_file_pipeline(&root, &wav));
    let single_finals: Vec<String> = single
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Segment { text, is_partial: false, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(!single_finals.is_empty(), "单流应产生 final 段");

    // 双流：同一 wav 同时喂两条流（内容完全相同的回显场景）
    let mut cfg = zh_file_pipeline(&root, &wav);
    cfg.client = Some(StreamConfig {
        engine_kind: EngineKind::ParaformerZh,
        model_dir: zh_model_dir(&root),
        input: AudioInput::File(wav),
        speaker_id: 1,
        speaker_label: "客户".into(),
    });
    let evs = run_and_collect(cfg);
    let finals: Vec<(u32, String)> = evs
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Segment { speaker_id, text, is_partial: false, .. } => Some((*speaker_id, text.clone())),
            _ => None,
        })
        .collect();
    assert!(!finals.is_empty(), "双流去重后仍应有转写");

    // 同一文本不应同时出现在两个说话人
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for (sp, t) in &finals {
        if let Some(prev) = seen.get(t) {
            assert_eq!(prev, sp, "同一文本出现在不同说话人（回显去重失效）: {t}");
        } else {
            seen.insert(t.clone(), *sp);
        }
    }
    // 去重后总段数不超过单流段数
    assert!(
        finals.len() <= single_finals.len(),
        "回显未去重: 双流 {} 段 > 单流 {} 段",
        finals.len(),
        single_finals.len()
    );
    eprintln!("跨流去重：单流 {} 段，双流去重后 {} 段", single_finals.len(), finals.len());
}

/// 只数派发次数的 observer：在 should_trigger 里计数后返回 false，
/// 因此不产生骨架事件、不起线程、不碰 LLM —— 计数完全确定。
/// should_trigger 被调用，就等于这一段确实派发到了 observer。
struct CountingObserver {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl talksage_plugins::SegmentObserver for CountingObserver {
    fn name(&self) -> &'static str {
        "counting_observer"
    }

    fn should_trigger(&self, _seg: &talksage_core::TranscriptSegment) -> bool {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        false
    }

    fn skeleton(&self, _seg: &talksage_core::TranscriptSegment) -> Vec<DomainEvent> {
        Vec::new()
    }

    fn run(
        &self,
        _seg: &talksage_core::TranscriptSegment,
        _ctx: &talksage_plugins::PluginContext,
    ) -> Option<DomainEvent> {
        None
    }
}

fn count_finals(evs: &[DomainEvent]) -> usize {
    evs.iter()
        .filter(|e| matches!(e, DomainEvent::Segment { is_partial: false, .. }))
        .count()
}

/// 跑一遍管道，返回 (事件流, observer 派发次数)。
fn run_with_counting_observer(mut cfg: LivePipelineConfig) -> (Vec<DomainEvent>, usize) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    cfg.plugins = vec![Arc::new(CountingObserver { calls: calls.clone() })];
    let evs = run_and_collect(cfg);
    let n = calls.load(std::sync::atomic::Ordering::SeqCst);
    (evs, n)
}

/// 回归网：被 filter 吞掉的段绝不能派发到 observer。
///
/// 这是「filter 链放在产生点而不是 sink」的核心不变量。若有人把
/// `apply_filters` 挪回 emit 包装里，事件流看上去仍然是去重过的
/// （`cross_stream_echo_dedup_keeps_single_copy` 照样绿），但 `on_final`
/// 会对回声重复段照常派发 —— 术语/翻译/简报插件白跑一遍，可能带重复的
/// LLM 调用。那正是这次重构要修掉的缺陷，也只有这个测试能抓到它。
///
/// 判据是「派发次数 == 存活的 final 段数」，不依赖 ASR 具体识别出什么，
/// 因此与语料无关、可确定复现。
#[test]
fn filtered_segments_never_reach_observers() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
    }

    // 基线：单流没有回声可去重，派发次数应恰等于 final 段数。
    // 这一条同时保证测试非空转 —— 计数器确实在动。
    let (single, single_calls) = run_with_counting_observer(zh_file_pipeline(&root, &wav));
    let single_finals = count_finals(&single);
    assert!(single_finals > 0, "单流应产生 final 段: {single:?}");
    assert_eq!(
        single_calls, single_finals,
        "单流：每个 final 段应恰好派发一次 observer"
    );

    // 双流回声：同一 wav 同时喂两条流，一半的段会被 cross_stream_dedup 吞掉。
    let mut cfg = zh_file_pipeline(&root, &wav);
    cfg.client = Some(StreamConfig {
        engine_kind: EngineKind::ParaformerZh,
        model_dir: zh_model_dir(&root),
        input: AudioInput::File(wav),
        speaker_id: 1,
        speaker_label: "客户".into(),
    });
    let (dual, dual_calls) = run_with_counting_observer(cfg);
    let dual_finals = count_finals(&dual);
    assert!(dual_finals > 0, "双流去重后仍应有 final 段");

    // 核心断言：派发次数不多于存活段数。被吞掉的段一次都不许派发。
    assert_eq!(
        dual_calls, dual_finals,
        "被 filter 吞掉的段派发到了 observer：派发 {dual_calls} 次，但只有 {dual_finals} 个 final 段存活。\
         filter 链必须施加在产生点（emit 与 on_final 之前），不能只挡 emit"
    );
    // 附带：去重后不该退化成双份
    assert!(
        dual_calls <= single_finals,
        "双流派发 {dual_calls} 次 > 单流 {single_finals} 段，回声段仍在触发插件"
    );
    eprintln!(
        "observer 派发：单流 {single_calls} 次 / {single_finals} 段，双流 {dual_calls} 次 / {dual_finals} 段"
    );
}

/// 改写型 filter 的后缀。带一个空格 → 每段词数 +1，统计计数器是否用了
/// filter 之后的文本因此可判定。
const REWRITE_SUFFIX: &str = " [已改写]";

/// 测试替身：改写（而非丢弃）final 段文本的 filter。
/// 内置的两个 filter 只会 drop，第一个真正做变换的 filter（脱敏/标点/规范化）
/// 会暴露「sink 拿到改写后的，observer 与统计拿到原文」的错位。
struct RewriteFilter;

impl talksage_plugins::EventFilter for RewriteFilter {
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
        match ev {
            DomainEvent::Segment {
                speaker_id, speaker_label, text, is_partial: false, ts_ms,
                duration_ms, rms, revision, start_sample, end_sample,
            } => Some(DomainEvent::Segment {
                speaker_id,
                speaker_label,
                text: format!("{text}{REWRITE_SUFFIX}"),
                is_partial: false,
                ts_ms,
                duration_ms,
                rms,
                revision,
                start_sample,
                end_sample,
            }),
            other => Some(other),
        }
    }
}

/// 记录每次派发看到的段文本。
struct RecordingObserver {
    seen: Arc<Mutex<Vec<String>>>,
}

impl talksage_plugins::SegmentObserver for RecordingObserver {
    fn name(&self) -> &'static str {
        "recording_observer"
    }

    fn should_trigger(&self, seg: &talksage_core::TranscriptSegment) -> bool {
        self.seen.lock().unwrap().push(seg.text.clone());
        false
    }

    fn skeleton(&self, _seg: &talksage_core::TranscriptSegment) -> Vec<DomainEvent> {
        Vec::new()
    }

    fn run(
        &self,
        _seg: &talksage_core::TranscriptSegment,
        _ctx: &talksage_plugins::PluginContext,
    ) -> Option<DomainEvent> {
        None
    }
}

/// 回归网：filter 链是「变换」，不只是「丢弃」——observer 与统计计数器
/// 必须看 filter 之后的数据，否则落库/sink 的文本与观察者、words/questions
/// 会静默错位。
#[test]
fn observers_and_counters_see_the_filtered_text() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
    }

    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut cfg = zh_file_pipeline(&root, &wav);
    cfg.plugins = vec![Arc::new(RecordingObserver { seen: seen.clone() })];
    let mut hooks = hooks_with_min_commit_ms(0);
    hooks.add_filter(Arc::new(RewriteFilter));
    cfg.hooks = hooks;

    let evs = run_and_collect(cfg);

    // sink 侧：filter 确实改写了 final 文本
    let emitted: Vec<String> = evs
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Segment { text, is_partial: false, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(!emitted.is_empty(), "应产生 final 段: {evs:?}");
    assert!(
        emitted.iter().all(|t| t.ends_with(REWRITE_SUFFIX)),
        "filter 未生效，事件流里的文本没有改写: {emitted:?}"
    );

    // observer 侧：必须与 sink 侧逐条一致
    let observed = seen.lock().unwrap().clone();
    assert_eq!(
        observed, emitted,
        "observer 看到的是 filter 之前的文本 —— on_final 必须用 filter 之后的段，\
         否则落库/sink 的内容与插件看到的内容会静默错位"
    );

    // 统计计数器侧：words 必须按改写后的文本计
    let words: Vec<usize> = evs
        .iter()
        .filter_map(|e| match e {
            DomainEvent::SessionStats { words, .. } => Some(*words),
            _ => None,
        })
        .collect();
    let expected: usize = emitted.iter().map(|t| talksage_core::metrics::count_words(t)).sum();
    assert_eq!(
        words,
        vec![expected],
        "SessionStats.words 应按 filter 之后的文本统计（改写后每段多一个词）"
    );
}

/// 插件集成：英文客户文件 + term_explainer（mock LLM）+ translator → Term/Translation 事件。
#[test]
fn plugins_emit_term_and_translation_events() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let en_wav = en_model_dir(&root).join("0.wav");
    let en_model = en_model_dir(&root);
    if !en_model.is_dir() || !en_wav.is_file() || !vad_model(&root).is_file() {
        return skip("模型/VAD/测试音频不完整");
    }

    // mock LLM：术语解释返回固定文本；翻译返回固定文本
    let mock = talksage_llm::MockProvider {
        response: "NPI = New Product Introduction（新产品导入）\n翻译：在早夜降临后……".into(),
    };
    let ctx = talksage_plugins::PluginContext {
        kb: None,
        llm: Some(Arc::new(mock)),
    };
    let plugins: Vec<Arc<dyn talksage_plugins::SegmentObserver>> = vec![
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
        recording_dir: None,
        runtime: std::sync::Arc::new(talksage_pipeline::RuntimeParams::default()),
        speaker: None,
        engine_pool: None,
        hooks: hooks_with_min_commit_ms(0),
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
}

/// 录音端到端：file 输入 + recording_dir → 结束后 wav 文件存在且含音频数据。
#[test]
fn recording_saves_wav_files_for_each_stream() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
    }

    let rec_dir = std::env::temp_dir().join(format!("talksage-rec-test-{}", std::process::id()));
    let mut cfg = zh_file_pipeline(&root, &wav);
    cfg.recording_dir = Some(rec_dir.clone());
    let _evs = run_and_collect(cfg);

    // 用户流录音文件应存在且 > 44 字节（含音频数据）
    let files: Vec<PathBuf> = std::fs::read_dir(&rec_dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "wav").unwrap_or(false)).collect())
        .unwrap_or_default();
    assert!(!files.is_empty(), "应产生录音文件: {}", rec_dir.display());
    let f = &files[0];
    assert!(
        std::fs::metadata(f).map(|m| m.len() > 44).unwrap_or(false),
        "录音文件应包含音频数据: {}",
        f.display()
    );
    let _ = std::fs::remove_dir_all(&rec_dir);
}

/// 最短提交时长（min_commit_ms）：final 段时长低于阈值的丢弃（噪音短段抑制）。
/// 测试音频段约 2~3.5s：阈值 60s → 全部丢弃；阈值 0 → 正常提交。
#[test]
fn min_commit_ms_suppresses_short_segments() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
    }

    // 对照组：min_commit_ms=0 → 有 final 段
    let evs_off = run_and_collect(zh_file_pipeline(&root, &wav));
    let finals_off = evs_off
        .iter()
        .filter(|e| matches!(e, DomainEvent::Segment { is_partial: false, .. }))
        .count();
    assert!(finals_off > 0, "对照组应产生 final 段: {evs_off:?}");

    // 实验组：min_commit_ms=60_000（> 任何段时长）→ 全部丢弃
    let mut cfg = zh_file_pipeline(&root, &wav);
    cfg.hooks = hooks_with_min_commit_ms(60_000);
    let evs_on = run_and_collect(cfg);
    let finals_on = evs_on
        .iter()
        .filter(|e| matches!(e, DomainEvent::Segment { is_partial: false, .. }))
        .count();
    assert_eq!(finals_on, 0, "应丢弃全部短段: {evs_on:?}");
}

/// 时间戳可追溯到采样点：duration_ms = (end_sample - start_sample) / 16k。
#[test]
fn segment_timestamps_trace_to_samples() {
    let Some(root) = model_root() else {
        return skip("未找到 models/ 目录（设 TALKSAGE_MODELS_DIR）");
    };
    let wav = zh_model_dir(&root).join("0.wav");
    if !vad_model(&root).is_file() || !wav.is_file() {
        return skip("模型/VAD/测试音频不完整");
    }
    let evs = run_and_collect(zh_file_pipeline(&root, &wav));
    let finals: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            DomainEvent::Segment {
                is_partial: false,
                duration_ms,
                start_sample,
                end_sample,
                revision,
                ..
            } => Some((*duration_ms, *start_sample, *end_sample, *revision)),
            _ => None,
        })
        .collect();
    assert!(!finals.is_empty(), "应有 final 段: {evs:?}");
    let mut last_rev = 0u64;
    for (duration_ms, start, end, revision) in &finals {
        assert!(end >= start, "end_sample >= start_sample");
        let expected = talksage_core::AudioClock::samples_to_ms(16_000, end - start);
        assert_eq!(*duration_ms, expected, "duration_ms 应对齐采样时钟 start={start} end={end}");
        assert!(*revision > last_rev, "revision 应递增");
        last_rev = *revision;
    }
}
