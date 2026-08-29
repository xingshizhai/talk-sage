//! whisper.cpp GPU 段级 ASR 适配器。
//!
//! 同一份 Rust 代码由 whisper-rs 按编译 feature 选择后端：
//! - macOS（Apple Silicon）：Metal（`metal` feature）；
//! - Windows x64：Vulkan（`vulkan` feature，AMD/Intel/NVIDIA 通吃，同 Dictata）。
//!
//! 引擎逻辑与后端无关：`use_gpu(true)` 由 whisper-rs 内部路由到已编译的后端。

use std::path::Path;
use std::sync::Once;
use std::time::Instant;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use crate::{EngineKind, EngineOptions, SegmentEngine};

static INSTALL_LOG_HOOKS: Once = Once::new();

fn model_file(kind: crate::EngineKind) -> &'static str {
    match kind {
        crate::EngineKind::WhisperMediumMetal => "ggml-medium-q5_0.bin",
        _ => "ggml-large-v3-turbo-q5_0.bin",
    }
}

fn min_model_bytes(kind: crate::EngineKind) -> u64 {
    match kind {
        crate::EngineKind::WhisperMediumMetal => 280 * 1024 * 1024,
        _ => 500 * 1024 * 1024,
    }
}

/// 后端名（日志/诊断用）。
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const BACKEND: &str = "metal";
#[cfg(all(target_os = "windows", target_arch = "x86_64", feature = "vulkan-gpu"))]
const BACKEND: &str = "vulkan";

pub struct WhisperMetalEngine {
    _context: WhisperContext,
    state: WhisperState,
    buffer: Vec<f32>,
    threads: i32,
    initial_prompt: String,
    language: Option<String>,
    kind: crate::EngineKind,
}

impl WhisperMetalEngine {
    pub fn new(kind: crate::EngineKind, model_dir: &Path, num_threads: i32, options: &EngineOptions) -> anyhow::Result<Self> {
        INSTALL_LOG_HOOKS.call_once(whisper_rs::install_logging_hooks);
        let model = model_dir.join(model_file(kind));
        let size = model.metadata().map_err(|error| anyhow::anyhow!("whisper.cpp GPU 模型不可读 {}: {error}", model.display()))?.len();
        if size < min_model_bytes(kind) {
            anyhow::bail!("whisper.cpp GPU 模型不完整: {} ({:.1} MiB)", model.display(), size as f64 / 1024.0 / 1024.0);
        }

        let mut context_params = WhisperContextParameters::default();
        // flash_attn 在 Vulkan 后端部分 GPU 上会触发慢速回退路径，先关闭
        context_params.use_gpu(true).gpu_device(0).flash_attn(false);
        log::info!(
            "whisper.cpp GPU 模型加载开始: model={} size_mib={:.1} whisper_cpp={} backend={} gpu_device=0 flash_attn=false",
            model.display(), size as f64 / 1024.0 / 1024.0, whisper_rs::WHISPER_CPP_VERSION, BACKEND
        );
        let started = Instant::now();
        let context = WhisperContext::new_with_params(&model, context_params)
            .map_err(|error| anyhow::anyhow!("加载 whisper.cpp {BACKEND} 模型失败: {error}"))?;
        let state = context.create_state()
            .map_err(|error| anyhow::anyhow!("创建 whisper.cpp {BACKEND} state 失败: {error}"))?;
        log::info!("whisper.cpp GPU 模型加载完成: elapsed_ms={} backend={BACKEND}", started.elapsed().as_millis());

        Ok(Self {
            _context: context,
            state,
            buffer: Vec::new(),
            threads: num_threads.max(1),
            initial_prompt: options.hotwords.join("，"),
            language: options.language.clone(),
            kind,
        })
    }

    fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        // 语言策略：Some(lang) = 按场景固定（关闭自动检测，避免短句/口音/专业词
        // 漂移到其他语言）；None = 模型每段自动检测。
        match &self.language {
            Some(lang) => {
                params.set_language(Some(lang.as_str()));
                params.set_detect_language(false);
                log::debug!("whisper.cpp {BACKEND} 固定语言: {lang}");
            }
            None => {
                params.set_language(None);
                params.set_detect_language(true);
            }
        }
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if !self.initial_prompt.is_empty() {
            params.set_initial_prompt(&self.initial_prompt);
        }

        let started = Instant::now();
        self.state.full(params, samples)
            .map_err(|error| anyhow::anyhow!("whisper.cpp {BACKEND} 推理失败: {error}"))?;
        let text = self.state.as_iter().map(|segment| segment.to_string()).collect::<String>();
        let elapsed = started.elapsed().as_secs_f64();
        let audio_seconds = samples.len() as f64 / 16_000.0;
        log::info!(
            "whisper.cpp GPU 段级推理完成: audio_ms={:.0} inference_ms={:.0} rtf={:.3} chars={} backend={BACKEND}",
            audio_seconds * 1000.0, elapsed * 1000.0,
            if audio_seconds > 0.0 { elapsed / audio_seconds } else { 0.0 },
            text.chars().count()
        );
        Ok(text.trim().to_string())
    }
}

impl SegmentEngine for WhisperMetalEngine {
    fn accept(&mut self, samples: &[f32]) -> Option<String> {
        self.buffer.extend_from_slice(samples);
        None
    }

    fn finish(&mut self) -> String {
        let samples = std::mem::take(&mut self.buffer);
        self.transcribe(&samples).unwrap_or_else(|error| {
            log::error!("whisper.cpp GPU 段级推理失败: {error:#}");
            String::new()
        })
    }

    fn reset(&mut self) {
        self.buffer.clear();
    }

    fn kind(&self) -> EngineKind {
        self.kind
    }
}
