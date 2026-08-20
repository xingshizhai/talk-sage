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
use sherpa_onnx::{
    OfflineModelConfig, OfflineQwen3ASRModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineStream, OfflineWhisperModelConfig, OnlineModelConfig, OnlineParaformerModelConfig,
    OnlineRecognizer, OnlineRecognizerConfig, OnlineStream, OnlineTransducerModelConfig,
};

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
}

impl EngineKind {
    /// 从配置字符串解析（zipformer-en / paraformer-zh / whisper-base / whisper-small / qwen3-asr / …）。
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "zipformer-en" => Some(Self::ZipformerEn),
            "paraformer-zh" => Some(Self::ParaformerZh),
            "whisper" | "whisper-base" => Some(Self::WhisperBase),
            "whisper-small" => Some(Self::WhisperSmall),
            "qwen3-asr" | "qwen3" => Some(Self::Qwen3Asr),
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
        }
    }

    /// 是否流式（逐块增量出 partial）。false = 离线段级（VAD 段结束后整段识别）。
    pub fn is_streaming(self) -> bool {
        matches!(self, Self::ParaformerZh | Self::ZipformerEn)
    }

    /// 模型目录名（下载脚本约定，与 `scripts/download_models.py` 输出一致）。
    pub fn model_dir_name(self) -> &'static str {
        match self {
            Self::ParaformerZh => "sherpa-onnx-streaming-paraformer-zh",
            Self::ZipformerEn => "sherpa-onnx-streaming-zipformer-en-2023-06-26",
            Self::WhisperBase => "sherpa-onnx-whisper-base",
            Self::WhisperSmall => "sherpa-onnx-whisper-small",
            Self::Qwen3Asr => "sherpa-onnx-qwen3-asr-0.6b",
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

impl SherpaStreamingEngine {
    /// 构建指定引擎（模型文件在 model_dir 下，文件名与下载脚本约定一致）。
    pub fn new(kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<Self> {
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
                    ..Default::default()
                }
            }
            EngineKind::ZipformerEn => {
                let enc = "encoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx";
                let dec = "decoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx";
                let joiner = "joiner-epoch-99-avg-1-chunk-16-left-64.int8.onnx";
                OnlineModelConfig {
                    model_type: Some("zipformer2".into()),
                    modeling_unit: Some("bpe".into()),
                    bpe_vocab: Some(model_dir.join("bpe.model").to_string_lossy().into()),
                    transducer: OnlineTransducerModelConfig {
                        encoder: Some(model_dir.join(enc).to_string_lossy().into()),
                        decoder: Some(model_dir.join(dec).to_string_lossy().into()),
                        joiner: Some(model_dir.join(joiner).to_string_lossy().into()),
                    },
                    tokens: Some(model_dir.join("tokens.txt").to_string_lossy().into()),
                    num_threads,
                    ..Default::default()
                }
            }
            EngineKind::WhisperBase | EngineKind::WhisperSmall | EngineKind::Qwen3Asr => {
                anyhow::bail!("{} 是离线段级引擎，请用 OfflineSegmentEngine::new", kind.display_name())
            }
        };

        let config = OnlineRecognizerConfig {
            model_config,
            decoding_method: Some("greedy_search".into()),
            enable_endpoint: false,
            ..Default::default()
        };

        let recognizer = OnlineRecognizer::create(&config)
            .ok_or_else(|| anyhow::anyhow!("创建 sherpa-onnx 流式识别器失败（模型路径/文件不完整？）"))?;
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
                    provider: Some("cpu".into()),
                    model_type: Some("whisper".into()),
                    ..Default::default()
                }
            }
            EngineKind::Qwen3Asr => OfflineModelConfig {
                qwen3_asr: OfflineQwen3ASRModelConfig {
                    conv_frontend: Some(model_dir.join("conv_frontend.onnx").to_string_lossy().into()),
                    encoder: Some(model_dir.join("encoder.onnx").to_string_lossy().into()),
                    decoder: Some(model_dir.join("decoder.onnx").to_string_lossy().into()),
                    tokenizer: Some(model_dir.join("tokenizer.json").to_string_lossy().into()),
                    max_total_len: 512,
                    max_new_tokens: 256,
                    temperature: 0.0,
                    top_p: 1.0,
                    seed: 42,
                    hotwords: None,
                },
                num_threads,
                debug: false,
                provider: Some("cpu".into()),
                model_type: Some("qwen3_asr".into()),
                ..Default::default()
            },
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
    if kind.is_streaming() {
        Ok(Box::new(SherpaStreamingEngine::new(kind, model_dir, num_threads)?))
    } else {
        Ok(Box::new(OfflineSegmentEngine::new(kind, model_dir, num_threads)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_kind_parsing() {
        assert_eq!(EngineKind::from_name("zipformer-en"), Some(EngineKind::ZipformerEn));
        assert_eq!(EngineKind::from_name("paraformer-zh"), Some(EngineKind::ParaformerZh));
        assert_eq!(EngineKind::from_name("whisper"), Some(EngineKind::WhisperBase));
        assert_eq!(EngineKind::from_name("whisper-base"), Some(EngineKind::WhisperBase));
        assert_eq!(EngineKind::from_name("whisper-small"), Some(EngineKind::WhisperSmall));
        assert_eq!(EngineKind::from_name("qwen3-asr"), Some(EngineKind::Qwen3Asr));
        assert_eq!(EngineKind::from_name("unknown"), None);
        // 流式 vs 离线段级
        assert!(EngineKind::ParaformerZh.is_streaming());
        assert!(!EngineKind::WhisperBase.is_streaming());
        assert!(!EngineKind::Qwen3Asr.is_streaming());
    }
}

/// 每类 (kind, model_dir) 引擎在池中的最大缓存数（防无限累积）。
const POOL_MAX_PER_KEY: usize = 4;

/// 引擎池：按 (kind, model_dir) 缓存已加载的**流式**引擎，会话间复用。
///
/// 参考 WhisperLiveKit 的引擎单例设计：模型只加载一次，后续监听
/// 直接从池中取已就绪的引擎（热启动，毫秒级），归还时自动 reset。
/// 离线段级引擎（whisper/qwen3）不进池（每次新建）。
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

    fn key(kind: EngineKind, model_dir: &Path) -> String {
        format!("{}|{}", kind.display_name(), model_dir.display())
    }

    /// 借出流式引擎：池中有缓存则复用（已 reset），否则新建并加载模型。
    pub fn acquire(&self, kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<Box<dyn SegmentEngine>> {
        let key = Self::key(kind, model_dir);
        if let Some(e) = self.inner.lock().unwrap().get_mut(&key).and_then(|v| v.pop()) {
            log::debug!("引擎池命中: {key}");
            return Ok(e);
        }
        log::debug!("引擎池未命中，新建: {key}");
        Ok(Box::new(SherpaStreamingEngine::new(kind, model_dir, num_threads)?))
    }

    /// 归还流式引擎：reset 清空识别状态后入池（超出容量则丢弃）；离线引擎直接释放。
    pub fn release(&self, kind: EngineKind, model_dir: &Path, mut engine: Box<dyn SegmentEngine>) {
        if !kind.is_streaming() {
            log::debug!("离线引擎不缓存: {key}", key = Self::key(kind, model_dir));
            return;
        }
        engine.reset();
        let key = Self::key(kind, model_dir);
        let mut inner = self.inner.lock().unwrap();
        let v = inner.entry(key).or_default();
        if v.len() < POOL_MAX_PER_KEY {
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
}

#[cfg(test)]
mod pool_tests {
    use super::*;

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
        let candidates = [
            std::path::PathBuf::from("models/sherpa-onnx-streaming-paraformer-zh"),
            std::path::PathBuf::from("../models/sherpa-onnx-streaming-paraformer-zh"),
            std::path::PathBuf::from("../../models/sherpa-onnx-streaming-paraformer-zh"),
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
}
