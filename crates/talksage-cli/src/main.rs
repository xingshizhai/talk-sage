//! TalkSage v2 launcher。
//!
//! 结构参考 DeepSeek Harness：`dsh --profile web` 之类的启动器入口。
//! 已实现：version / doctor / listen（实时转写，麦克风或文件输入）。

use std::process::ExitCode;
use clap::{Parser, Subcommand};

use talksage_asr::EngineKind;
use talksage_core::DomainEvent;
use talksage_pipeline::{AudioInput, LivePipeline, LivePipelineConfig, StreamConfig};

#[derive(Parser)]
#[command(
    name = "talksage",
    version = talksage_core::VERSION,
    about = "TalkSage v2 — 实时个人 AI 会议助理",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
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
    },
    /// 导入音频离线转写（预留）。
    Import {
        /// 音频文件路径（wav/mp3/flac/m4a/ogg）
        path: String,
    },
    /// 录制会议（预留）。
    Record,
    /// 诊断环境：配置、目录、平台。
    Doctor,
    /// 打印版本。
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!("talksage {}", talksage_core::VERSION);
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
        } => cmd_listen(&input, seconds, &engine, client.as_deref(), kb.as_deref()),
        Command::Import { path } => cmd_import(path),
        Command::Record => cmd_record(),
        Command::Doctor => cmd_doctor(),
    }
}

fn cmd_web() -> ExitCode {
    println!(
        "talk-sage {} — 桌面模式\n\
         \n\
         开发期请使用: pnpm --dir web tauri dev\n\
         构建后: 直接运行桌面应用安装包。",
        talksage_core::VERSION
    );
    ExitCode::SUCCESS
}

fn cmd_serve(host: String, port: u16) -> ExitCode {
    println!(
        "headless 服务模式（M4 预留，尚未实现）\n\
         计划: axum 服务绑定 {host}:{port}，浏览器访问 http://{host}:{port}",
    );
    ExitCode::SUCCESS
}

fn cmd_listen(input: &str, seconds: u64, engine: &str, client_input: Option<&str>, kb_folder: Option<&str>) -> ExitCode {
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
    if snapshot.plugins.term_explainer.enabled {
        plugins.push(std::sync::Arc::new(talksage_plugins::term_explainer::TermExplainerPlugin::new(
            snapshot.plugins.term_explainer.cooldown_seconds as f64,
        )));
    }
    if snapshot.plugins.translator.enabled {
        plugins.push(std::sync::Arc::new(talksage_plugins::translator::TranslatorPlugin::new()));
    }
    if snapshot.plugins.brief_retriever.enabled && kb.is_some() {
        plugins.push(std::sync::Arc::new(talksage_plugins::brief_retriever::BriefRetrieverPlugin::new(
            snapshot.plugins.brief_retriever.cooldown_seconds as f64,
            0.05,
        )));
    }
    let plugin_ctx = talksage_plugins::PluginContext { kb, llm };

    let cfg = LivePipelineConfig {
        vad_model,
        chunk_ms: 100,
        min_silence_seconds: 0.5,
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
    };

    let stop_after = seconds;
    let mut pipeline = LivePipeline::new(cfg);
    let sink: std::sync::Arc<dyn Fn(DomainEvent) + Send + Sync> = std::sync::Arc::new(|ev: DomainEvent| match &ev {
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
        other => println!("[event] {other:?}"),
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
    println!("\n已停止。");
    ExitCode::SUCCESS
}

fn cmd_import(path: String) -> ExitCode {
    println!("导入转写（M3 预留，尚未实现）: {path}");
    ExitCode::SUCCESS
}

fn cmd_record() -> ExitCode {
    println!("录制模式（M3 预留，尚未实现）");
    ExitCode::SUCCESS
}

fn cmd_doctor() -> ExitCode {
    println!("== TalkSage doctor ==");
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
