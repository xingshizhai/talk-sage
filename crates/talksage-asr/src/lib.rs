//! TalkSage v2 流式 ASR 引擎。
//!
//! M1 PoC：基于 sherpa-onnx（Rust 绑定）的流式识别封装。
//! 设计目标：`StreamingASREngine` trait 与传输/管道无关，
//! 双引擎（英文 zipformer / 中文 paraformer）由配置选择。

use std::path::Path;
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
