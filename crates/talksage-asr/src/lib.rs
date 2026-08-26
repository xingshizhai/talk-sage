//! TalkSage v2 流式 ASR 引擎。
//!
//! M1 PoC：基于 sherpa-onnx（Rust 绑定）的流式识别封装。
//! 设计目标：`StreamingASREngine` trait 与传输/管道无关，
//! 双引擎（英文 zipformer / 中文 paraformer）由配置选择。
//!
//! 引擎池（`EnginePool`）：参考 WhisperLiveKit 的"引擎单例"思想——
//! 模型只加载一次，监听会话间复用（热启动），避免每次监听重复加载模型。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use sherpa_onnx::{
    OfflineModelConfig, OfflineQwen3ASRModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineStream, OfflineWhisperModelConfig, OnlineModelConfig, OnlineParaformerModelConfig,
    OnlineRecognizer, OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig,
};

/// 阿里云 NLS 引擎与 Token 管理。
pub mod aliyun;

/// GPU 后端检测（CUDA / CoreML / CPU）。
pub mod gpu;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[cfg(any(all(target_os = "macos", target_arch = "aarch64"), all(target_os = "windows", target_arch = "x86_64", feature = "vulkan-gpu")))]
mod metal;
pub use gpu::GpuBackend;

/// Hardware/cloud execution routing.
pub mod routing;
pub use routing::{resolve_asr_route, AsrRoute, CloudCredentials};

/// 模型管理（下载 / 删除 / 磁盘占用；应用内「转写引擎」页使用）。
pub mod models;

/// Punctuation restoration using sherpa-onnx CT-Transformer model.
pub mod punct;
pub use punct::{PunctuationRestorer, is_punct_model_available};
pub use models::{download_punct_model, remove_punct_model, is_punct_model_installed, punct_download_size_mb};

/// 段级识别引擎接口（统一流式与离线段级模型）。
///
/// - 流式（paraformer/zipformer）：`accept` 逐块返回增量文本，`finish` 返回最终文本。
/// - 离线段级（whisper/qwen3）：`accept` 只累积音频（返回 None，无增量），
///   `finish` 对整段做一次识别并返回最终文本。
pub trait SegmentEngine: Send {
    /// 推送一段 16kHz mono f32 音频，返回增量文本（离线引擎返回 None）。
    fn accept(&mut self, samples: &[f32]) -> Option<String>;
    /// 段结束：返回该段最终识别文本。
    fn finish(&mut self) -> String;
    /// 清空当前段状态（新一句开始）。
    fn reset(&mut self);
    /// 引擎类型标识。
    fn kind(&self) -> EngineKind;
}

/// 流式识别引擎接口：喂音频块 → 增量出字。
pub trait StreamingASREngine {
    /// 推送一段 16kHz mono f32 音频，返回本轮增量识别文本（可为空）。
    fn accept(&mut self, samples: &[f32]) -> Option<String>;
    /// 清空当前识别会话（新一句开始）。
    fn reset(&mut self);
    /// 标记输入结束，刷新尾部上下文。
    fn finish(&mut self);
    /// 引擎类型标识。
    fn kind(&self) -> EngineKind;
}

/// 单次识别会话的上下文选项。参与引擎池键，避免不同会议复用错误热词上下文。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineOptions {
    pub hotwords: Vec<String>,
    pub hotword_score: f32,
    /// sherpa-onnx provider: "cpu" | "cuda" | "coreml". Empty string defaults to "cpu".
    pub provider: String,
}

impl EngineOptions {
    fn signature(&self) -> String {
        let provider = if self.provider.is_empty() { "cpu" } else { &self.provider };
        format!("{:.3}|{}|{}", self.hotword_score, self.hotwords.join("\u{1f}"), provider)
    }
}

static HOTWORD_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// sherpa 在线 transducer 的 `hotwords_file` 接受普通短语（每行一个），
/// 识别器创建时内部按 bpe.model 编译。文件只需存活到 create 完成。
fn write_hotwords_file(hotwords: &[String]) -> anyhow::Result<std::path::PathBuf> {
    let seq = HOTWORD_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("talksage-hotwords-{}-{seq}.txt", std::process::id()));
    let body = hotwords.iter()
        .map(|term| term.replace(['\r', '\n'], " ").trim().to_string())
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{body}\n"))?;
    Ok(path)
}

/// 引擎后端选择（与配置 asr.client_engine / user_engine 对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    /// 英文 streaming zipformer（transducer，流式增量）。
    ZipformerEn,
    /// 中文 streaming paraformer（int8；模型目录含 fp32 时自动用 fp32 更准，流式增量）。
    ParaformerZh,
    /// OpenAI Whisper base（离线，段级识别：段结束后出结果；多语言更准）。
    WhisperBase,
    /// OpenAI Whisper small（离线，段级；更准但更慢）。
    WhisperSmall,
    /// Qwen3-ASR 0.6B（离线，段级；中英等多语言，需模型仓库开放后下载）。
    Qwen3Asr,
    /// whisper.cpp large-v3-turbo Q5_0（Apple Silicon Metal 路线）。
    WhisperLargeV3TurboMetal,
    /// 阿里云实时语音识别（云端流式，需配置 AccessKey）。
    AliyunCloud,
}

/// 单一事实来源的模型能力描述，供配置界面和服务 API 共用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelProfile {
    pub kind: EngineKind,
    pub label: &'static str,
    pub languages: &'static str,
    pub streaming: bool,
    /// `realtime` / `balanced` / `accurate`。
    pub speed: &'static str,
    pub description: &'static str,
    /// 是否已经有可运行的引擎适配器；false 时只允许预下载模型。
    pub selectable: bool,
}

impl EngineKind {
    /// 产品模型目录。旧流式/旧 ONNX Whisper 仍可被解析用于测试，但不再暴露给用户。
    pub const ALL: [Self; 2] = [Self::Qwen3Asr, Self::WhisperLargeV3TurboMetal];

    pub fn is_product_model(self) -> bool {
        Self::ALL.contains(&self)
    }

    /// 从配置字符串解析（zipformer-en / paraformer-zh / whisper-base / whisper-small / qwen3-asr / …）。
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "zipformer-en" => Some(Self::ZipformerEn),
            "paraformer-zh" => Some(Self::ParaformerZh),
            "whisper" | "whisper-base" => Some(Self::WhisperBase),
            "whisper-small" => Some(Self::WhisperSmall),
            "qwen3-asr" | "qwen3" => Some(Self::Qwen3Asr),
            "whisper-large-v3-turbo-metal" | "large-v3-turbo-metal" => Some(Self::WhisperLargeV3TurboMetal),
            "aliyun" | "aliyun-cloud" => Some(Self::AliyunCloud),
            _ => None,
        }
    }

    /// 人类可读名。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ZipformerEn => "zipformer-en",
            Self::ParaformerZh => "paraformer-zh",
            Self::WhisperBase => "whisper-base",
            Self::WhisperSmall => "whisper-small",
            Self::Qwen3Asr => "qwen3-asr",
            Self::WhisperLargeV3TurboMetal => "whisper-large-v3-turbo-metal",
            Self::AliyunCloud => "aliyun-cloud",
        }
    }

    /// 是否流式（逐块增量出 partial）。false = 离线段级（VAD 段结束后整段识别）。
    pub fn is_streaming(self) -> bool {
        matches!(self, Self::ParaformerZh | Self::ZipformerEn | Self::AliyunCloud)
    }

    /// 模型目录名（下载脚本约定，与 `scripts/download_models.py` 输出一致）。
    pub fn model_dir_name(self) -> &'static str {
        match self {
            Self::ParaformerZh => "sherpa-onnx-streaming-paraformer-zh",
            Self::ZipformerEn => "sherpa-onnx-streaming-zipformer-en-2023-06-26",
            Self::WhisperBase => "sherpa-onnx-whisper-base",
            Self::WhisperSmall => "sherpa-onnx-whisper-small",
            Self::Qwen3Asr => "sherpa-onnx-qwen3-asr-0.6b",
            Self::WhisperLargeV3TurboMetal => "whisper.cpp-large-v3-turbo-q5_0",
            Self::AliyunCloud => "aliyun-cloud",
        }
    }

    pub fn profile(self) -> ModelProfile {
        match self {
            Self::ParaformerZh => ModelProfile { kind: self, label: "Paraformer 中文（旧诊断模型）", languages: "zh", streaming: true, speed: "realtime", description: "仅保留给自动化测试，不再作为产品模型", selectable: false },
            Self::ZipformerEn => ModelProfile { kind: self, label: "Zipformer 英文（旧诊断模型）", languages: "en", streaming: true, speed: "realtime", description: "仅保留给自动化测试，不再作为产品模型", selectable: false },
            Self::WhisperBase => ModelProfile { kind: self, label: "Whisper base ONNX（旧模型）", languages: "multilingual", streaming: false, speed: "balanced", description: "旧 sherpa ONNX 模型，不再提供下载", selectable: false },
            Self::WhisperSmall => ModelProfile { kind: self, label: "Whisper small ONNX（旧模型）", languages: "multilingual", streaming: false, speed: "accurate", description: "旧 sherpa ONNX 模型，不再提供下载", selectable: false },
            Self::Qwen3Asr => ModelProfile { kind: self, label: "Qwen3-ASR 0.6B int8", languages: "multilingual", streaming: false, speed: "accurate", description: "CUDA/CPU 本地高精度模型；中文与专业术语优先", selectable: true },
            Self::WhisperLargeV3TurboMetal => ModelProfile { kind: self, label: "Whisper large-v3-turbo Q5_0（whisper.cpp GPU）", languages: "multilingual", streaming: false, speed: "balanced", description: "whisper.cpp GPU 段级识别（macOS Metal / Windows Vulkan），约 547 MiB；中文/中英混说鲁棒性好", selectable: cfg!(any(all(target_os = "macos", target_arch = "aarch64"), all(target_os = "windows", target_arch = "x86_64", feature = "vulkan-gpu"))) },
            Self::AliyunCloud => ModelProfile { kind: self, label: "阿里云实时语音", languages: "zh,en", streaming: true, speed: "realtime", description: "云端流式识别，需配置 AccessKey；无本地 GPU 时自动启用", selectable: true },
        }
    }

    /// 检查模型文件是否完整（存在且非空），而不只是检查目录存在。
    ///
    /// 非空校验防止「下载/解压失败留下 0 字节文件」被误判为已安装——
    /// 空 onnx 会让 sherpa-onnx 在 native 层崩溃而非优雅报错。
    pub fn is_available(self, models_root: &Path) -> bool {
        let dir = models_root.join(self.model_dir_name());
        let has = |name: &str| dir.join(name).is_file() && dir.join(name).metadata().map(|m| m.len() > 0).unwrap_or(false);
        let has_large = |name: &str| dir.join(name).metadata().map(|m| m.len() >= 100 * 1024 * 1024).unwrap_or(false);
        let has_tokenizer_dir = || {
            dir.join("tokenizer").is_dir()
                && dir.join("tokenizer").join("vocab.json").is_file()
                && dir.join("tokenizer").join("vocab.json").metadata().map(|m| m.len() > 0).unwrap_or(false)
        };
        match self {
            Self::ParaformerZh => has("tokens.txt") && ((has("encoder.onnx") && has("decoder.onnx")) || (has("encoder.int8.onnx") && has("decoder.int8.onnx"))),
            Self::ZipformerEn => has("tokens.txt") && has("bpe.model") && has("encoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx") && has("decoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx") && has("joiner-epoch-99-avg-1-chunk-16-left-64.int8.onnx"),
            Self::WhisperBase | Self::WhisperSmall => {
                let stem = if self == Self::WhisperBase { "base" } else { "small" };
                has(&format!("{stem}-tokens.txt")) && ((has(&format!("{stem}-encoder.onnx")) && has(&format!("{stem}-decoder.onnx"))) || (has(&format!("{stem}-encoder.int8.onnx")) && has(&format!("{stem}-decoder.int8.onnx"))))
            }
            Self::Qwen3Asr => {
                // 官方包为 int8 布局（encoder.int8.onnx / decoder.int8.onnx / tokenizer/ 目录）；
                // 兼容早期约定的 fp32 布局（encoder.onnx / decoder.onnx / tokenizer.json）。
                has("conv_frontend.onnx")
                    && ((has_large("encoder.onnx") && has_large("decoder.onnx"))
                        || (has_large("encoder.int8.onnx") && has_large("decoder.int8.onnx")))
                    && (has("tokenizer.json") || has_tokenizer_dir())
            }
            Self::WhisperLargeV3TurboMetal => {
                let model = dir.join("ggml-large-v3-turbo-q5_0.bin");
                let size = model.metadata().ok().map(|m| m.len());
                has("ggml-large-v3-turbo-q5_0.bin")
                    && std::fs::read_to_string(dir.join("ggml-large-v3-turbo-q5_0.sha1"))
                        .ok()
                        .and_then(|value| {
                            let mut fields = value.split_whitespace();
                            Some((fields.next()?.to_string(), fields.next()?.parse::<u64>().ok()?))
                        })
                        .is_some_and(|(hash, verified_size)| {
                            hash == "e050f7970618a659205450ad97eb95a18d69c9ee"
                                && size == Some(verified_size)
                        })
            }
            Self::AliyunCloud => true, // 云端引擎：无本地模型文件，始终可用
        }
    }
}

/// sherpa-onnx 流式识别器封装。
pub struct SherpaStreamingEngine {
    recognizer: OnlineRecognizer,
    stream: OnlineStream,
    sample_rate: i32,
    kind: EngineKind,
}

/// 在线模型需要右侧声学上下文才能确认句尾 token。直接调用
/// `input_finished()` 虽会排空已经就绪的帧，却不会替缺失的右上下文补帧，
/// Paraformer 因此容易吞掉最后一个中文字符。这里补的是模型输入，不是实际
/// 录音，所以不会延长会话时间戳，也不会让用户额外等待。取 4 个解码步长
/// （实测 Paraformer 流式解码延迟约 600ms/步），覆盖 VAD 切段落在任意
/// chunk 边界、以及尾字尚未解码完的情况。
const STREAMING_TAIL_PADDING_MS: usize = 2400;

fn streaming_tail_padding_samples(sample_rate: i32) -> usize {
    sample_rate.max(0) as usize * STREAMING_TAIL_PADDING_MS / 1000
}

impl SherpaStreamingEngine {
    /// 构建指定引擎（模型文件在 model_dir 下，文件名与下载脚本约定一致）。
    pub fn new(kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<Self> {
        Self::new_with_options(kind, model_dir, num_threads, &EngineOptions::default())
    }

    pub fn new_with_options(kind: EngineKind, model_dir: &Path, num_threads: i32, options: &EngineOptions) -> anyhow::Result<Self> {
        let model_config = match kind {
            EngineKind::ParaformerZh => {
                // fp32 更准（存在时优先）；int8 作为后备（小/快）
                let (enc, dec) = if model_dir.join("encoder.onnx").is_file() && model_dir.join("decoder.onnx").is_file() {
                    ("encoder.onnx", "decoder.onnx")
                } else {
                    ("encoder.int8.onnx", "decoder.int8.onnx")
                };
                OnlineModelConfig {
                    model_type: Some("paraformer".into()),
                    paraformer: OnlineParaformerModelConfig {
                        encoder: Some(model_dir.join(enc).to_string_lossy().into()),
                        decoder: Some(model_dir.join(dec).to_string_lossy().into()),
                    },
                    tokens: Some(model_dir.join("tokens.txt").to_string_lossy().into()),
                    num_threads,
                    provider: Some(if options.provider.is_empty() { "cpu".into() } else { options.provider.clone() }),
                    ..Default::default()
                }
            }
            EngineKind::ZipformerEn => {
                let enc = "encoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx";
                let dec = "decoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx";
                let joiner = "joiner-epoch-99-avg-1-chunk-16-left-64.int8.onnx";
                let bpe_vocab = model_dir.join("bpe.vocab");
                OnlineModelConfig {
                    model_type: Some("zipformer2".into()),
                    modeling_unit: bpe_vocab.is_file().then(|| "bpe".into()),
                    bpe_vocab: bpe_vocab.is_file().then(|| bpe_vocab.to_string_lossy().into_owned()),
                    transducer: OnlineTransducerModelConfig {
                        encoder: Some(model_dir.join(enc).to_string_lossy().into()),
                        decoder: Some(model_dir.join(dec).to_string_lossy().into()),
                        joiner: Some(model_dir.join(joiner).to_string_lossy().into()),
                    },
                    tokens: Some(model_dir.join("tokens.txt").to_string_lossy().into()),
                    num_threads,
                    provider: Some(if options.provider.is_empty() { "cpu".into() } else { options.provider.clone() }),
                    ..Default::default()
                }
            }
            EngineKind::WhisperBase | EngineKind::WhisperSmall | EngineKind::Qwen3Asr | EngineKind::WhisperLargeV3TurboMetal => {
                anyhow::bail!("{} 是离线段级引擎，请用 OfflineSegmentEngine::new", kind.display_name())
            }
            EngineKind::AliyunCloud => {
                anyhow::bail!("AliyunCloud 是云端引擎，请直接构造 AliyunEngine")
            }
        };

        let use_hotwords = kind == EngineKind::ZipformerEn
            && !options.hotwords.is_empty()
            && model_dir.join("bpe.vocab").is_file();
        if kind == EngineKind::ZipformerEn && !options.hotwords.is_empty() && !use_hotwords {
            log::warn!("Zipformer 缺少 bpe.vocab，热词未启用；运行 scripts/download_models.py zipformer-en 补齐");
        }
        let hotwords_path = if use_hotwords { Some(write_hotwords_file(&options.hotwords)?) } else { None };
        let config = OnlineRecognizerConfig {
            model_config,
            decoding_method: Some(if use_hotwords { "modified_beam_search" } else { "greedy_search" }.into()),
            max_active_paths: if use_hotwords { 4 } else { 0 },
            hotwords_file: hotwords_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            hotwords_score: if use_hotwords { options.hotword_score } else { 0.0 },
            enable_endpoint: false,
            ..Default::default()
        };

        let recognizer = OnlineRecognizer::create(&config);
        if let Some(path) = hotwords_path { std::fs::remove_file(path).ok(); }
        let recognizer = match recognizer {
            Some(recognizer) => recognizer,
            None if use_hotwords => {
                log::warn!("Zipformer 热词识别器创建失败，回退 greedy_search");
                let mut fallback = config.clone();
                fallback.decoding_method = Some("greedy_search".into());
                fallback.max_active_paths = 0;
                fallback.hotwords_file = None;
                fallback.hotwords_score = 0.0;
                OnlineRecognizer::create(&fallback)
                    .ok_or_else(|| anyhow::anyhow!("创建 sherpa-onnx 流式识别器失败（模型路径/文件不完整？）"))?
            }
            None => return Err(anyhow::anyhow!("创建 sherpa-onnx 流式识别器失败（模型路径/文件不完整？）")),
        };
        let stream = recognizer.create_stream();
        Ok(Self {
            recognizer,
            stream,
            sample_rate: 16000,
            kind,
        })
    }
}

impl StreamingASREngine for SherpaStreamingEngine {
    fn accept(&mut self, samples: &[f32]) -> Option<String> {
        self.stream.accept_waveform(self.sample_rate, samples);
        // sherpa-onnx 流式解码是帧级推进：缓冲中每就绪一帧就解码一次
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
        self.recognizer.get_result(&self.stream).map(|r| r.text)
    }

    fn reset(&mut self) {
        self.recognizer.reset(&self.stream);
    }

    fn finish(&mut self) {
        // 补足右上下文：分段补帧并反复解码，直到引擎不再 ready。
        // 一次性补完再 decode 的问题：若补帧后仍差几帧才满一个 chunk，
        // is_ready 为 false 会提前退出循环，尾字留在缓冲里。分段补帧 +
        // 每轮都检查，确保最后一个 chunk 被完整消费。
        let padding = vec![0.0; streaming_tail_padding_samples(self.sample_rate)];
        // 分 4 段喂入，每段后都尝试解码（每段约 600ms = 一个解码步长）
        let step = padding.len() / 4;
        for slice in padding.chunks(step.max(1)) {
            self.stream.accept_waveform(self.sample_rate, slice);
            while self.recognizer.is_ready(&self.stream) {
                self.recognizer.decode(&self.stream);
            }
        }
        self.stream.input_finished();
        while self.recognizer.is_ready(&self.stream) {
            self.recognizer.decode(&self.stream);
        }
    }

    fn kind(&self) -> EngineKind {
        self.kind
    }
}

impl SegmentEngine for SherpaStreamingEngine {
    fn accept(&mut self, samples: &[f32]) -> Option<String> {
        <Self as StreamingASREngine>::accept(self, samples)
    }

    fn finish(&mut self) -> String {
        <Self as StreamingASREngine>::finish(self);
        self.recognizer.get_result(&self.stream).map(|r| r.text).unwrap_or_default()
    }

    fn reset(&mut self) {
        <Self as StreamingASREngine>::reset(self);
    }

    fn kind(&self) -> EngineKind {
        self.kind
    }
}

/// 离线段级引擎：VAD 段内累积音频，段结束时对整段做一次离线识别
/// （Whisper / Qwen3-ASR，比流式更准；无 partial 增量）。
pub struct OfflineSegmentEngine {
    recognizer: OfflineRecognizer,
    stream: OfflineStream,
    buffer: Vec<f32>,
    kind: EngineKind,
    sample_rate: i32,
}

impl OfflineSegmentEngine {
    /// 构建离线引擎（whisper-base / whisper-small / qwen3-asr）。
    pub fn new(kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<Self> {
        Self::new_with_options(kind, model_dir, num_threads, &EngineOptions::default())
    }

    pub fn new_with_options(kind: EngineKind, model_dir: &Path, num_threads: i32, options: &EngineOptions) -> anyhow::Result<Self> {
        let model_config = match kind {
            EngineKind::WhisperBase | EngineKind::WhisperSmall => {
                let stem = if kind == EngineKind::WhisperBase { "base" } else { "small" };
                // fp32 更准（存在时优先），否则 int8
                let (enc, dec) = if model_dir.join(format!("{stem}-encoder.onnx")).is_file() {
                    (format!("{stem}-encoder.onnx"), format!("{stem}-decoder.onnx"))
                } else {
                    (format!("{stem}-encoder.int8.onnx"), format!("{stem}-decoder.int8.onnx"))
                };
                OfflineModelConfig {
                    whisper: OfflineWhisperModelConfig {
                        encoder: Some(model_dir.join(enc).to_string_lossy().into()),
                        decoder: Some(model_dir.join(dec).to_string_lossy().into()),
                        language: None, // 自动检测
                        task: Some("transcribe".into()),
                        tail_paddings: 1000,
                        enable_token_timestamps: false,
                        enable_segment_timestamps: false,
                    },
                    tokens: Some(model_dir.join(format!("{stem}-tokens.txt")).to_string_lossy().into()),
                    num_threads,
                    debug: false,
                    provider: Some(if options.provider.is_empty() { "cpu".into() } else { options.provider.clone() }),
                    model_type: Some("whisper".into()),
                    ..Default::default()
                }
            }
            EngineKind::Qwen3Asr => {
                // 官方 int8 布局优先；兼容早期 fp32 约定。
                let (enc, dec) = if model_dir.join("encoder.onnx").is_file() {
                    ("encoder.onnx", "decoder.onnx")
                } else {
                    ("encoder.int8.onnx", "decoder.int8.onnx")
                };
                // 空/极小模型文件会让 sherpa-onnx 在 native 层崩溃（而非优雅报错）。
                // 加载前做最小大小校验（官方 int8 包 >100MB），截断/空文件直接拒绝。
                let min_size = |name: &str| -> anyhow::Result<()> {
                    let p = model_dir.join(name);
                    let len = std::fs::metadata(&p).map_err(|e| anyhow::anyhow!("模型文件不可读 {name}: {e}"))?.len();
                    if len < 100 * 1024 * 1024 {
                        anyhow::bail!(
                            "Qwen3-ASR 模型文件异常（{name} 仅 {:.1}MB，预期 >100MB）。\
                             下载可能未完成或已损坏，请删除后重新下载（设置 → ASR 转写 → 模型管理）",
                            len as f64 / 1e6
                        );
                    }
                    Ok(())
                };
                min_size(enc)?;
                min_size(dec)?;
                // sherpa-onnx 的 QwenAsrTokenizer 期望 tokenizer **目录**
                // （内含 vocab.json / merges.txt / tokenizer_config.json）；旧约定为单文件。
                let tokenizer = if model_dir.join("tokenizer").is_dir() {
                    model_dir.join("tokenizer")
                } else {
                    model_dir.join("tokenizer.json")
                };
                OfflineModelConfig {
                    qwen3_asr: OfflineQwen3ASRModelConfig {
                        conv_frontend: Some(model_dir.join("conv_frontend.onnx").to_string_lossy().into()),
                        encoder: Some(model_dir.join(enc).to_string_lossy().into()),
                        decoder: Some(model_dir.join(dec).to_string_lossy().into()),
                        tokenizer: Some(tokenizer.to_string_lossy().into()),
                        max_total_len: 512,
                        max_new_tokens: 256,
                        temperature: 0.0,
                        top_p: 1.0,
                        seed: 42,
                        hotwords: (!options.hotwords.is_empty()).then(|| options.hotwords.join(", ")),
                    },
                    num_threads,
                    debug: false,
                    provider: Some(if options.provider.is_empty() { "cpu".into() } else { options.provider.clone() }),
                    model_type: Some("qwen3_asr".into()),
                    ..Default::default()
                }
            }
            EngineKind::WhisperLargeV3TurboMetal => anyhow::bail!("whisper.cpp Metal 使用独立引擎工厂"),
            _ => anyhow::bail!("{} 不是离线引擎", kind.display_name()),
        };
        let config = OfflineRecognizerConfig {
            model_config,
            decoding_method: Some("greedy_search".into()),
            ..Default::default()
        };
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| anyhow::anyhow!("创建 sherpa-onnx 离线识别器失败（模型路径/文件不完整？）"))?;
        let stream = recognizer.create_stream();
        Ok(Self {
            recognizer,
            stream,
            buffer: Vec::new(),
            kind,
            sample_rate: 16000,
        })
    }
}

impl SegmentEngine for OfflineSegmentEngine {
    fn accept(&mut self, samples: &[f32]) -> Option<String> {
        self.buffer.extend_from_slice(samples);
        None // 离线：无增量，段结束才出结果
    }

    fn finish(&mut self) -> String {
        let samples = std::mem::take(&mut self.buffer);
        self.stream.accept_waveform(self.sample_rate, &samples);
        self.recognizer.decode(&self.stream);
        self.stream.get_result().map(|r| r.text).unwrap_or_default()
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.stream = self.recognizer.create_stream();
    }

    fn kind(&self) -> EngineKind {
        self.kind
    }
}

/// 按引擎类型创建段级引擎：流式走 sherpa-onnx 流式识别器；离线（whisper/qwen3）走离线段级。
pub fn create_engine(kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<Box<dyn SegmentEngine>> {
    create_engine_with_options(kind, model_dir, num_threads, &EngineOptions::default())
}

pub fn create_engine_with_options(kind: EngineKind, model_dir: &Path, num_threads: i32, options: &EngineOptions) -> anyhow::Result<Box<dyn SegmentEngine>> {
    if kind == EngineKind::WhisperLargeV3TurboMetal {
        // whisper.cpp GPU 适配器：macOS → Metal，Windows → Vulkan。
        // 两个平台都由 whisper-rs 按编译 feature 选择后端（use_gpu(true)）。
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            return Ok(Box::new(metal::WhisperMetalEngine::new(model_dir, num_threads, options)?));
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64", feature = "vulkan-gpu"))]
        {
            return Ok(Box::new(metal::WhisperMetalEngine::new(model_dir, num_threads, options)?));
        }
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64", feature = "vulkan-gpu"),
        )))]
        {
            anyhow::bail!(
                "whisper.cpp GPU 引擎不可用：macOS 需 Apple Silicon；Windows 需以 vulkan-gpu feature 构建（需 VULKAN_SDK）"
            );
        }
    }
    if kind.is_streaming() {
        Ok(Box::new(SherpaStreamingEngine::new_with_options(kind, model_dir, num_threads, options)?))
    } else {
        Ok(Box::new(OfflineSegmentEngine::new_with_options(kind, model_dir, num_threads, options)?))
    }
}

/// Create an engine with automatic provider selection based on detected GPU.
pub fn create_engine_auto(
    kind: EngineKind,
    model_dir: &Path,
    num_threads: i32,
    gpu: GpuBackend,
    options: &EngineOptions,
) -> anyhow::Result<Box<dyn SegmentEngine>> {
    let provider = gpu.provider_str().to_string();
    let opts = EngineOptions { provider, ..options.clone() };
    create_engine_with_options(kind, model_dir, num_threads, &opts)
}

#[cfg(test)]
mod tests {
    #[test]
    fn streaming_finish_adds_right_context_without_changing_audio_clock() {
        // 2400ms 尾补帧（4 个 ~600ms 解码步长，覆盖 VAD 切段落在任意 chunk 边界）
        assert_eq!(super::streaming_tail_padding_samples(16_000), 38_400);
        assert_eq!(super::streaming_tail_padding_samples(8_000), 19_200);
    }

    use super::*;

    #[test]
    fn engine_kind_parsing() {
        assert_eq!(EngineKind::from_name("zipformer-en"), Some(EngineKind::ZipformerEn));
        assert_eq!(EngineKind::from_name("paraformer-zh"), Some(EngineKind::ParaformerZh));
        assert_eq!(EngineKind::from_name("whisper"), Some(EngineKind::WhisperBase));
        assert_eq!(EngineKind::from_name("whisper-base"), Some(EngineKind::WhisperBase));
        assert_eq!(EngineKind::from_name("whisper-small"), Some(EngineKind::WhisperSmall));
        assert_eq!(EngineKind::from_name("qwen3-asr"), Some(EngineKind::Qwen3Asr));
        assert_eq!(EngineKind::from_name("whisper-large-v3-turbo-metal"), Some(EngineKind::WhisperLargeV3TurboMetal));
        assert_eq!(EngineKind::from_name("unknown"), None);
        // 流式 vs 离线段级
        assert!(EngineKind::ParaformerZh.is_streaming());
        assert!(!EngineKind::WhisperBase.is_streaming());
        assert!(!EngineKind::Qwen3Asr.is_streaming());
        assert!(!EngineKind::WhisperLargeV3TurboMetal.is_streaming());
    }

    #[test]
    fn model_catalog_has_stable_unique_ids_and_speed_classes() {
        let mut ids = std::collections::HashSet::new();
        for kind in EngineKind::ALL {
            let profile = kind.profile();
            assert_eq!(profile.kind, kind);
            assert_eq!(profile.streaming, kind.is_streaming());
            assert!(ids.insert(kind.display_name()), "模型 id 重复");
            assert!(matches!(profile.speed, "realtime" | "balanced" | "accurate"));
        }
        assert_eq!(EngineKind::ALL, [EngineKind::Qwen3Asr, EngineKind::WhisperLargeV3TurboMetal]);
        assert!(EngineKind::Qwen3Asr.profile().selectable);
        assert_eq!(EngineKind::WhisperLargeV3TurboMetal.profile().selectable, cfg!(all(target_os = "macos", target_arch = "aarch64")));
        assert!(!EngineKind::ParaformerZh.is_product_model());
    }

    #[test]
    fn engine_options_has_provider_field() {
        let opts = EngineOptions {
            provider: "cpu".into(),
            ..Default::default()
        };
        assert_eq!(opts.provider, "cpu");
    }

    #[test]
    fn create_engine_auto_exists() {
        // Just verify it compiles — no model needed
        use std::path::Path;
        let _ = std::panic::catch_unwind(|| {
            let _ = crate::create_engine_auto(
                crate::EngineKind::WhisperBase,
                Path::new("/nonexistent"),
                1,
                crate::GpuBackend::None,
                &crate::EngineOptions::default(),
            );
        });
    }

    /// 0 字节模型文件不得被判定为已安装（空 onnx 会让 sherpa-onnx native 崩溃）。
    #[test]
    fn empty_model_files_are_not_available() {
        let tmp = std::env::temp_dir().join(format!("talksage-asr-empty-{}", std::process::id()));
        let dir = tmp.join(EngineKind::Qwen3Asr.model_dir_name());
        std::fs::create_dir_all(&dir).unwrap();
        // 空文件模拟下载失败残留
        for f in ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        std::fs::create_dir_all(dir.join("tokenizer")).unwrap();
        std::fs::write(dir.join("tokenizer").join("vocab.json"), b"").unwrap();
        assert!(!EngineKind::Qwen3Asr.is_available(&tmp), "空文件不应判定为已安装");
        // 编码器/解码器必须达到最低合理大小；用稀疏文件避免测试实际写入 200MB。
        for name in ["encoder.int8.onnx", "decoder.int8.onnx"] {
            let file = std::fs::OpenOptions::new().write(true).open(dir.join(name)).unwrap();
            file.set_len(100 * 1024 * 1024).unwrap();
        }
        std::fs::write(dir.join("conv_frontend.onnx"), vec![1u8; 1024]).unwrap();
        std::fs::write(dir.join("tokenizer").join("vocab.json"), b"{}").unwrap();
        assert!(EngineKind::Qwen3Asr.is_available(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// 每类流式 (kind, model_dir) 引擎在池中的最大缓存数（防无限累积）。
const POOL_MAX_PER_KEY: usize = 4;

/// 引擎池：按 (kind, model_dir) 缓存已加载的引擎，会话间复用。
///
/// 参考 WhisperLiveKit 的引擎单例设计：模型只加载一次，后续监听
/// 直接从池中取已就绪的引擎（热启动，毫秒级），归还时自动 reset。
/// 离线段级模型体积较大，每种最多保留一个；流式模型最多保留四个。
///
/// 线程安全（内部 Mutex）；`SherpaStreamingEngine` 为 Send（OnlineRecognizer/
/// OnlineStream 均为 Send+Sync），可跨线程借出/归还。
#[derive(Default)]
pub struct EnginePool {
    inner: Mutex<HashMap<String, Vec<Box<dyn SegmentEngine>>>>,
}

impl EnginePool {
    /// 创建空池（常驻于应用状态，跨监听复用）。
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn key(kind: EngineKind, model_dir: &Path, options: &EngineOptions) -> String {
        format!("{}|{}|{}", kind.display_name(), model_dir.display(), options.signature())
    }

    /// 借出引擎：池中有缓存则复用（已 reset），否则新建并加载模型。
    pub fn acquire(&self, kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<Box<dyn SegmentEngine>> {
        self.acquire_with_options(kind, model_dir, num_threads, &EngineOptions::default())
    }

    pub fn acquire_with_options(&self, kind: EngineKind, model_dir: &Path, num_threads: i32, options: &EngineOptions) -> anyhow::Result<Box<dyn SegmentEngine>> {
        let key = Self::key(kind, model_dir, options);
        if let Some(e) = self.inner.lock().unwrap().get_mut(&key).and_then(|v| v.pop()) {
            log::debug!("引擎池命中: {key}");
            return Ok(e);
        }
        log::debug!("引擎池未命中，新建: {key}");
        create_engine_with_options(kind, model_dir, num_threads, options)
    }

    /// 归还引擎：reset 清空识别状态后入池，超出该类型容量时释放。
    pub fn release(&self, kind: EngineKind, model_dir: &Path, engine: Box<dyn SegmentEngine>) {
        self.release_with_options(kind, model_dir, &EngineOptions::default(), engine)
    }

    pub fn release_with_options(&self, kind: EngineKind, model_dir: &Path, options: &EngineOptions, mut engine: Box<dyn SegmentEngine>) {
        engine.reset();
        let key = Self::key(kind, model_dir, options);
        let mut inner = self.inner.lock().unwrap();
        let v = inner.entry(key).or_default();
        let capacity = if kind.is_streaming() { POOL_MAX_PER_KEY } else { 1 };
        if v.len() < capacity {
            v.push(engine);
        } // 超限丢弃（内存释放）
    }

    /// 预加载（预热）：应用启动/首次监听前可调用，避免首段延迟。
    pub fn warmup(&self, kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<()> {
        let engine = self.acquire(kind, model_dir, num_threads)?;
        self.release(kind, model_dir, engine);
        Ok(())
    }

    /// 池内引擎总数（测试/诊断用）。
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().values().map(|v| v.len()).sum()
    }

    /// 主动释放池中模型。桌面应用必须在 Tauri 调用 `process::exit` 前执行；
    /// 否则 Rust Drop 不会运行，whisper.cpp Metal residency set 会残留到 C++
    /// 全局析构阶段并触发断言。
    pub fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        let count = inner.values().map(Vec::len).sum::<usize>();
        inner.clear();
        log::info!("ASR 引擎池已清空: released={count}");
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;

    struct DummyEngine;

    impl SegmentEngine for DummyEngine {
        fn accept(&mut self, _samples: &[f32]) -> Option<String> {
            None
        }

        fn finish(&mut self) -> String {
            String::new()
        }

        fn reset(&mut self) {}

        fn kind(&self) -> EngineKind {
            EngineKind::Qwen3Asr
        }
    }

    /// 探测模型目录（真实模型缺失时跳过，避免失败）。
    fn model_dir(kind: EngineKind) -> Option<std::path::PathBuf> {
        if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
            let dir = std::path::PathBuf::from(d);
            let sub = kind.model_dir_name();
            let p = dir.join(sub);
            if p.is_dir() {
                return Some(p);
            }
        }
        let sub = kind.model_dir_name();
        let candidates = [
            std::path::PathBuf::from("models").join(sub),
            std::path::PathBuf::from("../models").join(sub),
            std::path::PathBuf::from("../../models").join(sub),
        ];
        candidates.into_iter().find(|p| p.is_dir())
    }

    #[test]
    fn acquire_release_reuses_engine() {
        let Some(dir) = model_dir(EngineKind::ParaformerZh) else {
            eprintln!("跳过：缺少 paraformer 模型");
            return;
        };
        let pool = EnginePool::new();
        // 第一次：创建（池空）
        let e1 = pool.acquire(EngineKind::ParaformerZh, &dir, 1).expect("引擎创建失败");
        assert_eq!(pool.len(), 0);
        // 归还 → 入池
        pool.release(EngineKind::ParaformerZh, &dir, e1);
        assert_eq!(pool.len(), 1);
        // 第二次：命中缓存，不再新建
        let e2 = pool.acquire(EngineKind::ParaformerZh, &dir, 1).expect("复用失败");
        assert_eq!(pool.len(), 0);
        pool.release(EngineKind::ParaformerZh, &dir, e2);
        // 不同 kind 互不影响
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn warmup_populates_pool() {
        let Some(dir) = model_dir(EngineKind::ParaformerZh) else {
            eprintln!("跳过：缺少 paraformer 模型");
            return;
        };
        let pool = EnginePool::new();
        pool.warmup(EngineKind::ParaformerZh, &dir, 1).expect("预热失败");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn clear_releases_all_cached_engines() {
        let pool = EnginePool::new();
        pool.inner.lock().unwrap().insert(
            "test".into(),
            vec![Box::new(DummyEngine), Box::new(DummyEngine)],
        );
        assert_eq!(pool.len(), 2);
        pool.clear();
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn pool_key_isolated_by_meeting_hotwords() {
        let dir = Path::new("models/example");
        let a = EngineOptions { hotwords: vec!["TalkSage".into()], hotword_score: 1.5, ..Default::default() };
        let b = EngineOptions { hotwords: vec!["WhisperLiveKit".into()], hotword_score: 1.5, ..Default::default() };
        assert_ne!(EnginePool::key(EngineKind::ZipformerEn, dir, &a), EnginePool::key(EngineKind::ZipformerEn, dir, &b));
    }

    #[test]
    fn pool_key_isolated_by_execution_provider() {
        let dir = Path::new("models/example");
        let cpu = EngineOptions { provider: "cpu".into(), ..Default::default() };
        let coreml = EngineOptions { provider: "coreml".into(), ..Default::default() };
        let cuda = EngineOptions { provider: "cuda".into(), ..Default::default() };
        assert_ne!(EnginePool::key(EngineKind::Qwen3Asr, dir, &cpu), EnginePool::key(EngineKind::Qwen3Asr, dir, &coreml));
        assert_ne!(EnginePool::key(EngineKind::Qwen3Asr, dir, &coreml), EnginePool::key(EngineKind::Qwen3Asr, dir, &cuda));
    }

    #[test]
    fn zipformer_builds_plain_text_hotwords_safely() {
        let Some(dir) = model_dir(EngineKind::ZipformerEn) else {
            eprintln!("跳过：缺少 zipformer 模型");
            return;
        };
        let options = EngineOptions {
            hotwords: vec!["TalkSage".into(), "Whisper Live Kit".into()],
            hotword_score: 1.5,
            ..Default::default()
        };
        let mut engine = create_engine_with_options(EngineKind::ZipformerEn, &dir, 1, &options)
            .expect("普通文本热词应由 sherpa 内部 BPE 编译");
        assert_eq!(engine.kind(), EngineKind::ZipformerEn);
        let wav = dir.join("0.wav");
        if let Some(wave) = sherpa_onnx::Wave::read(&wav.to_string_lossy()) {
            for chunk in wave.samples().chunks(3200) {
                let _ = engine.accept(chunk);
            }
            assert!(!engine.finish().trim().is_empty(), "启用热词后仍应完成真实音频识别");
        }
    }

}
