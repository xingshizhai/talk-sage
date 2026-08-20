//! TalkSage v2 launcher。
//!
//! 结构参考 DeepSeek Harness：`dsh --profile web` 之类的启动器入口。
//! 已实现：version / doctor / listen（实时转写，麦克风或文件输入）。

use std::process::ExitCode;
use clap::{Parser, Subcommand};

use talksage_asr::{EngineKind, EnginePool};
use talksage_audio::AudioHub;
use talksage_core::DomainEvent;
use talksage_pipeline::{AudioInput, LivePipeline, LivePipelineConfig, StreamConfig};

#[derive(Parser)]
#[command(
    name = "拓思者",
    bin_name = "talksage",
    version = talksage_core::VERSION,
    about = "拓思者（TalkSage）— AI 会议助理：实时转写 · 说话人识别 · 纪要分析",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    /// 详细日志（等价 RUST_LOG=trace）
    #[arg(long, global = true)]
    verbose: bool,
    /// 日志级别（trace/debug/info/warn/error；默认读 RUST_LOG/TALKSAGE_LOG）
    #[arg(long, global = true, value_name = "LEVEL")]
    log_level: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 启动桌面应用（Tauri）。开发期请使用 `pnpm tauri dev`。
    Web,
    /// 启动 headless 服务（多设备/团队模式，预留）。
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// 实时转写验证：麦克风或 wav 文件 → VAD → 流式 ASR → 事件打印。
    Listen {
        /// 用户流输入：mic | loopback | <wav 路径>
        #[arg(long, default_value = "mic")]
        input: String,
        /// 客户流输入（英文 zipformer）：mic | loopback | <wav 路径>（可选）
        #[arg(long)]
        client: Option<String>,
        /// 简报知识库文件夹（可选，启用 brief_retriever）
        #[arg(long)]
        kb: Option<String>,
        /// 运行秒数（0 = 默认时长）
        #[arg(long, default_value_t = 0)]
        seconds: u64,
        /// 用户流引擎（paraformer-zh | zipformer-en）
        #[arg(long, default_value = "paraformer-zh")]
        engine: String,
        /// 落库到会话 SQLite（~/.talksage/sessions.db）
        #[arg(long)]
        save: bool,
        /// 不保存录音（默认按配置 recording.enabled 决定）
        #[arg(long)]
        no_record: bool,
        /// 运行时噪音电平阈值（0 = 关闭；0.005~0.05 常用），监听中可实时调节
        #[arg(long, default_value_t = 0.0)]
        noise_level: f32,
    },
    /// 导入音频离线转写并保存为新会话。
    Import {
        /// 音频文件路径（16kHz mono wav）
        path: String,
        /// 引擎（paraformer-zh | zipformer-en）
        #[arg(long, default_value = "paraformer-zh")]
        engine: String,
        /// 说话人标签（默认 导入）
        #[arg(long, default_value = "导入")]
        speaker: String,
    },
    /// 静音裁剪：用 silero VAD 去掉无声音的部分，输出紧凑音频（测试素材）。
    Trim {
        /// 输入 wav（任意采样率，自动重采样到 16k）
        path: String,
        /// 输出 wav（默认 <输入>.trimmed.wav）
        #[arg(short, long)]
        output: Option<String>,
        /// VAD 灵敏度预设（standard | sensitive | strict）
        #[arg(long, default_value = "standard")]
        preset: String,
        /// VAD 模型路径（默认 <models>/silero-vad/silero_vad.onnx）
        #[arg(long)]
        model: Option<String>,
    },
    /// 录制原始音频（不转写）：麦克风/回环 → wav。
    Record {
        /// 录制秒数（0 = 手动停止）
        #[arg(short, long, default_value_t = 30)]
        seconds: u64,
        /// 输出目录（默认 <data_dir>/recordings）
        #[arg(short, long)]
        dir: Option<String>,
        /// 输入源：mic | loopback
        #[arg(long, default_value = "mic")]
        input: String,
    },
    /// 诊断环境：配置、目录、平台。
    Doctor,
    /// 固定语料转写评测：CER/WER + 实时率(RTF) + 首词延迟（参考 WhisperLiveKit bench）。
    Bench {
        /// 语料目录（*.wav + 同名 .txt 参考文本；缺省 ./bench-corpus）
        #[arg(short, long)]
        dir: Option<String>,
        /// 引擎（paraformer-zh | zipformer-en）
        #[arg(long, default_value = "paraformer-zh")]
        engine: String,
        /// 只处理前 N 个文件
        #[arg(long)]
        limit: Option<usize>,
    },
    /// 打印版本。
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // 日志初始化（先于命令执行）
    if cli.verbose {
        std::env::set_var("TALKSAGE_LOG", "trace");
    } else if let Some(level) = &cli.log_level {
        std::env::set_var("TALKSAGE_LOG", level);
    }
    let _log_guard = talksage_logging::init(None);
    log::info!("talksage {} 启动", talksage_core::VERSION);

    match cli.command {
        Command::Version => {
            println!("拓思者（TalkSage）{}", talksage_core::VERSION);
            ExitCode::SUCCESS
        }
        Command::Web => cmd_web(),
        Command::Serve { host, port } => cmd_serve(host, port),
        Command::Listen {
            input,
            seconds,
            engine,
            client,
            kb,
            save,
            no_record,
            noise_level,
        } => cmd_listen(&input, seconds, &engine, client.as_deref(), kb.as_deref(), save, no_record, noise_level),
        Command::Import { path, engine, speaker } => cmd_import(&path, &engine, &speaker),
        Command::Trim {
            path,
            output,
            preset,
            model,
        } => cmd_trim(&path, output.as_deref(), &preset, model.as_deref()),
        Command::Record { seconds, dir, input } => cmd_record(seconds, dir.as_deref(), &input),
        Command::Bench { dir, engine, limit } => cmd_bench(dir.as_deref(), &engine, limit),
        Command::Doctor => cmd_doctor(),
    }
}

fn cmd_web() -> ExitCode {
    println!(
        "拓思者（TalkSage）{} — 桌面模式\n\
         \n\
         开发期请使用: pnpm --dir web tauri dev\n\
         构建后: 直接运行桌面应用安装包。",
        talksage_core::VERSION
    );
    ExitCode::SUCCESS
}

fn cmd_serve(host: String, port: u16) -> ExitCode {
    // SPA 静态目录（web/dist）
    let web_dist = match std::env::var("TALKSAGE_WEB_DIST") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => {
            let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
            let candidates = [
                here.join("../web/dist"),
                here.join("../../web/dist"),
                std::path::PathBuf::from("web/dist"),
            ];
            candidates.into_iter().find(|c| c.is_dir()).unwrap_or_else(|| std::path::PathBuf::from("web/dist"))
        }
    };
    if !web_dist.is_dir() {
        eprintln!("未找到前端构建产物（{web_dist:?}）。请先运行: cd web && npm run build");
        return ExitCode::FAILURE;
    }
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("tokio runtime 失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let token = std::env::var("TALKSAGE_SERVER_TOKEN").unwrap_or_default();
    match rt.block_on(talksage_server::run(&host, port, &token, &web_dist)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("服务退出: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_listen(
    input: &str,
    seconds: u64,
    engine: &str,
    client_input: Option<&str>,
    kb_folder: Option<&str>,
    save: bool,
    no_record: bool,
    noise_level: f32,
) -> ExitCode {
    let kind = match EngineKind::from_name(engine) {
        Some(k) => k,
        None => {
            eprintln!("未知引擎: {engine}（可选 paraformer-zh | zipformer-en）");
            return ExitCode::FAILURE;
        }
    };
    let model_dir = match resolve_models_dir() {
        Some(d) => d,
        None => {
            eprintln!("未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）");
            return ExitCode::FAILURE;
        }
    };
    let engine_dir = match kind {
        EngineKind::ParaformerZh => model_dir.join("sherpa-onnx-streaming-paraformer-zh"),
        EngineKind::ZipformerEn => model_dir.join("sherpa-onnx-streaming-zipformer-en-2023-06-26"),
    };
    let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
    if !vad_model.is_file() {
        eprintln!("缺少 VAD 模型: {}", vad_model.display());
        return ExitCode::FAILURE;
    }
    if !engine_dir.is_dir() {
        eprintln!("缺少 ASR 模型目录: {}", engine_dir.display());
        return ExitCode::FAILURE;
    }

    let parse_input = |s: &str| -> Result<talksage_pipeline::AudioInput, String> {
        if s.eq_ignore_ascii_case("mic") {
            Ok(talksage_pipeline::AudioInput::Mic(None))
        } else if s.eq_ignore_ascii_case("loopback") {
            Ok(talksage_pipeline::AudioInput::Loopback)
        } else {
            let p = std::path::PathBuf::from(s);
            if !p.is_file() {
                Err(format!("wav 文件不存在: {s}（或使用 mic / loopback）"))
            } else {
                Ok(talksage_pipeline::AudioInput::File(p))
            }
        }
    };

    let user_input = match parse_input(input) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    // 客户流（英文 zipformer，可选）
    let client_cfg = match client_input {
        Some(c) => {
            let en_dir = model_dir.join("sherpa-onnx-streaming-zipformer-en-2023-06-26");
            if !en_dir.is_dir() {
                eprintln!("缺少英文 ASR 模型目录: {}", en_dir.display());
                return ExitCode::FAILURE;
            }
            let ci = match parse_input(c) {
                Ok(i) => i,
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            };
            Some(talksage_pipeline::StreamConfig {
                engine_kind: EngineKind::ZipformerEn,
                model_dir: en_dir,
                input: ci,
                speaker_id: 1,
                speaker_label: "客户".into(),
            })
        }
        None => None,
    };

    let is_file_input = matches!(user_input, AudioInput::File(_));

    // 插件上下文：LLM（配置）+ 知识库（--kb 或配置）
    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("配置加载失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let snapshot = mgr.snapshot();
    let llm: Option<std::sync::Arc<dyn talksage_llm::LLMProvider>> = {
        let name = snapshot.llm.default.clone();
        snapshot
            .llm
            .providers
            .get(&name)
            .filter(|p| !p.api_key.is_empty() || name == "ollama")
            .map(|p| -> std::sync::Arc<dyn talksage_llm::LLMProvider> {
                std::sync::Arc::new(talksage_llm::OpenAICompatProvider::new(
                    p.api_key.clone(),
                    p.model.clone(),
                    p.base_url.clone().unwrap_or_else(|| "https://api.deepseek.com/v1".to_string()),
                ))
            })
    };
    let kb: Option<std::sync::Arc<talksage_knowledge::KnowledgeBase>> = {
        let folder = kb_folder
            .map(|s| s.to_string())
            .or_else(|| {
                if snapshot.knowledge_base.enabled {
                    Some(snapshot.knowledge_base.folder.clone())
                } else {
                    None
                }
            });
        folder
            .filter(|f| !f.is_empty())
            .and_then(|f| {
                let mut kb = talksage_knowledge::KnowledgeBase::new();
                kb.index_folder(std::path::Path::new(&f));
                if kb.chunk_count() > 0 {
                    Some(std::sync::Arc::new(kb))
                } else {
                    eprintln!("知识库目录无 .md/.txt 内容: {f}");
                    None
                }
            })
    };
    let mut plugins: Vec<std::sync::Arc<dyn talksage_plugins::AnalyzerPlugin>> = Vec::new();
    // 场景模式：有效参数（CLI 的 --engine/--client 仍优先，其余跟随场景）
    let scene = snapshot.scene.effective();
    if scene.term_enabled && snapshot.plugins.term_explainer.enabled {
        plugins.push(std::sync::Arc::new(talksage_plugins::term_explainer::TermExplainerPlugin::new(
            snapshot.plugins.term_explainer.cooldown_seconds as f64,
        )));
    }
    if scene.translation_enabled && snapshot.plugins.translator.enabled {
        plugins.push(std::sync::Arc::new(talksage_plugins::translator::TranslatorPlugin::new()));
    }
    if scene.brief_enabled && snapshot.plugins.brief_retriever.enabled && kb.is_some() {
        plugins.push(std::sync::Arc::new(talksage_plugins::brief_retriever::BriefRetrieverPlugin::new(
            snapshot.plugins.brief_retriever.cooldown_seconds as f64,
            0.05,
        )));
    }
    let plugin_ctx = talksage_plugins::PluginContext { kb, llm };

    // 录音目录（--no-record 或配置关闭时禁用）
    let recording_dir = if no_record || !snapshot.recording.enabled {
        None
    } else {
        let dir = snapshot.recording.resolve_dir(mgr.data_dir());
        std::fs::create_dir_all(&dir)
            .map_err(|e| {
                eprintln!("创建录音目录失败: {e}");
                ExitCode::FAILURE
            })
            .ok();
        Some(dir)
    };
    if let Some(d) = &recording_dir {
        println!("录音保存目录: {}", d.display());
    }

    let cfg = LivePipelineConfig {
        vad_model,
        chunk_ms: 100,
        vad: scene.to_vad_config(),
        denoise: scene.to_denoise_config(),
        asr_threads: 4,
        user: StreamConfig {
            engine_kind: kind,
            model_dir: engine_dir,
            input: user_input,
            speaker_id: 0,
            speaker_label: "我".into(),
        },
        client: client_cfg,
        plugins,
        plugin_ctx,
        recording_dir,
        runtime: std::sync::Arc::new(talksage_pipeline::RuntimeParams::with_noise_level(noise_level)),
        speaker: if scene.speaker_enabled { build_speaker_config(&mgr) } else { None },
        engine_pool: None,
        min_commit_ms: scene.min_segment_ms,
    };

    let stop_after = seconds;
    let mut pipeline = LivePipeline::new(cfg);

    // 可选会话落库
    let session_store = if save {
        let data_dir = talksage_config::default_data_dir();
        let db_path = data_dir.join("sessions.db");
        match talksage_session::SessionStore::open(&db_path.to_string_lossy()) {
            Ok(s) => Some(std::sync::Arc::new(s)),
            Err(e) => {
                eprintln!("打开会话库失败（忽略落库）: {e}");
                None
            }
        }
    } else {
        None
    };
    let current_session = std::sync::Arc::new(std::sync::Mutex::new(None));
    if let Some(store) = &session_store {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        match store.start_session(now) {
            Ok(sid) => *current_session.lock().unwrap() = Some(sid),
            Err(e) => eprintln!("开启会话失败: {e}"),
        }
    }
    let sessions_for_sink = session_store.clone();
    let current_for_sink = current_session.clone();
    // 会话统计收集（质量评估；save 模式落库）
    let stats_for_sink: std::sync::Arc<std::sync::Mutex<Vec<talksage_session::StreamMeta>>> = Default::default();
    let texts_for_sink: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let stats_sink = stats_for_sink.clone();
    let texts_sink = texts_for_sink.clone();

    let sink: std::sync::Arc<dyn Fn(DomainEvent) + Send + Sync> = std::sync::Arc::new(move |ev: DomainEvent| {
        // 落库（可选）
        if let (Some(store), Ok(guard)) = (&sessions_for_sink, current_for_sink.lock()) {
            if let Some(sid) = *guard {
                match &ev {
                    DomainEvent::Segment { text, is_partial: false, speaker_id, speaker_label, ts_ms, duration_ms, rms, .. } => {
                        if let Ok(mut t) = texts_sink.lock() {
                            t.push(text.clone());
                        }
                        let _ = store.add_segment(
                            sid,
                            &talksage_core::TranscriptSegment {
                                speaker_id: *speaker_id,
                                speaker_label: speaker_label.clone(),
                                text: text.clone(),
                                is_partial: false,
                                ts_ms: *ts_ms,
                                duration_ms: *duration_ms,
                                rms: *rms,
                            },
                        );
                    }
                    DomainEvent::Term { status: talksage_core::ResultStatus::Final, content, .. } => {
                        let _ = store.add_term(sid, content);
                    }
                    DomainEvent::Translation { content, .. } => {
                        let _ = store.add_translation(sid, "translate", content);
                    }
                    _ => {}
                }
            }
        }
        // 统计收集
        if let DomainEvent::SessionStats {
            speaker_label,
            total_ms,
            speech_ms,
            final_segments,
            avg_rms,
            max_rms,
            non_speech_avg_rms,
            recording,
            vad_preset,
            vad_threshold,
            words,
            questions,
            ..
        } = &ev
        {
            if let Ok(mut sm) = stats_sink.lock() {
                sm.push(talksage_session::StreamMeta {
                    speaker_label: speaker_label.clone(),
                    total_ms: *total_ms,
                    speech_ms: *speech_ms,
                    final_segments: *final_segments,
                    avg_rms: *avg_rms,
                    max_rms: *max_rms,
                    non_speech_avg_rms: *non_speech_avg_rms,
                    recording: recording.clone(),
                    vad_preset: vad_preset.clone(),
                    vad_threshold: *vad_threshold,
                    words: *words,
                    questions: *questions,
                });
            }
        }
        // 打印
        match &ev {
            DomainEvent::Status { stage, message } => {
                println!("[status] {:?}: {}", stage, message);
            }
            DomainEvent::Segment {
                speaker_label,
                text,
                is_partial,
                ..
            } => {
                if *is_partial {
                    print!("\r[{speaker_label}] {text} ▍");
                } else {
                    println!("\n[{speaker_label}] {text}");
                }
            }
            DomainEvent::SessionStats {
                speaker_label,
                total_ms,
                speech_ms,
                final_segments,
                avg_rms,
                max_rms,
                recording,
                ..
            } => {
                println!(
                    "\n[stats] [{speaker_label}] total={total_ms}ms speech={speech_ms}ms({:.0}%) segs={final_segments} avg_rms={avg_rms:.4} max_rms={max_rms:.4} recording={recording:?}",
                    if *total_ms > 0 { *speech_ms as f64 / *total_ms as f64 * 100.0 } else { 0.0 },
                );
            }
            other => println!("[event] {other:?}"),
        }
    });
    if let Err(e) = pipeline.start(sink) {
        eprintln!("启动失败: {e}");
        return ExitCode::FAILURE;
    }
    println!("监听中…（Ctrl+C 或超时停止）");

    if stop_after > 0 {
        std::thread::sleep(std::time::Duration::from_secs(stop_after));
    } else if is_file_input {
        // 文件模式：等 pipeline 自然结束（文件读完即停）
        std::thread::sleep(std::time::Duration::from_secs(60));
    } else {
        // mic / loopback 模式：默认采集 30 秒（无终端环境 stdin 会立即 EOF）
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
    pipeline.stop();
    // 结束会话 + 质量评估落库
    if let Some(store) = &session_store {
        if let Some(sid) = current_session.lock().unwrap().take() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let _ = store.end_session(sid, now);
            let stats = stats_for_sink.lock().unwrap().clone();
            let texts = texts_for_sink.lock().unwrap().clone();
            if !stats.is_empty() {
                let params = talksage_session::QualityParams::from_config(&snapshot.quality);
                let meta = talksage_session::SessionMeta::evaluate(stats, &texts, now, &params);
                let _ = store.set_session_meta(sid, &meta);
                println!(
                    "会话 #{sid} 质量: {}（时长 {}s，语音占比 {:.0}%，文本噪音 {:.2}，跳过下游分析={}）",
                    meta.quality_label(),
                    meta.duration_ms / 1000,
                    meta.speech_ratio * 100.0,
                    meta.text_noise,
                    meta.skipped_analysis,
                );
            }
            println!("会话 #{sid} 已保存");
        }
    }
    println!("\n已停止。");
    ExitCode::SUCCESS
}

fn cmd_import(path: &str, engine: &str, speaker_label: &str) -> ExitCode {
    let kind = match EngineKind::from_name(engine) {
        Some(k) => k,
        None => {
            eprintln!("未知引擎: {engine}（可选 paraformer-zh | zipformer-en）");
            return ExitCode::FAILURE;
        }
    };
    let model_dir = match resolve_models_dir() {
        Some(d) => d,
        None => {
            eprintln!("未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）");
            return ExitCode::FAILURE;
        }
    };
    let engine_dir = match kind {
        EngineKind::ParaformerZh => model_dir.join("sherpa-onnx-streaming-paraformer-zh"),
        EngineKind::ZipformerEn => model_dir.join("sherpa-onnx-streaming-zipformer-en-2023-06-26"),
    };
    let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
    if !vad_model.is_file() || !engine_dir.is_dir() {
        eprintln!("模型不完整（VAD 或 ASR 模型缺失），请先运行 scripts/download_models.py");
        return ExitCode::FAILURE;
    }
    let audio_path = std::path::PathBuf::from(path);
    if !audio_path.is_file() {
        eprintln!("文件不存在: {path}");
        return ExitCode::FAILURE;
    }

    println!("导入转写: {path}（{engine}）…");
    let cfg = LivePipelineConfig {
        vad_model,
        chunk_ms: 100,
        vad: talksage_config::VadConfig::default(),
        denoise: talksage_config::DenoiseConfig::default(),
        asr_threads: 4,
        user: StreamConfig {
            engine_kind: kind,
            model_dir: engine_dir,
            input: AudioInput::File(audio_path),
            speaker_id: 0,
            speaker_label: speaker_label.to_string(),
        },
        client: None,
        plugins: Vec::new(),
        plugin_ctx: talksage_plugins::PluginContext::new(),
        recording_dir: None,
        runtime: std::sync::Arc::new(talksage_pipeline::RuntimeParams::default()),
        speaker: None,
        engine_pool: None,
        min_commit_ms: 0,
    };

    // 收集 final 段
    let segments = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let segs_for_sink = segments.clone();
    let done_for_sink = done.clone();
    let sink: std::sync::Arc<dyn Fn(DomainEvent) + Send + Sync> = std::sync::Arc::new(move |ev| {
        match &ev {
            DomainEvent::Segment { text, is_partial: false, speaker_id, speaker_label, ts_ms, duration_ms, rms, .. } => {
                segs_for_sink.lock().unwrap().push(talksage_core::TranscriptSegment {
                    speaker_id: *speaker_id,
                    speaker_label: speaker_label.clone(),
                    text: text.clone(),
                    is_partial: false,
                    ts_ms: *ts_ms,
                    duration_ms: *duration_ms,
                    rms: *rms,
                });
            }
            DomainEvent::Status { stage: talksage_core::StatusStage::Idle, .. } => {
                done_for_sink.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            _ => {}
        }
    });

    let mut pipeline = LivePipeline::new(cfg);
    if let Err(e) = pipeline.start(sink) {
        eprintln!("启动失败: {e}");
        return ExitCode::FAILURE;
    }

    // 等待完成（文件模式自然结束）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while !done.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    pipeline.stop();

    let segs = segments.lock().unwrap();
    if segs.is_empty() {
        eprintln!("未识别到语音内容");
        return ExitCode::FAILURE;
    }

    // 保存新会话
    let data_dir = talksage_config::default_data_dir();
    let store = match talksage_session::SessionStore::open(&data_dir.join("sessions.db").to_string_lossy()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("打开会话库失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    match store.start_session(now) {
        Ok(sid) => {
            for s in segs.iter() {
                let _ = store.add_segment(sid, s);
            }
            let _ = store.end_session(sid, now);
            println!("\n已保存会话 #{sid}（{} 段）", segs.len());
        }
        Err(e) => {
            eprintln!("保存会话失败: {e}");
            return ExitCode::FAILURE;
        }
    }

    println!("转写结果（{} 段）:", segs.len());
    for s in segs.iter() {
        println!("  [{}] {}", s.speaker_label, s.text);
    }
    ExitCode::SUCCESS
}

/// 构造说话人识别配置：wespeaker 模型 + 已注册的主人声纹（有则启用）。
fn build_speaker_config(mgr: &talksage_config::ConfigManager) -> Option<talksage_pipeline::SpeakerConfig> {
    let model_dir = resolve_models_dir()?;
    let model = model_dir.join("wespeaker").join("wespeaker_zh_cnceleb_resnet34.onnx");
    if !model.is_file() {
        return None;
    }
    let owner = talksage_pipeline::speaker::load_owner_embedding(mgr.data_dir());
    Some(talksage_pipeline::SpeakerConfig {
        model,
        owner_embedding: owner,
        threshold: talksage_pipeline::speaker::DEFAULT_THRESHOLD,
    })
}

fn cmd_trim(path: &str, output: Option<&str>, preset: &str, model: Option<&str>) -> ExitCode {
    let input = std::path::PathBuf::from(path);
    if !input.is_file() {
        eprintln!("文件不存在: {path}");
        return ExitCode::FAILURE;
    }
    // VAD 模型路径
    let vad_model = match model {
        Some(m) => std::path::PathBuf::from(m),
        None => match resolve_models_dir() {
            Some(d) => d.join("silero-vad").join("silero_vad.onnx"),
            None => {
                eprintln!("未找到模型目录（可设 TALKSAGE_MODELS_DIR 或 --model）");
                return ExitCode::FAILURE;
            }
        },
    };
    if !vad_model.is_file() {
        eprintln!("缺少 VAD 模型: {}", vad_model.display());
        return ExitCode::FAILURE;
    }
    let output = match output {
        Some(o) => std::path::PathBuf::from(o),
        None => {
            let mut s = input.clone().into_os_string();
            s.push(".trimmed.wav");
            std::path::PathBuf::from(s)
        }
    };
    // VAD 预设
    let mut vad_cfg = talksage_config::VadConfig::default();
    vad_cfg.preset = match preset {
        "sensitive" => talksage_config::VadPreset::Sensitive,
        "strict" => talksage_config::VadPreset::Strict,
        _ => talksage_config::VadPreset::Standard,
    };

    println!("静音裁剪: {path}");
    println!("  VAD: {}（preset={}）", vad_model.display(), preset);
    match talksage_audio::silence_trim::trim_silence(&input, &output, &vad_model, &vad_cfg) {
        Ok(stats) => {
            println!(
                "  完成: 输入 {:.1}s → 输出 {:.1}s（去掉 {:.1}s 静音，{} 段语音，压缩率 {:.0}%）",
                stats.input_ms as f64 / 1000.0,
                stats.output_ms as f64 / 1000.0,
                stats.removed_ms as f64 / 1000.0,
                stats.speech_segments,
                stats.compression_ratio() * 100.0,
            );
            if stats.output_samples == 0 {
                eprintln!("  警告: 未检测到语音内容，输出为空。可尝试 --preset sensitive。");
            }
            println!("  输出: {}", stats.output_path);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("裁剪失败: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_record(seconds: u64, dir: Option<&str>, input: &str) -> ExitCode {
    let out_dir = match dir {
        Some(d) => std::path::PathBuf::from(d),
        None => {
            let mgr = match talksage_config::ConfigManager::load(None, None) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("配置加载失败: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let s = mgr.snapshot();
            s.recording.resolve_dir(mgr.data_dir())
        }
    };
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("创建目录失败: {e}");
        return ExitCode::FAILURE;
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let label = if input.eq_ignore_ascii_case("loopback") { "loopback" } else { "mic" };
    let path = out_dir.join(format!("record_{ts}_{label}.wav"));

    if input.eq_ignore_ascii_case("loopback") {
        #[cfg(windows)]
        {
            let (mut cap, rx) = talksage_audio::LoopbackCapture::new(100);
            if let Err(e) = cap.start() {
                eprintln!("启动回环采集失败: {e}");
                return ExitCode::FAILURE;
            }
            let mut rec = match talksage_audio::wav::WavRecorder::create(&path, 16000) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("创建录音文件失败: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds.max(1));
            loop {
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(c) => {
                        let _ = rec.write(&c);
                    }
                    Err(_) => {}
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
            }
            cap.stop();
            match rec.finish() {
                Ok(()) => {
                    println!("录制完成: {}", path.display());
                    println!("提示: 可用 `talksage trim {}` 去掉静音", path.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("录制失败: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        #[cfg(not(windows))]
        {
            eprintln!("系统回环采集当前仅支持 Windows");
            ExitCode::FAILURE
        }
    } else {
        cmd_record_mic(&path, seconds)
    }
}

/// 麦克风录音：采集 → wav（不转写）。
fn cmd_record_mic(path: &std::path::Path, seconds: u64) -> ExitCode {
    let (mut hub, rx) = AudioHub::new(100);
    if let Err(e) = hub.start(None) {
        eprintln!("启动麦克风失败: {e}");
        return ExitCode::FAILURE;
    }
    let mut rec = match talksage_audio::wav::WavRecorder::create(path, 16000) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("创建录音文件失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds.max(1));
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(c) => {
                let _ = rec.write(&c);
            }
            Err(_) => {}
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
    }
    hub.stop();
    match rec.finish() {
        Ok(()) => {
            println!("录制完成: {}", path.display());
            println!("提示: 可用 `talksage trim {}` 去掉静音", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("录制失败: {e}");
            ExitCode::FAILURE
        }
    }
}

/// 固定语料转写评测：对 `*.wav` 逐个跑流式转写（引擎池热启动），
/// 有同名 `.txt` 参考文本时计算 CER（中文）/ WER（英文），并输出
/// 实时率 RTF（处理耗时/音频时长）与首词延迟（管道启动→首个 final 段）。
fn cmd_bench(dir: Option<&str>, engine: &str, limit: Option<usize>) -> ExitCode {
    let kind = match EngineKind::from_name(engine) {
        Some(k) => k,
        None => {
            eprintln!("未知引擎: {engine}（可选 paraformer-zh | zipformer-en）");
            return ExitCode::FAILURE;
        }
    };
    let model_dir = match resolve_models_dir() {
        Some(d) => d,
        None => {
            eprintln!("未找到 models/ 目录（可设 TALKSAGE_MODELS_DIR）");
            return ExitCode::FAILURE;
        }
    };
    let engine_dir = match kind {
        EngineKind::ParaformerZh => model_dir.join("sherpa-onnx-streaming-paraformer-zh"),
        EngineKind::ZipformerEn => model_dir.join("sherpa-onnx-streaming-zipformer-en-2023-06-26"),
    };
    let vad_model = model_dir.join("silero-vad").join("silero_vad.onnx");
    if !vad_model.is_file() || !engine_dir.is_dir() {
        eprintln!("模型不完整（VAD 或 ASR 模型缺失）");
        return ExitCode::FAILURE;
    }

    let dir = std::path::PathBuf::from(dir.unwrap_or("bench-corpus"));
    if !dir.is_dir() {
        eprintln!("语料目录不存在: {}（准备 *.wav + 同名 .txt 参考文本）", dir.display());
        return ExitCode::FAILURE;
    }
    let mut wavs: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| {
            eprintln!("读取语料目录失败: {e}");
            ExitCode::FAILURE
        })
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".wav"))
        .map(|e| e.path())
        .collect();
    wavs.sort();
    if let Some(n) = limit {
        wavs.truncate(n);
    }
    if wavs.is_empty() {
        eprintln!("语料目录无 .wav 文件");
        return ExitCode::FAILURE;
    }

    let pool = EnginePool::new();
    println!("== 拓思者 bench ==");
    println!("语料: {}（{} 个文件） 引擎: {}（引擎池热启动）", dir.display(), wavs.len(), kind.display_name());
    println!("{:<36} {:>8} {:>8} {:>10} {:>12}", "文件", "时长s", "CER/WER%", "RTF", "首词延迟ms");
    println!("{}", "-".repeat(80));

    let mut total_audio = 0.0f64;
    let mut total_elapsed = 0.0f64;
    let mut total_err = 0.0f64;
    let mut total_err_n = 0usize;
    let mut total_latency = 0.0f64;
    let mut latency_n = 0usize;

    for wav in &wavs {
        let base = wav.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ref_path = [dir.join(format!("{base}.txt")), dir.join(format!("{base}.ref.txt"))]
            .into_iter()
            .find(|p| p.is_file());
        let reference = ref_path.and_then(|p| std::fs::read_to_string(p).ok());
        let audio_secs = talksage_audio::wav::read_wav(wav)
            .map(|(_, s)| s.len() as f64 / 16000.0)
            .unwrap_or(0.0);

        match run_bench_pipeline(&pool, wav, kind, &engine_dir, &vad_model) {
            Ok(tr) => {
                let (elapsed_ms, latency_ms) = (tr.elapsed_ms, tr.first_latency_ms);
                let rtf = if audio_secs > 0.0 { elapsed_ms / 1000.0 / audio_secs } else { 0.0 };
                total_audio += audio_secs;
                total_elapsed += elapsed_ms / 1000.0;
                if let Some(lt) = latency_ms {
                    total_latency += lt;
                    latency_n += 1;
                }
                let err = reference.as_deref().map(|r| {
                    if kind == EngineKind::ParaformerZh {
                        talksage_core::cer(r, &tr.text)
                    } else {
                        talksage_core::wer(r, &tr.text)
                    }
                });
                if let Some(e) = err {
                    total_err += e as f64;
                    total_err_n += 1;
                    println!(
                        "{:<36} {:>8.1} {:>7.1}% {:>9.2} {:>11.0}",
                        base, audio_secs, e * 100.0, rtf, latency_ms.unwrap_or(0.0)
                    );
                } else {
                    println!(
                        "{:<36} {:>8.1} {:>8} {:>9.2} {:>11.0}",
                        base, audio_secs, "-", rtf, latency_ms.unwrap_or(0.0)
                    );
                }
            }
            Err(e) => {
                eprintln!("  [{base}] 失败: {e}");
            }
        }
    }

    println!("{}", "-".repeat(80));
    let avg_rtf = if total_audio > 0.0 { total_elapsed / total_audio } else { 0.0 };
    let avg_err = if total_err_n > 0 { total_err / total_err_n as f64 } else { f64::NAN };
    let avg_latency = if latency_n > 0 { total_latency / latency_n as f64 } else { f64::NAN };
    println!(
        "平均: RTF={avg_rtf:.2}{} 首词延迟={:.0}ms{}",
        if total_err_n > 0 {
            format!("  CER/WER={:.1}%", avg_err * 100.0)
        } else {
            "  （无参考文本，未计算 CER/WER）".to_string()
        },
        avg_latency,
        if latency_n == 0 { "（无 final 段）".to_string() } else { String::new() },
    );
    println!("RTF < 1.0 表示实时；越小越快。准备参考文本（<同名>.txt）即可得到准确率。");
    ExitCode::SUCCESS
}

/// 对单个 wav 跑流式转写（共享 talksage_pipeline::offline::transcribe_file，
/// 引擎池热启动；与 headless 转写 API 同一条路径）。
fn run_bench_pipeline(
    pool: &std::sync::Arc<EnginePool>,
    wav: &std::path::Path,
    kind: EngineKind,
    engine_dir: &std::path::Path,
    vad_model: &std::path::Path,
) -> anyhow::Result<talksage_pipeline::offline::FileTranscription> {
    talksage_pipeline::offline::transcribe_file(Some(pool), kind, engine_dir, vad_model, wav)
}

fn cmd_doctor() -> ExitCode {
    println!("== 拓思者（TalkSage）doctor ==");
    println!("version      : {}", talksage_core::VERSION);

    let data_dir = talksage_config::default_data_dir();
    println!("data dir     : {}", data_dir.display());
    let config_file = data_dir.join("talksage.toml");
    println!(
        "config file  : {} ({})",
        config_file.display(),
        if config_file.exists() { "存在" } else { "不存在（将使用内置默认）" }
    );

    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => m,
        Err(e) => {
            println!("config error : {e}");
            return ExitCode::FAILURE;
        }
    };
    let c = mgr.snapshot();
    println!("asr engines  : client={} user={} backend={}", c.asr.client_engine, c.asr.user_engine, c.asr.backend);
    println!("server       : enabled={} ({}:{})", c.server.enabled, c.server.host, c.server.port);
    println!("plugins      : term={} translator={} brief={}",
        c.plugins.term_explainer.enabled, c.plugins.translator.enabled, c.plugins.brief_retriever.enabled);
    let rec_dir = c.recording.resolve_dir(mgr.data_dir());
    println!("recording    : enabled={} dir={}（{}）",
        c.recording.enabled,
        rec_dir.display(),
        if rec_dir.is_dir() { format!("{} 个文件", count_wav(&rec_dir)) } else { "目录不存在".into() });
    println!("quality      : auto_detect={} text_noise={} min_ratio={} max_ratio={} silence_rms={} high_rms={}",
        c.quality.auto_detect, c.quality.text_noise_threshold, c.quality.min_speech_ratio,
        c.quality.max_speech_ratio, c.quality.silence_rms, c.quality.high_rms);

    println!("\n模型检查:");
    match resolve_models_dir() {
        Some(d) => {
            println!("models dir   : {}", d.display());
            for sub in ["sherpa-onnx-streaming-paraformer-zh", "sherpa-onnx-streaming-zipformer-en-2023-06-26", "silero-vad"] {
                let p = d.join(sub);
                println!("  {sub}: {}", if p.exists() { "✓" } else { "✗" });
            }
        }
        None => println!("models dir   : 未找到（可设 TALKSAGE_MODELS_DIR）"),
    }

    println!("\n平台信息:");
    println!("os           : {}", std::env::consts::OS);
    println!("arch         : {}", std::env::consts::ARCH);

    println!("\ndoctor 完成。");
    ExitCode::SUCCESS
}

/// 统计目录内 wav 文件数。
fn count_wav(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".wav"))
                .count()
        })
        .unwrap_or(0)
}

/// 解析模型根目录：优先环境变量，其次相对可执行文件/当前目录探测。
fn resolve_models_dir() -> Option<std::path::PathBuf> {
    if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
        let p = std::path::PathBuf::from(d);
        if p.is_dir() {
            return Some(p);
        }
    }
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(base) = exe.parent() {
            candidates.push(base.join("../../models"));
            candidates.push(base.join("../../../models"));
        }
    }
    candidates.push(std::path::PathBuf::from("models"));
    candidates.push(std::path::PathBuf::from("../models"));
    candidates.into_iter().find(|c| c.is_dir())
}
