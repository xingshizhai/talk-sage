//! Apple Silicon whisper.cpp/Metal 段级 ASR 适配器。

use std::path::Path;
use std::sync::Once;
use std::time::Instant;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

use crate::{EngineKind, EngineOptions, SegmentEngine};

const MODEL_FILE: &str = "ggml-large-v3-turbo-q5_0.bin";
static INSTALL_LOG_HOOKS: Once = Once::new();

pub struct WhisperMetalEngine {
    _context: WhisperContext,
    state: WhisperState,
    buffer: Vec<f32>,
    threads: i32,
    initial_prompt: String,
}

impl WhisperMetalEngine {
    pub fn new(model_dir: &Path, num_threads: i32, options: &EngineOptions) -> anyhow::Result<Self> {
        INSTALL_LOG_HOOKS.call_once(whisper_rs::install_logging_hooks);
        let model = model_dir.join(MODEL_FILE);
        let size = model.metadata().map_err(|error| anyhow::anyhow!("Whisper Metal 模型不可读 {}: {error}", model.display()))?.len();
        if size < 500 * 1024 * 1024 {
            anyhow::bail!("Whisper Metal 模型不完整: {} ({:.1} MiB)", model.display(), size as f64 / 1024.0 / 1024.0);
        }

        let mut context_params = WhisperContextParameters::default();
        context_params.use_gpu(true).gpu_device(0).flash_attn(true);
        log::info!(
            "Whisper Metal 模型加载开始: model={} size_mib={:.1} whisper_cpp={} gpu_device=0 flash_attn=true",
            model.display(), size as f64 / 1024.0 / 1024.0, whisper_rs::WHISPER_CPP_VERSION
        );
        let started = Instant::now();
        let context = WhisperContext::new_with_params(&model, context_params)
            .map_err(|error| anyhow::anyhow!("加载 whisper.cpp Metal 模型失败: {error}"))?;
        let state = context.create_state()
            .map_err(|error| anyhow::anyhow!("创建 whisper.cpp Metal state 失败: {error}"))?;
        log::info!("Whisper Metal 模型加载完成: elapsed_ms={} backend=metal", started.elapsed().as_millis());

        Ok(Self {
            _context: context,
            state,
            buffer: Vec::new(),
            threads: num_threads.max(1),
            initial_prompt: options.hotwords.join("，"),
        })
    }

    fn transcribe(&mut self, samples: &[f32]) -> anyhow::Result<String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        // language=None 已表示自动识别。detect_language=true 是 whisper.cpp 的
        // “只检测语言后退出”模式，会得到语言概率但不生成任何文本。
        params.set_language(None);
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
            .map_err(|error| anyhow::anyhow!("Whisper Metal 推理失败: {error}"))?;
        let text = self.state.as_iter().map(|segment| segment.to_string()).collect::<String>();
        let elapsed = started.elapsed().as_secs_f64();
        let audio_seconds = samples.len() as f64 / 16_000.0;
        log::info!(
            "Whisper Metal 段级推理完成: audio_ms={:.0} inference_ms={:.0} rtf={:.3} chars={}",
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
            log::error!("Whisper Metal 段级推理失败: {error:#}");
            String::new()
        })
    }

    fn reset(&mut self) {
        self.buffer.clear();
    }

    fn kind(&self) -> EngineKind {
        EngineKind::WhisperLargeV3TurboMetal
    }
}
