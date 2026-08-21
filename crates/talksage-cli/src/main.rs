//! TalkSage v2 launcher。
//!
//! 结构参考 DeepSeek Harness：`dsh --profile web` 之类的启动器入口。
//! 已实现：version / doctor / listen（实时转写，麦克风或文件输入）。

use std::process::ExitCode;
use clap::{Parser, Subcommand};

use talksage_asr::{EngineKind, EnginePool};
use talksage_audio::AudioHub;
use talksage_core::DomainEvent;
use talksage_pipeline::{AudioInput, StartListen, TalkSageService};

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
    /// 会话分析：转储某会话的原始段信息（时间戳/时长/文本），并检测疑似重复段。
    /// 用于排查"识别内容重复"等转写质量问题。
    Session {
        /// 会话 id
        id: i64,
        /// 只检测重复段（不打印全部转写）
        #[arg(long)]
        dup_only: bool,
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
    /// 固定语料转写评测：CER/WER + 端到端实时系数 + 首段 final 延迟。
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
        Command::Session { id, dup_only } => cmd_session(id, dup_only),
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
            eprintln!("未知引擎: {engine}（可选 paraformer-zh | zipformer-en | whisper-base | whisper-small | qwen3-asr）");
            return ExitCode::FAILURE;
        }
    };

    let parse_input = |s: &str| -> Result<AudioInput, String> {
        if s.eq_ignore_ascii_case("mic") {
            Ok(AudioInput::Mic(None))
        } else if s.eq_ignore_ascii_case("loopback") {
            Ok(AudioInput::Loopback)
        } else {
            let p = std::path::PathBuf::from(s);
            if !p.is_file() {
                Err(format!("wav 文件不存在: {s}（或使用 mic / loopback）"))
            } else {
                Ok(AudioInput::File(p))
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
    let client = match client_input {
        Some(c) => match parse_input(c) {
            Ok(i) => talksage_pipeline::ClientCapture::Explicit(i),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
        None => talksage_pipeline::ClientCapture::Off,
    };
    let is_file_input = matches!(user_input, AudioInput::File(_));

    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => std::sync::Arc::new(m),
        Err(e) => {
            eprintln!("配置加载失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let snapshot = mgr.snapshot();
    if !no_record && snapshot.recording.enabled {
        println!("录音保存目录: {}", snapshot.recording.resolve_dir(mgr.data_dir()).display());
    }

    let sessions = if save {
        let db_path = mgr.data_dir().join("sessions.db");
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
    let persist = sessions.is_some();
    let service = TalkSageService::new(mgr, sessions, EnginePool::new());
    let req = StartListen {
        user_input,
        user_engine: Some(kind),
        client,
        persist,
        record: if no_record { Some(false) } else { None },
        noise_level,
        kb_folder_override: kb_folder.map(std::path::PathBuf::from),
        user_label: None,
    };

    let sink: talksage_pipeline::EventSink = std::sync::Arc::new(move |ev: DomainEvent| {
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

    let running = match service.start(req, sink) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("启动失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("监听中…（Ctrl+C 或超时停止）");

    if seconds > 0 {
        std::thread::sleep(std::time::Duration::from_secs(seconds));
    } else if is_file_input {
        std::thread::sleep(std::time::Duration::from_secs(60));
    } else {
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    match service.finish(running) {
        Ok(Some(sid)) => println!("会话 #{sid} 已保存"),
        Ok(None) => {}
        Err(e) => eprintln!("停止失败: {e}"),
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
    let engine_dir = model_dir.join(kind.model_dir_name());
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
    let mgr = match talksage_config::ConfigManager::load(None, None) {
        Ok(m) => std::sync::Arc::new(m),
        Err(e) => {
            eprintln!("配置加载失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sessions = match talksage_session::SessionStore::open(&mgr.data_dir().join("sessions.db").to_string_lossy()) {
        Ok(s) => Some(std::sync::Arc::new(s)),
        Err(e) => {
            eprintln!("打开会话库失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let service = TalkSageService::new(mgr, sessions, EnginePool::new());
    let segments = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let segs_for_sink = segments.clone();
    let done_for_sink = done.clone();
    let sink: talksage_pipeline::EventSink = std::sync::Arc::new(move |ev| {
        match &ev {
            DomainEvent::Segment { text, is_partial: false, speaker_id, speaker_label, speaker_attribution, ts_ms, duration_ms, rms, .. } => {
                segs_for_sink.lock().unwrap().push(talksage_core::TranscriptSegment {
                    speaker_id: *speaker_id,
                    speaker_label: speaker_label.clone(),
                    speaker_attribution: speaker_attribution.clone(),
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
    let running = match service.start(
        StartListen::import_file(audio_path, kind, speaker_label.to_string()),
        sink,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("启动失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while !done.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let sid = match service.finish(running) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("保存会话失败: {e}");
            return ExitCode::FAILURE;
        }
    };

    let segs = segments.lock().unwrap();
    if segs.is_empty() {
        eprintln!("未识别到语音内容");
        return ExitCode::FAILURE;
    }
    if let Some(sid) = sid {
        println!("\n已保存会话 #{sid}（{} 段）", segs.len());
    }
    println!("转写结果（{} 段）:", segs.len());
    for s in segs.iter() {
        println!("  [{}] {}", s.speaker_label, s.text);
    }
    ExitCode::SUCCESS
}

/// 会话分析：转储原始段信息（时间戳/时长/文本）+ 疑似重复段检测。
/// 用于排查"识别内容重复"等转写质量问题。
fn cmd_session(id: i64, dup_only: bool) -> ExitCode {
    let data_dir = talksage_config::default_data_dir();
    let store = match talksage_session::SessionStore::open(&data_dir.join("sessions.db").to_string_lossy()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("打开会话库失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    let detail = match store.get_session(id) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("读取会话 #{id} 失败: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("== 会话 #{id} 原始转写段 ==");
    println!(
        "{} 段 · 开始 {} · 结束 {:?} · 质量 {:?}",
        detail.segments.len(),
        detail.started_at,
        detail.ended_at,
        detail.meta.as_ref().map(|m| m.quality_label()),
    );
    for (i, s) in detail.segments.iter().enumerate() {
        let start_ms = s.ts_ms.saturating_sub(s.duration_ms);
        println!(
            "  #{:<3} [{}] start={:>8}ms end={:>8}ms dur={:>5}ms | {}",
            i,
            s.speaker_label,
            start_ms,
            s.ts_ms,
            s.duration_ms,
            s.text,
        );
    }
    // 疑似重复段检测
    let dups = talksage_session::find_duplicate_segments(&detail.segments);
    if dups.is_empty() {
        println!("\n疑似重复段: 无（同说话人相邻段相似度均 < 0.9）");
    } else {
        println!("\n疑似重复段（同说话人、时间窗 5s 内、相似度 ≥ 0.9）:");
        for d in &dups {
            println!(
                "  #{:<3} 与 #{:<3} [{}] 相似度={:.2} 间隔={}ms",
                d.idx_a, d.idx_b, d.speaker, d.similarity, d.gap_ms
            );
            println!("    A: {}", detail.segments[d.idx_a].text);
            println!("    B: {}", detail.segments[d.idx_b].text);
        }
    }
    if !dup_only {
        println!("\n提示: 时间戳为 epoch ms；同说话人相邻段间隔小且文本相似 = VAD 把一句话切成两段重复识别。");
    }
    ExitCode::SUCCESS
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
/// 端到端实时系数（处理耗时/音频时长）与首段 final 延迟。
/// 文件输入会按真实时间喂块，因此该系数包含音频播放时间，不是纯模型计算 RTF。
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
    let engine_dir = model_dir.join(kind.model_dir_name());
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
    println!("{:<36} {:>8} {:>8} {:>10} {:>12}", "文件", "时长s", "CER/WER%", "实时系数", "首段final ms");
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
                println!("  识别: {}", tr.text.replace('\n', " "));
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
    println!("实时系数约 1 表示管道能跟随音频时钟；它包含文件实时喂入时间。准备参考文本（<同名>.txt）即可得到准确率。");
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
    print_plugin_status(&c);
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

/// 打印插件状态。
///
/// 遍历注册表而非具名字段：加插件不用改 doctor。每行的 enabled 是
/// 「插件默认值 + 用户 `[plugins.<id>]`」的合并结果，再由当前场景的
/// allowlist 对分析类插件裁决一次 —— 与 service.rs 建注册表时同一套顺序。
fn print_plugin_status(c: &talksage_config::Config) {
    let scene = c.scene.effective();
    println!(
        "plugins      : 场景={} allowlist=[{}]",
        c.scene.mode_label(),
        scene.plugin_allowlist.join(", ")
    );
    for p in talksage_plugins::builtin_plugins() {
        let id = p.id();
        let mut cfg = p.default_config();
        if let Some(user) = c.plugins.entries.get(id) {
            cfg.merge(user);
        }
        let gated = talksage_plugins::ANALYSIS_PLUGIN_IDS.contains(&id)
            && !scene.plugin_allowlist.iter().any(|a| a == id);
        let note = if gated {
            "（本场景不允许）"
        } else if !cfg.enabled() {
            "（配置已关闭）"
        } else {
            ""
        };
        println!("  {id:<21}enabled={}{note}", cfg.enabled() && !gated);
    }
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

fn resolve_models_dir() -> Option<std::path::PathBuf> {
    TalkSageService::resolve_models_dir()
}
