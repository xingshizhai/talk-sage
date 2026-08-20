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
    OnlineModelConfig, OnlineParaformerModelConfig, OnlineRecognizer, OnlineRecognizerConfig,
    OnlineStream, OnlineTransducerModelConfig,
};

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
    /// 英文 streaming zipformer（transducer）。
    ZipformerEn,
    /// 中文 streaming paraformer。
    ParaformerZh,
}

impl EngineKind {
    /// 从配置字符串解析（zipformer-en / paraformer-zh / …）。
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "zipformer-en" => Some(Self::ZipformerEn),
            "paraformer-zh" => Some(Self::ParaformerZh),
            _ => None,
        }
    }

    /// 人类可读名。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ZipformerEn => "zipformer-en",
            Self::ParaformerZh => "paraformer-zh",
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
            EngineKind::ParaformerZh => OnlineModelConfig {
                model_type: Some("paraformer".into()),
                paraformer: OnlineParaformerModelConfig {
                    encoder: Some(model_dir.join("encoder.int8.onnx").to_string_lossy().into()),
                    decoder: Some(model_dir.join("decoder.int8.onnx").to_string_lossy().into()),
                },
                tokens: Some(model_dir.join("tokens.txt").to_string_lossy().into()),
                num_threads,
                ..Default::default()
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_kind_parsing() {
        assert_eq!(EngineKind::from_name("zipformer-en"), Some(EngineKind::ZipformerEn));
        assert_eq!(EngineKind::from_name("paraformer-zh"), Some(EngineKind::ParaformerZh));
        assert_eq!(EngineKind::from_name("unknown"), None);
    }
}

/// 每类 (kind, model_dir) 引擎在池中的最大缓存数（防无限累积）。
const POOL_MAX_PER_KEY: usize = 4;

/// 引擎池：按 (kind, model_dir) 缓存已加载的流式引擎，会话间复用。
///
/// 参考 WhisperLiveKit 的引擎单例设计：模型只加载一次，后续监听
/// 直接从池中取已就绪的引擎（热启动，毫秒级），归还时自动 reset。
///
/// 线程安全（内部 Mutex）；`SherpaStreamingEngine` 为 Send（OnlineRecognizer/
/// OnlineStream 均为 Send+Sync），可跨线程借出/归还。
#[derive(Default)]
pub struct EnginePool {
    inner: Mutex<HashMap<String, Vec<SherpaStreamingEngine>>>,
}

impl EnginePool {
    /// 创建空池（常驻于应用状态，跨监听复用）。
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn key(kind: EngineKind, model_dir: &Path) -> String {
        format!("{}|{}", kind.display_name(), model_dir.display())
    }

    /// 借出引擎：池中有缓存则复用（已 reset），否则新建并加载模型。
    pub fn acquire(&self, kind: EngineKind, model_dir: &Path, num_threads: i32) -> anyhow::Result<SherpaStreamingEngine> {
        let key = Self::key(kind, model_dir);
        if let Some(e) = self.inner.lock().unwrap().get_mut(&key).and_then(|v| v.pop()) {
            log::debug!("引擎池命中: {key}");
            return Ok(e);
        }
        log::debug!("引擎池未命中，新建: {key}");
        SherpaStreamingEngine::new(kind, model_dir, num_threads)
    }

    /// 归还引擎：reset 清空识别状态后入池（超出容量则丢弃）。
    pub fn release(&self, kind: EngineKind, model_dir: &Path, mut engine: SherpaStreamingEngine) {
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
            let sub = match kind {
                EngineKind::ParaformerZh => "sherpa-onnx-streaming-paraformer-zh",
                EngineKind::ZipformerEn => "sherpa-onnx-streaming-zipformer-en-2023-06-26",
            };
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
