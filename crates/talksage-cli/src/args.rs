//! `talksage` 命令行参数。

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "拓思者",
    bin_name = "talksage",
    version = talksage_core::VERSION,
    about = "拓思者（TalkSage）— AI 会议助理：实时转写 · 说话人识别 · 纪要分析",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// 详细日志（等价 RUST_LOG=trace）
    #[arg(long, global = true)]
    pub verbose: bool,
    /// 日志级别（trace/debug/info/warn/error；默认读 RUST_LOG/TALKSAGE_LOG）
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,
    /// 机器可读 JSON（列表/搜索/导出/纪要等）
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
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
    /// 导入音频并保存为新会话（`transcribe --save` 的别名）。
    Import {
        /// 音频文件路径（16kHz mono wav）
        path: String,
        /// 引擎（qwen3-asr | whisper-large-v3-turbo-metal | whisper-medium-metal | …）
        #[arg(long, default_value = "qwen3-asr")]
        engine: String,
        /// 说话人标签（默认 导入）
        #[arg(long, default_value = "导入")]
        speaker: String,
    },
    /// 转写音频文件；默认只打印结果，加 `--save` 才落库。
    Transcribe {
        /// 音频文件路径
        path: String,
        /// 引擎（qwen3-asr | whisper-large-v3-turbo-metal | whisper-medium-metal | …）
        #[arg(long, default_value = "qwen3-asr")]
        engine: String,
        /// 保存为新会话
        #[arg(long)]
        save: bool,
        /// 说话人标签（仅 `--save` 时有意义）
        #[arg(long, default_value = "导入")]
        speaker: String,
    },
    /// 模型：list / download / remove / gpu。
    Models(ModelsArgs),
    /// 配置：path / get / set。
    Config(ConfigArgs),
    /// 打印最近日志（默认 200 行）。
    Logs {
        #[arg(short, long, default_value_t = 200)]
        lines: usize,
    },
    /// 列出最近的会话（`session list` 的别名）。
    Sessions {
        /// 最多显示几条（默认 20）
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// 会话：list / show / search / rename / delete / export / notes / trio / replay。
    /// 兼容旧用法：`talksage session <id>` 等同 `session show <id>`。
    Session(SessionArgs),
    /// 导出会话为 Markdown（`session export --format md` 的别名；默认写当前目录）。
    Export {
        /// 会话 id（省略则导出最近一条）
        id: Option<i64>,
        /// 输出路径（默认：当前目录 session-<id>.md）
        #[arg(short, long)]
        output: Option<String>,
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
    /// 对完整 wav 运行离线说话人分离，输出精确讲话者时间轴。
    Diarize {
        /// 输入 wav（任意采样率，自动转 16k mono）。
        path: String,
        /// 已知讲话者数量；省略时自动聚类。
        #[arg(long)]
        speakers: Option<u32>,
    },
    /// 打印版本。
    Version,
}

#[derive(Args, Debug)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub command: ModelsAction,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ModelsAction {
    /// 列出产品模型安装状态。
    List,
    /// 下载/安装引擎（qwen3-asr / whisper-*-metal / punct）。
    Download {
        engine: String,
    },
    /// 删除已安装模型目录（需 `--yes`）。
    Remove {
        engine: String,
        #[arg(long)]
        yes: bool,
    },
    /// 探测 GPU 与当前 ASR 路由。
    Gpu,
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigAction,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigAction {
    /// 打印配置文件与数据目录路径。
    Path,
    /// 读取配置（省略路径则打印全部；密钥已打码）。
    Get {
        /// 点路径，如 asr.engine_zh、llm.default
        path: Option<String>,
    },
    /// 写入配置项并保存到 talksage.toml。
    Set {
        /// 点路径，如 asr.engine_zh
        path: String,
        /// 值（true/false/数字/JSON，其余当字符串）
        value: String,
    },
}

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: Option<SessionAction>,
    /// 兼容旧用法：`talksage session <id>`
    pub id: Option<i64>,
    /// 只检测重复段（配合旧用法 `session <id> --dup-only`）
    #[arg(long)]
    pub dup_only: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum SessionAction {
    /// 列出最近会话。
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// 查看会话转写与疑似重复段。
    Show {
        id: i64,
        #[arg(long)]
        dup_only: bool,
    },
    /// 跨会话搜索转写文本。
    Search {
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// 重命名会话。
    Rename {
        id: i64,
        title: String,
    },
    /// 删除会话（数据库记录；需 `--yes`）。
    Delete {
        id: i64,
        #[arg(long)]
        yes: bool,
    },
    /// 导出会话（md / txt / audio）。
    Export {
        /// 会话 id（省略则导出最近一条）
        id: Option<i64>,
        #[arg(long, value_enum, default_value_t = ExportFormat::Md)]
        format: ExportFormat,
        #[arg(short, long)]
        output: Option<String>,
    },
    /// 按模板生成纪要并保存。
    Notes {
        id: i64,
        /// 模板 id（默认 standard_meeting）
        #[arg(long, default_value = "standard_meeting")]
        template: String,
    },
    /// 生成三段式智能纪要（概述 / 要点 / 行动项）并保存。
    Trio {
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        desc: Option<String>,
    },
    /// 用该会话录音再转写，保存为新会话。
    Replay {
        id: i64,
        /// 引擎；省略则用原会话快照中的引擎
        #[arg(long)]
        engine: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Md,
    Txt,
    Audio,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Txt => "txt",
            Self::Audio => "audio",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Txt => "txt",
            Self::Audio => "wav",
        }
    }
}

impl SessionArgs {
    /// 子命令，或旧用法 `session <id>`。
    pub fn resolve(self) -> Result<SessionAction, String> {
        match self.command {
            Some(cmd) => Ok(cmd),
            None => match self.id {
                Some(id) => Ok(SessionAction::Show {
                    id,
                    dup_only: self.dup_only,
                }),
                None => Err(
                    "请指定子命令（list / show / search / rename / delete / export / notes / trio / replay）或会话 id".into(),
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("parse")
    }

    #[test]
    fn session_search_parses() {
        let c = parse(&["talksage", "session", "search", "合同"]);
        match c.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Search { query, limit } => {
                    assert_eq!(query, "合同");
                    assert_eq!(limit, 50);
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }
    }

    #[test]
    fn session_legacy_id_is_show() {
        let c = parse(&["talksage", "session", "12", "--dup-only"]);
        match c.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Show { id, dup_only } => {
                    assert_eq!(id, 12);
                    assert!(dup_only);
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }
    }

    #[test]
    fn session_list_and_global_json() {
        let c = parse(&["talksage", "--json", "session", "list", "--limit", "5"]);
        assert!(c.json);
        match c.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::List { limit } => assert_eq!(limit, 5),
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }
    }

    #[test]
    fn session_export_audio_parses() {
        let c = parse(&[
            "talksage",
            "session",
            "export",
            "3",
            "--format",
            "audio",
            "-o",
            "/tmp/a.wav",
        ]);
        match c.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Export { id, format, output } => {
                    assert_eq!(id, Some(3));
                    assert_eq!(format, ExportFormat::Audio);
                    assert_eq!(output.as_deref(), Some("/tmp/a.wav"));
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }
    }

    #[test]
    fn session_notes_trio_rename_delete_parse() {
        let notes = parse(&["talksage", "session", "notes", "8"]);
        match notes.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Notes { id, template } => {
                    assert_eq!(id, 8);
                    assert_eq!(template, "standard_meeting");
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }

        let trio = parse(&["talksage", "session", "trio", "8", "--name", "周会"]);
        match trio.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Trio { id, name, .. } => {
                    assert_eq!(id, 8);
                    assert_eq!(name.as_deref(), Some("周会"));
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }

        let rename = parse(&["talksage", "session", "rename", "8", "客户拜访"]);
        match rename.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Rename { id, title } => {
                    assert_eq!(id, 8);
                    assert_eq!(title, "客户拜访");
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }

        let del = parse(&["talksage", "session", "delete", "8", "--yes"]);
        match del.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Delete { id, yes } => {
                    assert_eq!(id, 8);
                    assert!(yes);
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }
    }

    #[test]
    fn sessions_alias_still_parses() {
        let c = parse(&["talksage", "sessions", "--limit", "3"]);
        match c.command {
            Command::Sessions { limit } => assert_eq!(limit, 3),
            _ => panic!("expected sessions"),
        }
    }

    #[test]
    fn models_list_download_gpu_parse() {
        let list = parse(&["talksage", "models", "list"]);
        match list.command {
            Command::Models(args) => assert!(matches!(args.command, ModelsAction::List)),
            _ => panic!("expected models"),
        }

        let dl = parse(&["talksage", "models", "download", "qwen3-asr"]);
        match dl.command {
            Command::Models(args) => match args.command {
                ModelsAction::Download { engine } => assert_eq!(engine, "qwen3-asr"),
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected models"),
        }

        let rm = parse(&["talksage", "models", "remove", "punct", "--yes"]);
        match rm.command {
            Command::Models(args) => match args.command {
                ModelsAction::Remove { engine, yes } => {
                    assert_eq!(engine, "punct");
                    assert!(yes);
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected models"),
        }

        let gpu = parse(&["talksage", "--json", "models", "gpu"]);
        assert!(gpu.json);
        match gpu.command {
            Command::Models(args) => assert!(matches!(args.command, ModelsAction::Gpu)),
            _ => panic!("expected models"),
        }
    }

    #[test]
    fn transcribe_and_import_parse() {
        let t = parse(&["talksage", "transcribe", "a.wav", "--engine", "qwen3-asr"]);
        match t.command {
            Command::Transcribe { path, engine, save, speaker } => {
                assert_eq!(path, "a.wav");
                assert_eq!(engine, "qwen3-asr");
                assert!(!save);
                assert_eq!(speaker, "导入");
            }
            _ => panic!("expected transcribe"),
        }

        let saved = parse(&["talksage", "transcribe", "a.wav", "--save"]);
        match saved.command {
            Command::Transcribe { save, .. } => assert!(save),
            _ => panic!("expected transcribe"),
        }

        let imp = parse(&["talksage", "import", "a.wav"]);
        match imp.command {
            Command::Import { path, engine, .. } => {
                assert_eq!(path, "a.wav");
                assert_eq!(engine, "qwen3-asr");
            }
            _ => panic!("expected import"),
        }
    }

    #[test]
    fn config_path_get_set_parse() {
        let path = parse(&["talksage", "config", "path"]);
        match path.command {
            Command::Config(args) => assert!(matches!(args.command, ConfigAction::Path)),
            _ => panic!("expected config"),
        }

        let all = parse(&["talksage", "config", "get"]);
        match all.command {
            Command::Config(args) => match args.command {
                ConfigAction::Get { path } => assert!(path.is_none()),
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected config"),
        }

        let get = parse(&["talksage", "config", "get", "asr.engine_zh"]);
        match get.command {
            Command::Config(args) => match args.command {
                ConfigAction::Get { path } => assert_eq!(path.as_deref(), Some("asr.engine_zh")),
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected config"),
        }

        let set = parse(&["talksage", "config", "set", "asr.engine_zh", "qwen3-asr"]);
        match set.command {
            Command::Config(args) => match args.command {
                ConfigAction::Set { path, value } => {
                    assert_eq!(path, "asr.engine_zh");
                    assert_eq!(value, "qwen3-asr");
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected config"),
        }
    }

    #[test]
    fn logs_and_session_replay_parse() {
        let logs = parse(&["talksage", "logs", "--lines", "20"]);
        match logs.command {
            Command::Logs { lines } => assert_eq!(lines, 20),
            _ => panic!("expected logs"),
        }

        let replay = parse(&["talksage", "session", "replay", "8", "--engine", "qwen3-asr"]);
        match replay.command {
            Command::Session(args) => match args.resolve().unwrap() {
                SessionAction::Replay { id, engine } => {
                    assert_eq!(id, 8);
                    assert_eq!(engine.as_deref(), Some("qwen3-asr"));
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!("expected session"),
        }
    }
}
