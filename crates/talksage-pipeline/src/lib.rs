//! TalkSage v2 实时管道：AudioHub → VAD 分段 → 流式 ASR → 领域事件。
//!
//! 双流架构：
//!   user   （麦克风，speaker_id=0，中文 paraformer）→ 用户自己的语音
//!   client （系统回环/文件，speaker_id=1，英文 zipformer）→ 客户语音
//!
//! 每条流独立 VAD 分段 + 流式 ASR（增量 partial → 段结束 final）。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use talksage_asr::{EngineKind, EngineOptions, EnginePool};
use talksage_audio::{AudioHub, Preprocessor};
use talksage_config::{DenoiseConfig, EndpointConfig, VadConfig};
use talksage_core::{AudioClock, DomainEvent, StatusStage, TranscriptSegment};

pub mod finalize;
mod endpoint;
mod input_scheduler;
pub mod offline;
mod plugin_executor;
pub mod runtime;
mod session_writer;
mod segment;
pub mod service;
pub mod speaker;
mod speaker_assignment;
mod speaker_change;
pub mod speaker_diarization;
mod statistics;

pub use runtime::SessionRuntime;
pub use service::{ClientCapture, RecoveryReport, RunningListen, StartListen, TalkSageService};

use input_scheduler::{poll_audio, AudioPoll, FilePacer, RoundRobin};
use segment::{PartialUpdate, SegmentLifecycle};
use speaker_assignment::SpeakerAssignment;
use speaker_change::SpeakerChangeWorker;
use statistics::{StreamStatistics, StreamStatisticsSnapshot};

/// 停止管道时等待工作线程收尾的时限。超时返回，不卡死 UI；后台仍可能继续 ASR `finish`。
pub const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// 插件 `run`（含 LLM）结果超过此时长则丢弃，避免停止后迟到事件。
pub const PLUGIN_RUN_TIMEOUT: Duration = Duration::from_secs(15);

/// 旁路 `join`：`std::thread::JoinHandle` 无超时，用 channel 包一层。
fn join_with_timeout(handle: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        let _ = tx.send(());
    });
    match rx.recv_timeout(timeout) {
        Ok(()) => true,
        Err(mpsc::RecvTimeoutError::Timeout) => false,
        Err(mpsc::RecvTimeoutError::Disconnected) => true,
    }
}

/// 运行期可调参数（监听中可实时修改，无需重启；跨线程共享）。
///
/// 参考 WhisperLiveKit 的"会话状态与计算解耦"思想：把运行期可调状态集中管理，
/// 未来新增参数（实时切换 VAD 灵敏度、降噪强度等）只需在此扩展。
#[derive(Default)]
pub struct RuntimeParams {
    /// 噪音电平阈值（f32 bits；0 = 关闭）：块 RMS 低于该值的音频静音。
    pub noise_level: Arc<AtomicU32>,
    /// 暂停时继续排空实时设备队列，但不录音、不识别、不推进音频时钟。
    pub paused: Arc<AtomicBool>,
}

impl RuntimeParams {
    pub fn with_noise_level(level: f32) -> Self {
        Self {
            noise_level: Arc::new(AtomicU32::new(level.clamp(0.0, 0.5).to_bits())),
            paused: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// 说话人识别配置（可选）。
#[derive(Clone)]
pub struct SpeakerConfig {
    /// wespeaker 声纹模型路径。
    pub model: PathBuf,
    /// 主人声纹（已注册时 Some）。
    pub owner_embedding: Option<Vec<f32>>,
    /// 判定阈值（余弦相似度）。
    pub threshold: f32,
    /// 双流时麦克风角色已经确定，不再用声纹覆盖；单流会议才识别麦克风内多人。
    pub classify_user_stream: bool,
}

/// 事件发射器（Tauri 侧桥接 app.emit；headless 侧桥接 WS）。Arc 共享，可跨线程。
pub type EventSink = Arc<dyn Fn(DomainEvent) + Send + Sync>;

/// 音频输入源。
#[derive(Debug, Clone)]
pub enum AudioInput {
    /// 麦克风（device 为 None 时用默认设备）。
    Mic(Option<String>),
    /// wav 文件（模拟麦克风，用于无 GUI 验证）。
    File(std::path::PathBuf),
    /// 系统回环（WASAPI loopback，Windows；客户语音来源）。
    Loopback,
}

/// 单条流配置。
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// 引擎类型。
    pub engine_kind: EngineKind,
    /// 引擎模型目录。
    pub model_dir: PathBuf,
    /// 音频输入源。
    pub input: AudioInput,
    /// 说话人 id（事件用）。
    pub speaker_id: u32,
    /// 说话人标签（事件用）。
    pub speaker_label: String,
    /// 该会话的热词上下文；不同上下文使用不同引擎池键。
    pub engine_options: EngineOptions,
    /// 低延迟确定性术语纠错（误识别 → 标准术语）。
    pub terminology: talksage_config::TerminologyConfig,
}

/// 实时管道配置。
#[derive(Clone)]
pub struct LivePipelineConfig {
    /// silero VAD 模型路径。
    pub vad_model: PathBuf,
    /// 音频分块毫秒。
    pub chunk_ms: u64,
    /// VAD 参数（灵敏度预设 + 覆盖）。
    pub vad: VadConfig,
    /// 音频预处理（背景噪音处理）。
    pub denoise: DenoiseConfig,
    /// 流式 partial 稳定性端点参数。
    pub endpoint: EndpointConfig,
    /// ASR 推理线程数。
    pub asr_threads: usize,
    /// 麦克风输入增益；只作用于 AudioInput::Mic。
    pub input_gain_db: f32,
    /// 用户流（中文）。
    pub user: StreamConfig,
    /// 客户流（英文，可选）。
    pub client: Option<StreamConfig>,
    /// 插件上下文（知识库/LLM 共享）。
    pub plugin_ctx: talksage_plugins::PluginContext,
    /// 录音目录（Some 时监听期间把每条流的原始音频保存为 `{ts}_{speaker_label}.wav`）。
    pub recording_dir: Option<PathBuf>,
    /// 运行期可调参数（噪音电平阈值等，监听中实时可调）。
    pub runtime: Arc<RuntimeParams>,
    /// 说话人识别（可选；None = 无声纹/模型，保持流默认标签）。
    pub speaker: Option<SpeakerConfig>,
    /// ASR 引擎池（Some = 引擎常驻复用，监听热启动；None = 每次新建）。
    /// 参考 WhisperLiveKit 引擎单例设计。
    pub engine_pool: Option<Arc<EnginePool>>,
    /// 是否启用标点恢复与语义分段（流式引擎且模型已安装时生效）。
    pub punct_enabled: bool,
    /// 插件钩子（filter 链 + observer）。由 TalkSageService 用
    /// talksage_plugins::build_registry 装配。
    ///
    /// **必须整份克隆给每个 StreamWorker**：`HookRegistry` 克隆的是
    /// `Arc<dyn EventFilter>`，两条流因此共享同一个 `CrossStreamDedupFilter`
    /// 实例（内部历史共享）—— 这是跨流去重能工作的前提。
    pub hooks: talksage_plugins::HookRegistry,
}

/// 实时管道：持有组件并在专用线程中运行事件循环。
pub struct LivePipeline {
    cfg: Arc<LivePipelineConfig>,
    tx_stop: Option<mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// 运行期参数（与 cfg 共享，`set_noise_level` 实时更新）。
    runtime: Arc<RuntimeParams>,
    /// 停止/取消标志（插件线程在发出结果前检查）。
    cancel: Arc<AtomicBool>,
}

impl LivePipeline {
    pub fn new(cfg: LivePipelineConfig) -> Self {
        let runtime = cfg.runtime.clone();
        Self {
            cfg: Arc::new(cfg),
            tx_stop: None,
            handle: None,
            runtime,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 实时调节噪音电平阈值（0 = 关闭；0..0.1 常用范围）。
    /// 无需停止监听，下一音频块即生效。
    pub fn set_noise_level(&self, level: f32) {
        let level = level.clamp(0.0, 0.5);
        self.runtime.noise_level.store(level.to_bits(), Ordering::Relaxed);
        log::info!("运行时噪音电平阈值已更新: {level:.4}");
    }

    /// 当前噪音电平阈值。
    pub fn noise_level(&self) -> f32 {
        f32::from_bits(self.runtime.noise_level.load(Ordering::Relaxed))
    }

    pub fn set_paused(&self, paused: bool) {
        self.runtime.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.runtime.paused.load(Ordering::Acquire)
    }

    /// 在专用线程中启动管道；返回后可用 `stop()` 停止。
    pub fn start(&mut self, emit: EventSink) -> anyhow::Result<()> {
        self.cancel.store(false, Ordering::Relaxed);
        let (tx_stop, rx_stop) = mpsc::channel::<()>();
        let cfg = self.cfg.clone();
        let cancel = self.cancel.clone();
        let handle = std::thread::Builder::new()
            .name("talksage-pipeline".into())
            .spawn(move || {
                if let Err(e) = run_loop(cfg, rx_stop, emit, cancel) {
                    log::error!("pipeline 退出异常: {e}");
                }
            })?;
        self.tx_stop = Some(tx_stop);
        self.handle = Some(handle);
        Ok(())
    }

    /// 停止管道：发停止信号并等待线程结束（默认 [`STOP_JOIN_TIMEOUT`]）。
    pub fn stop(&mut self) {
        let _ = self.stop_with_timeout(STOP_JOIN_TIMEOUT);
    }

    /// 停止管道并在时限内 join。超时返回 `false`（不卡死调用方）。
    pub fn stop_with_timeout(&mut self, timeout: Duration) -> bool {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(tx) = self.tx_stop.take() {
            let _ = tx.send(());
        }
        match self.handle.take() {
            Some(h) => {
                if join_with_timeout(h, timeout) {
                    true
                } else {
                    log::warn!("管道停止超时 ({timeout:?})，后台线程仍在收尾");
                    false
                }
            }
            None => true,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Unix 秒 → "YYYY-MM-DD_HH-MM-SS"（UTC；Hinnant civil date 算法，无 chrono 依赖）。
fn chrono_like_ts(unix_secs: u64) -> String {
    let days = (unix_secs / 86400) as i64;
    let sod = unix_secs % 86400;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        y,
        m,
        d,
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// 创建 silero VAD（参数化：灵敏度由配置决定）。
fn create_vad(
    model: &PathBuf,
    threshold: f32,
    min_speech: f32,
    min_silence: f32,
    window: i32,
    max_speech: f32,
) -> anyhow::Result<VoiceActivityDetector> {
    let vad_cfg = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(model.to_string_lossy().into()),
            threshold,
            min_silence_duration: min_silence,
            min_speech_duration: min_speech,
            window_size: window,
            max_speech_duration: max_speech,
        },
        ten_vad: Default::default(),
        sample_rate: 16000,
        num_threads: 1,
        provider: Some("cpu".into()),
        debug: false,
    };
    VoiceActivityDetector::create(&vad_cfg, 30.0)
        .ok_or_else(|| anyhow::anyhow!("创建 VAD 失败（模型路径错误？）"))
}

/// 输入模式（内部用）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Mic,
    File,
    Loopback,
}

impl InputKind {
    fn audio_source(self) -> talksage_core::AudioSource {
        match self {
            Self::Mic => talksage_core::AudioSource::Microphone,
            Self::File => talksage_core::AudioSource::ImportedFile,
            Self::Loopback => talksage_core::AudioSource::SystemLoopback,
        }
    }
}

/// 单条流的运行时状态。
struct StreamWorker {
    vad: VoiceActivityDetector,
    /// ASR 引擎（Option 以便归还引擎池）。
    /// ASR 引擎（流式 或 离线段级；Option 以便归还引擎池）。
    engine: Option<Box<dyn talksage_asr::SegmentEngine>>,
    preprocessor: Preprocessor,
    mic_device: Option<String>,
    input_kind: InputKind,
    input_gain_db: f32,
    hub: Option<AudioHub>,
    rx_audio: Option<mpsc::Receiver<Vec<f32>>>,
    file_chunks: Option<std::vec::IntoIter<Vec<f32>>>,
    in_speech: bool,
    /// VAD 确认语音前的音频。确认后回放给 ASR，避免截掉中文句首。
    pre_roll: VecDeque<Vec<f32>>,
    pre_roll_samples: usize,
    pre_roll_limit_samples: usize,
    segment: SegmentLifecycle,
    speaker_id: u32,
    speaker_label: String,
    terminology: talksage_config::TerminologyConfig,
    done: bool,
    /// 文件输入实时节拍；None 表示设备输入。
    file_pacer: Option<FilePacer>,
    #[cfg(windows)]
    loopback: Option<talksage_audio::LoopbackCapture>,
    /// final 段完成后的回调（插件触发）。
    on_final: Option<Arc<dyn Fn(&TranscriptSegment) + Send + Sync>>,
    /// 录音器（Some = 本流录音中，原始 PCM 逐块写入）。
    recorder: Option<talksage_audio::wav::WavRecorder>,
    /// 录音输出路径（收尾时记录日志）。
    recording_path: Option<PathBuf>,
    // ── 统计（质量评估 / 历史回溯） ──
    /// 当前语音段开始采样（该流 AudioClock）。
    seg_start_sample: u64,
    statistics: StreamStatistics,
    /// 运行期可调参数（噪音电平阈值等）。
    runtime: Arc<RuntimeParams>,
    /// 说话人识别器（共享；None = 未启用）。
    speaker: Option<speaker::SharedSpeaker>,
    /// 是否允许把当前流识别为已注册主人；客户/回环流始终为 false。
    speaker_recognize_owner: bool,
    /// 当前语音段音频缓冲（说话人识别用，≤30s）。
    seg_audio: Vec<f32>,
    /// 滑动声纹窗口的段内换人检测；没有声纹模型的流不创建。
    speaker_change: Option<SpeakerChangeWorker>,
    /// 最近一块 RMS（f32 bits；Level 事件用）。
    level: Arc<AtomicU32>,
    /// ASR 引擎池（Some = 引擎常驻复用，shutdown 时归还）。
    engine_pool: Option<Arc<EnginePool>>,
    /// 引擎模型目录（归还引擎池用）。
    engine_dir: Option<PathBuf>,
    engine_options: EngineOptions,
    /// 插件钩子（filter 链）。与其它流共享同一批 filter 实例。
    hooks: talksage_plugins::HookRegistry,
    /// 标点恢复与语义分段（流式引擎 + 模型已安装时为 Some）。
    punct_restorer: Option<talksage_asr::PunctuationRestorer>,
    /// 该流采样时钟。
    clock: AudioClock,
    /// 会话墙钟原点（ms）；`ts_ms = origin_ms + clock.ms()`。
    origin_ms: u64,
}

impl StreamWorker {
    fn new(
        cfg: &StreamConfig,
        vad_cfg: &VadConfig,
        denoise: &DenoiseConfig,
        endpoint: &EndpointConfig,
        asr_threads: usize,
        input_gain_db: f32,
        vad_model: &PathBuf,
        chunk_ms: u64,
        recording_path: Option<PathBuf>,
        runtime: Arc<RuntimeParams>,
        speaker: Option<speaker::SharedSpeaker>,
        speaker_recognize_owner: bool,
        level: Arc<AtomicU32>,
        engine_pool: Option<Arc<EnginePool>>,
        hooks: talksage_plugins::HookRegistry,
        origin_ms: u64,
    ) -> anyhow::Result<Self> {
        let (threshold, min_speech, min_silence, window, max_speech) = vad_cfg.effective();
        log::info!(
            "流[{}] VAD 参数: preset={:?} threshold={threshold} min_speech={min_speech}s min_silence={min_silence}s window={window} max_speech={max_speech}s",
            cfg.speaker_label,
            vad_cfg.preset,
        );
        let vad = create_vad(vad_model, threshold, min_speech, min_silence, window, max_speech)?;
        // 所有模型均走引擎池以降低重复监听的启动延迟；离线大模型每种只缓存一个。
        let threads = asr_threads.max(1) as i32;
        let engine: Box<dyn talksage_asr::SegmentEngine> = match &engine_pool {
            Some(pool) => pool.acquire_with_options(cfg.engine_kind, &cfg.model_dir, threads, &cfg.engine_options)?,
            None => talksage_asr::create_engine_with_options(cfg.engine_kind, &cfg.model_dir, threads, &cfg.engine_options)?,
        };
        let preprocessor = Preprocessor::new(
            denoise.enabled,
            denoise.highpass,
            denoise.highpass_cutoff_hz,
            denoise.gate_threshold,
        );

        // 录音器（记录原始 PCM，预处理前）
        let recorder = match &recording_path {
            Some(p) => {
                let r = talksage_audio::wav::WavRecorder::create(p, talksage_audio::TARGET_SAMPLE_RATE)?;
                log::info!("流[{}] 录音中: {}", cfg.speaker_label, p.display());
                Some(r)
            }
            None => None,
        };

        let mic_device = match &cfg.input {
            AudioInput::Mic(d) => d.clone(),
            AudioInput::File(_) => None,
            AudioInput::Loopback => None,
        };
        let input_kind = match &cfg.input {
            AudioInput::Mic(_) => InputKind::Mic,
            AudioInput::File(_) => InputKind::File,
            AudioInput::Loopback => InputKind::Loopback,
        };

        // 文件模式：预读 wav 分块
        let mut file_chunks = None;
        if let AudioInput::File(path) = &cfg.input {
            let wave = sherpa_onnx::Wave::read(&path.to_string_lossy())
                .ok_or_else(|| anyhow::anyhow!("读取 wav 失败: {}", path.display()))?;
            if wave.sample_rate() != 16000 {
                anyhow::bail!("文件输入要求 16kHz wav，当前 {}", wave.sample_rate());
            }
            let chunk_size = (wave.sample_rate() as usize) * (chunk_ms as usize) / 1000;
            let chunks: Vec<Vec<f32>> = wave.samples().chunks(chunk_size).map(|c| c.to_vec()).collect();
            file_chunks = Some(chunks.into_iter());
        }
        let file_pacer = file_chunks
            .as_ref()
            .map(|_| FilePacer::new(Duration::from_millis(chunk_ms)));

        let speaker_change = speaker.clone().and_then(SpeakerChangeWorker::start);
        Ok(Self {
            vad,
            preprocessor,
            mic_device,
            input_kind,
            input_gain_db,
            hub: None,
            rx_audio: None,
            file_chunks,
            in_speech: false,
            pre_roll: VecDeque::new(),
            pre_roll_samples: 0,
            pre_roll_limit_samples: talksage_audio::TARGET_SAMPLE_RATE as usize / 2,
            segment: SegmentLifecycle::new(endpoint.clone()),
            speaker_id: cfg.speaker_id,
            speaker_label: cfg.speaker_label.clone(),
            terminology: cfg.terminology.clone(),
            done: false,
            file_pacer,
            #[cfg(windows)]
            loopback: None,
            on_final: None,
            recorder,
            recording_path,
            seg_start_sample: 0,
            statistics: StreamStatistics::default(),
            runtime,
            speaker,
            speaker_recognize_owner,
            seg_audio: Vec::new(),
            speaker_change,
            level,
            engine_pool,
            engine_dir: Some(cfg.model_dir.clone()),
            engine_options: cfg.engine_options.clone(),
            engine: Some(engine),
            hooks,
            punct_restorer: None,
            clock: AudioClock::new(talksage_audio::TARGET_SAMPLE_RATE),
            origin_ms,
        })
    }

    /// 启动音频输入（麦克风/回环）。
    fn start_input(&mut self, chunk_ms: u64) -> anyhow::Result<()> {
        match &self.input_kind {
            // 麦克风模式
            InputKind::Mic => {
                let (mut hub, rx) = AudioHub::new_with_gain(chunk_ms, self.input_gain_db);
                hub.start(self.mic_device.as_deref())?;
                self.hub = Some(hub);
                self.rx_audio = Some(rx);
            }
            // 文件模式：无采集流（迭代分块）
            InputKind::File => {}
            // 回环模式
            InputKind::Loopback => {
                #[cfg(windows)]
                {
                    let (mut cap, rx) = talksage_audio::LoopbackCapture::new(chunk_ms);
                    cap.start()?;
                    self.loopback = Some(cap);
                    self.rx_audio = Some(rx);
                }
                #[cfg(not(windows))]
                {
                    anyhow::bail!("系统回环采集当前仅支持 Windows");
                }
            }
        }
        Ok(())
    }

    /// 处理一步：取一块音频并处理。返回 Ok(false) 表示本步无数据（继续轮询）。
    fn tick(&mut self, emit: &EventSink) -> anyhow::Result<bool> {
        let chunk: Option<Vec<f32>> = if let Some(rx) = &self.rx_audio {
            match poll_audio(rx) {
                AudioPoll::Chunk(chunk) => Some(chunk),
                AudioPoll::Empty => None,
                AudioPoll::Disconnected => {
                    self.done = true;
                    return Ok(false);
                }
            }
        } else if let Some(iter) = &mut self.file_chunks {
            let due = self.file_pacer.as_ref().is_none_or(FilePacer::due);
            if !due {
                None
            } else if let Some(c) = iter.next() {
                if let Some(pacer) = &mut self.file_pacer {
                    pacer.consumed();
                }
                Some(c)
            } else {
                self.done = true;
                None
            }
        } else {
            self.done = true;
            None
        };

        let Some(mut chunk) = chunk else {
            // 无数据（输入结束）：若语音未收尾，强制 flush 当前段
            if self.done && self.in_speech {
                self.finish_speech(emit);
            }
            return Ok(false);
        };

        // 统计：原始块能量（预处理前，反映环境噪音真实水平）
        let block_rms = if chunk.is_empty() {
            0.0
        } else {
            (chunk.iter().map(|&x| x * x).sum::<f32>() / chunk.len() as f32).sqrt()
        };
        let chunk_start = self.clock.accept(chunk.len() as u64);
        self.statistics
            .observe_block(block_rms, chunk.len(), self.in_speech);
        // 电平指示（Level 事件用）
        self.level.store(block_rms.to_bits(), Ordering::Relaxed);
        // 录音：原始 PCM（预处理前），方便后续裁剪/降噪对比
        if let Some(rec) = &mut self.recorder {
            let _ = rec.write(&chunk);
        }

        // 运行时噪音电平阈值（监听中可调，无需重启）：块能量低于门槛 → 静音（抑制背景噪音）
        let nl = f32::from_bits(self.runtime.noise_level.load(Ordering::Relaxed));
        if nl > 0.0 && block_rms < nl {
            log::debug!("噪音电平阈值={nl:.5} 静音块 RMS={block_rms:.5}");
            chunk.fill(0.0);
        }

        // 背景噪音预处理（高通/噪声门），再进 VAD/ASR
        self.preprocessor.process(&mut chunk);

        // Silero 需要累计 min_speech 后才确认起音。保留最近 500ms，确认时连同
        // 当前块一起回放给 ASR；否则普通话首字很容易被截断。
        if !self.in_speech {
            self.pre_roll_samples += chunk.len();
            self.pre_roll.push_back(chunk.clone());
            while self.pre_roll_samples > self.pre_roll_limit_samples {
                if let Some(old) = self.pre_roll.pop_front() {
                    self.pre_roll_samples = self.pre_roll_samples.saturating_sub(old.len());
                }
            }
        }

        self.vad.accept_waveform(&chunk);

        let mut just_started = false;
        if self.vad.detected() && !self.in_speech {
            self.in_speech = true;
            just_started = true;
            self.segment.begin();
            if let Some(e) = &mut self.engine {
                e.reset();
            }
            self.seg_start_sample = chunk_start;
            self.statistics.start_segment();
            self.seg_audio.clear();

            let buffered_samples = self.pre_roll_samples as u64;
            self.seg_start_sample = chunk_start.saturating_sub(buffered_samples.saturating_sub(chunk.len() as u64));
            for buffered in self.pre_roll.drain(..) {
                self.statistics.observe_speech(&buffered);
                if self.speaker.is_some() {
                    const MAX_SEG_AUDIO: usize = 480000;
                    let remain = MAX_SEG_AUDIO.saturating_sub(self.seg_audio.len());
                    self.seg_audio.extend_from_slice(&buffered[..buffered.len().min(remain)]);
                }
                if let Some(engine) = &mut self.engine {
                    let _ = engine.accept(&buffered);
                }
            }
            self.pre_roll_samples = 0;
        }

        let mut endpoint_ready = false;
        if self.in_speech && !just_started {
            self.segment.advance_pending(chunk.len() as u64);
            self.statistics.observe_speech(&chunk);
            // 说话人音频缓冲（预处理后，限 30s，说话人识别用）
            if self.speaker.is_some() {
                const MAX_SEG_AUDIO: usize = 480000; // 30s @16k
                let remain = MAX_SEG_AUDIO.saturating_sub(self.seg_audio.len());
                if remain > 0 {
                    self.seg_audio.extend_from_slice(&chunk[..chunk.len().min(remain)]);
                }
            }
            let mut partial_update = PartialUpdate::Empty;
            if let Some(engine) = &mut self.engine {
                if let Some(text) = engine.accept(&chunk) {
                    let text = self.terminology.correct(text.trim());
                    partial_update = self.segment.accept_partial(&text);
                    if partial_update == PartialUpdate::Changed {
                        emit(DomainEvent::Segment {
                            speaker_id: self.speaker_id,
                            speaker_label: self.speaker_label.clone(),
                            speaker_attribution: Some(talksage_core::SpeakerAttribution::from_legacy(
                                self.input_kind.audio_source(),
                                &self.speaker_label,
                            )),
                            text,
                            is_partial: true,
                            ts_ms: self.origin_ms + self.clock.ms(),
                            duration_ms: 0,
                            rms: 0.0,
                            revision: 0,
                            start_sample: self.seg_start_sample,
                            end_sample: self.clock.accepted(),
                        });
                    }
                }
            }

            // partial 文字保持低延迟；声纹标签允许稍后确认。连续两个窗口偏离
            // 当前讲话者时，复用 finish_speech 安全收尾；下一块音频开始新段。
            if let Some(worker) = &mut self.speaker_change {
                worker.submit_if_due(&self.seg_audio);
            }
            let speaker_changed = self.speaker_change.as_mut().is_some_and(SpeakerChangeWorker::poll_changed);
            if speaker_changed {
                log::info!("流[{}] 检测到稳定讲话者变化，主动切分当前段", self.speaker_label);
                self.vad.reset();
                self.pre_roll.clear();
                self.pre_roll_samples = 0;
                self.finish_speech(emit);
                return Ok(true);
            }
            endpoint_ready = self.engine.as_ref().is_some_and(|e| e.kind().is_streaming())
                && self.segment.observe_endpoint(
                    partial_update,
                    block_rms,
                    chunk.len() as u64,
                    self.statistics.segment_samples(),
                );
        }

        let mut vad_endpoint = false;
        while !self.vad.is_empty() {
            self.vad.pop();
            vad_endpoint = true;
            self.segment.mark_vad_endpoint();
        }

        let is_streaming = self.engine.as_ref().is_some_and(|e| e.kind().is_streaming());
        let realtime_input = self.input_kind != InputKind::File;
        let decision =
            self.segment
                .decide(is_streaming, realtime_input, endpoint_ready, vad_endpoint);
        if decision.commit {
            if decision.natural {
                // 主动端点发生时 Silero 仍认为处于同一语音段。清空其内部状态，
                // 防止稍后产生的旧段尾立即切断下一句。
                self.vad.reset();
                self.pre_roll.clear();
                self.pre_roll_samples = 0;
                log::debug!("流[{}] 文本稳定/强停顿主动提交", self.speaker_label);
            } else if is_streaming && self.segment.endpoint_enabled() {
                log::debug!("流[{}] VAD 段尾且文本已稳定，提交", self.speaker_label);
            }
            self.finish_speech(emit);
        }
        Ok(true)
    }

    /// 结束当前语音段并发送 final 事件（VAD 段完成或输入结束时调用）。
    fn finish_speech(&mut self, emit: &EventSink) {
        if !self.in_speech {
            return;
        }
        self.in_speech = false;
        let final_text = match &mut self.engine {
            Some(engine) => engine.finish().trim().to_string(),
            None => String::new(),
        };
        let final_text = self.terminology.correct(&final_text);
        if !final_text.is_empty() {
            let end_sample = self.clock.accepted();
            let duration_ms = AudioClock::samples_to_ms(
                self.clock.sample_rate(),
                self.statistics.segment_samples(),
            );
            let ts_ms = self.origin_ms + AudioClock::samples_to_ms(self.clock.sample_rate(), end_sample);
            let rms = self.statistics.segment_rms();
            let assignment = SpeakerAssignment::resolve(
                self.speaker.clone(),
                &self.seg_audio,
                self.speaker_id,
                self.input_kind.audio_source(),
                &self.speaker_label,
                self.speaker_recognize_owner,
            );
            log::info!(
                "段完成[{}] 说话人判定=[{}] attribution={:?} 声纹={:?} 时长={}ms rms={rms:.4} 字数={} 文本={}",
                self.speaker_label,
                assignment.label(),
                assignment.attribution(),
                assignment.diagnostic(),
                duration_ms,
                final_text.chars().count(),
                final_text.chars().take(60).collect::<String>(),
            );

            // 标点恢复 + 语义分段：有 restorer 时把一条长段切成若干子段，
            // 每个子段独立发出；无 restorer 时退化为单段。
            let sub_segments: Vec<(String, u64)> = match &self.punct_restorer {
                Some(restorer) => restorer.restore_and_split(&final_text, duration_ms, 3),
                None => vec![(final_text.clone(), duration_ms)],
            };

            // 预先提取说话人信息，以便在循环中复用（commit 会消耗所有权）。
            let spk_id = assignment.source_id();
            let spk_label = assignment.label().to_string();
            let spk_attribution = Some(assignment.attribution().clone());
            let mut assignment_opt = Some(assignment);

            let seg_start_ms = ts_ms.saturating_sub(duration_ms);
            let mut offset_ms: u64 = 0;
            let sub_count = sub_segments.len();
            for (i, (sub_text, sub_dur)) in sub_segments.into_iter().enumerate() {
                if sub_text.is_empty() {
                    offset_ms += sub_dur;
                    continue;
                }
                let sub_ts = seg_start_ms + offset_ms + sub_dur;
                offset_ms += sub_dur;
                let is_last = i == sub_count - 1;

                let seg = TranscriptSegment {
                    speaker_id: spk_id,
                    speaker_label: spk_label.clone(),
                    speaker_attribution: spk_attribution.clone(),
                    text: sub_text.clone(),
                    is_partial: false,
                    ts_ms: sub_ts,
                    duration_ms: sub_dur,
                    rms,
                };
                // filter 链在产生点施加：被吞掉的事件既不 emit，也不触发 observer。
                // 这一点必须保持——短段抑制原本就同时拦住两者。
                let ev = DomainEvent::Segment {
                    speaker_id: seg.speaker_id,
                    speaker_label: seg.speaker_label.clone(),
                    speaker_attribution: seg.speaker_attribution.clone(),
                    text: seg.text.clone(),
                    is_partial: false,
                    ts_ms: seg.ts_ms,
                    duration_ms: seg.duration_ms,
                    rms: seg.rms,
                    revision: 0,
                    start_sample: self.seg_start_sample,
                    end_sample,
                };
                if let Some(ev) = self.hooks.apply_filters(ev) {
                    // filter 放行 → 第一个放行的子段才注册说话人（只提交一次）。
                    if let Some(a) = assignment_opt.take() {
                        a.commit();
                    }
                    // filter 是**变换**而不仅是丢弃：observer 与统计计数器都必须看
                    // filter 之后的数据。否则第一个做改写的 filter（脱敏/标点/规范化）
                    // 一上线，落库与 sink 的文本就会和插件、words/questions 静默错位。
                    let seg = filtered_segment(&ev).unwrap_or(seg);
                    self.statistics.record_committed_segment(&seg.text);
                    emit(ev);
                    // on_final（插件触发）仅在最后一个子段触发，与单段行为一致。
                    if is_last {
                        if let Some(hook) = &self.on_final {
                            hook(&seg);
                        }
                    }
                }
            }
        }
        self.reset_segment_state();
    }

    /// 所有 final 路径（空文本、filter 吞掉、正常提交）共用同一个收尾出口。
    fn reset_segment_state(&mut self) {
        self.segment.reset();
        if let Some(e) = &mut self.engine {
            e.reset();
        }
        self.seg_audio.clear();
        if let Some(detector) = &mut self.speaker_change {
            detector.reset();
        }
    }

    fn stop(&mut self) {
        if let Some(h) = &mut self.hub {
            h.stop();
        }
        #[cfg(windows)]
        if let Some(l) = &mut self.loopback {
            l.stop();
        }
    }

    fn capture_overruns(&self) -> u64 {
        if let Some(h) = &self.hub {
            return h.overruns();
        }
        #[cfg(windows)]
        if let Some(l) = &self.loopback {
            return l.overruns();
        }
        0
    }

    /// 关闭流：收尾未完成的语音段 + 结束录音 + 归还引擎（停止监听/输入结束时调用）。
    fn shutdown(&mut self, emit: &EventSink) {
        let overruns = self.capture_overruns();
        if overruns > 0 {
            log::warn!(
                "流[{}] 采集 overrun={} 帧（队列容量 {}）",
                self.speaker_label,
                overruns,
                talksage_audio::CAPTURE_QUEUE_CAP
            );
        }
        self.finish_speech(emit);
        self.stop();
        // 归还 ASR 引擎到池（常驻复用；下次监听热启动）
        if let (Some(pool), Some(dir), Some(engine)) = (self.engine_pool.take(), self.engine_dir.take(), self.engine.take()) {
            pool.release_with_options(engine.kind(), &dir, &self.engine_options, engine);
            log::debug!("流[{}] ASR 引擎已归还引擎池", self.speaker_label);
        }
        if let Some(rec) = self.recorder.take() {
            let samples = rec.samples_written();
            match rec.finish() {
                Ok(()) => {
                    log::info!(
                        "流[{}] 录音完成: {}（{} 采样 ≈ {:.1}s）",
                        self.speaker_label,
                        self.recording_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                        samples,
                        samples as f64 / talksage_audio::TARGET_SAMPLE_RATE as f64,
                    );
                }
                Err(e) => log::warn!("流[{}] 录音收尾失败: {e}", self.speaker_label),
            }
        }
    }

    /// 暂停边界：提交已开始的句子并清空 VAD/预滚，恢复后从新句开始。
    fn pause(&mut self, emit: &EventSink) {
        self.finish_speech(emit);
        self.vad.reset();
        self.pre_roll.clear();
        self.pre_roll_samples = 0;
        self.segment.reset();
        self.level.store(0.0f32.to_bits(), Ordering::Relaxed);
    }

    /// 暂停时丢弃实时设备产生的数据，防止恢复后识别暂停期间的积压音频。
    fn drain_paused(&mut self) {
        if let Some(rx) = &self.rx_audio {
            while rx.try_recv().is_ok() {}
        }
        // 文件输入不会被 drain；持续把逻辑时钟推到恢复之后，避免长时间暂停
        // 后为了“追赶旧 deadline”而瞬间灌入大量音频块。
        if let Some(pacer) = &mut self.file_pacer {
            pacer.postpone();
        }
        self.level.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

/// 从 filter 之后的事件还原转写段（observer 与统计都用它，保证与 sink 一致）。
///
/// filter 链的类型是 `DomainEvent -> Option<DomainEvent>`，理论上允许把 Segment
/// 换成别的事件类型；那种情况下没有可交给 observer 的段，返回 None，由调用方
/// 退回产生点的原段。
fn filtered_segment(ev: &DomainEvent) -> Option<TranscriptSegment> {
    let DomainEvent::Segment {
        speaker_id,
        speaker_label,
        speaker_attribution,
        text,
        is_partial,
        ts_ms,
        duration_ms,
        rms,
        ..
    } = ev
    else {
        return None;
    };
    Some(TranscriptSegment {
        speaker_id: *speaker_id,
        speaker_label: speaker_label.clone(),
        speaker_attribution: speaker_attribution.clone(),
        text: text.clone(),
        is_partial: *is_partial,
        ts_ms: *ts_ms,
        duration_ms: *duration_ms,
        rms: *rms,
    })
}

fn run_loop(
    cfg: Arc<LivePipelineConfig>,
    rx_stop: mpsc::Receiver<()>,
    emit: EventSink,
    cancel: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    fn fire(emit: &EventSink, ev: DomainEvent) {
        emit(ev);
    }

    // 会话指标与会中提示已搬到 conversation_metrics observer（阶段 3）。
    // 它们不再包装事件流，因此事件顺序由「metrics 先于 Segment」变为
    // 「Segment 先于 metrics」—— observer 在 on_final 派发，晚于 emit。

    fire(&emit, DomainEvent::Status {
        stage: StatusStage::AsrLoading,
        message: "ASR 加载中…".into(),
    });

    // 所有流共享一个有界慢任务执行器，避免每个 segment/plugin 创建线程。
    let mut plugin_executor = plugin_executor::PluginExecutor::new(2, 32, cancel.clone());
    let plugin_handle = plugin_executor.handle();

    // 构建各流（client 流失败降级为仅 user 流，不影响主链路）
    let mut workers: Vec<StreamWorker> = Vec::new();
    // 说话人识别器（共享；模型缺失/未注册 → None，保持流默认标签）
    let shared_speaker: Option<speaker::SharedSpeaker> = cfg.speaker.as_ref().and_then(|sc| {
        match speaker::SpeakerIdentifier::new(&sc.model, sc.owner_embedding.clone(), sc.threshold) {
            Some(s) => {
                log::info!(
                    "说话人识别已启用: model={} 主人声纹={} 阈值={}",
                    sc.model.display(),
                    if sc.owner_embedding.is_some() { "已注册" } else { "未注册" },
                    sc.threshold,
                );
                Some(Arc::new(s))
            }
            None => {
                log::warn!("说话人识别模型加载失败（降级为默认标签）: {}", sc.model.display());
                None
            }
        }
    });
    // 录音时间戳：整次监听共用一个
    let rec_ts = {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        chrono_like_ts(now)
    };
    let origin_ms = now_ms();
    // 电平指示（麦克风 / 回环），Level 事件节流推送
    let mic_level: Arc<AtomicU32> = Arc::new(AtomicU32::new(0.0f32.to_bits()));
    let loopback_level: Arc<AtomicU32> = Arc::new(AtomicU32::new(0.0f32.to_bits()));
    let build = |sc: &StreamConfig| -> anyhow::Result<StreamWorker> {
        let t0 = std::time::Instant::now();
        // 每条流一个录音文件：{ts}_{speaker_label}.wav
        let rec_path = cfg.recording_dir.as_ref().map(|dir| {
            let safe = sc.speaker_label.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
            dir.join(format!("{rec_ts}_{safe}.wav"))
        });
        let level = if sc.speaker_id == 0 { mic_level.clone() } else { loopback_level.clone() };
        let mut w = StreamWorker::new(
            sc,
            &cfg.vad,
            &cfg.denoise,
            &cfg.endpoint,
            cfg.asr_threads,
            cfg.input_gain_db,
            &cfg.vad_model,
            cfg.chunk_ms,
            rec_path,
            cfg.runtime.clone(),
            if sc.speaker_id == 0 && !cfg.speaker.as_ref().is_some_and(|s| s.classify_user_stream) {
                None
            } else {
                shared_speaker.clone()
            },
            sc.speaker_id == 0,
            level,
            cfg.engine_pool.clone(),
            // clone 共享 Arc<dyn EventFilter>：两条流用同一批 filter 实例
            cfg.hooks.clone(),
            origin_ms,
        )?;
        // 标点恢复：仅流式引擎 + 用户启用 + 模型已安装时激活
        if cfg.punct_enabled && sc.engine_kind.is_streaming() {
            if let Some(models_root) = sc.model_dir.parent() {
                w.punct_restorer = talksage_asr::PunctuationRestorer::try_load(models_root);
                if w.punct_restorer.is_some() {
                    log::info!("流[{}] 标点恢复已启用", sc.speaker_label);
                }
            }
        }
        w.start_input(cfg.chunk_ms)?;
        log::info!(
            "流[{}] 就绪: engine={} model={} 加载耗时={:?}",
            sc.speaker_label,
            sc.engine_kind.display_name(),
            sc.model_dir.display(),
            t0.elapsed()
        );
        Ok(w)
    };
    match build(&cfg.user) {
        Ok(mut w) => {
            w.on_final = Some(make_on_final(&cfg, &emit, plugin_handle.clone()));
            workers.push(w);
        }
        Err(e) => {
            eprintln!("[talksage] 启动失败: {e}");
            fire(&emit, DomainEvent::Status {
                stage: StatusStage::Idle,
                message: format!("启动失败: {e}"),
            });
            return Err(e);
        }
    }
    if let Some(c) = &cfg.client {
        match build(c) {
            Ok(mut w) => {
                w.on_final = Some(make_on_final(&cfg, &emit, plugin_handle.clone()));
                workers.push(w);
            }
            Err(e) => {
                eprintln!("[talksage] 客户流启动失败（降级为仅用户流）: {e}");
                log::warn!("客户流启动失败（降级为仅用户流）: {e}");
            }
        }
    }

    fire(&emit, DomainEvent::Status {
        stage: StatusStage::AsrReady,
        message: "ASR 就绪".into(),
    });
    fire(&emit, DomainEvent::Status {
        stage: StatusStage::Recording,
        message: "监听中…".into(),
    });
    log::info!("管道进入事件循环: {} 条流", workers.len());

    // 事件循环：轮询各流（出错时先收尾再返回，保证录音完成）
    let mut tick_err: Option<anyhow::Error> = None;
    let mut last_level_at = std::time::Instant::now();
    let mut was_paused = false;
    let mut poll_cursor = RoundRobin::default();
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match rx_stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        let paused = cfg.runtime.paused.load(Ordering::Acquire);
        if paused != was_paused {
            if paused {
                for w in workers.iter_mut() {
                    w.pause(&emit);
                }
                fire(&emit, DomainEvent::Status {
                    stage: StatusStage::Paused,
                    message: "已暂停".into(),
                });
            } else {
                fire(&emit, DomainEvent::Status {
                    stage: StatusStage::Recording,
                    message: "监听中…".into(),
                });
            }
            was_paused = paused;
        }
        if paused {
            for w in workers.iter_mut() {
                w.drain_paused();
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }

        // 电平指示：节流 100ms 推送（麦克风 / 回环 RMS）
        if last_level_at.elapsed() >= Duration::from_millis(100) {
            let mic = f32::from_bits(mic_level.load(Ordering::Relaxed));
            let loopback = f32::from_bits(loopback_level.load(Ordering::Relaxed));
            fire(&emit, DomainEvent::Level { mic_rms: mic, loopback_rms: loopback });
            last_level_at = std::time::Instant::now();
        }

        let mut any_alive = false;
        let mut processed_any = false;
        let worker_count = workers.len();
        for offset in 0..worker_count {
            // 每轮换一个起始流，避免固定让 user 流优先于 client 流。
            let index = poll_cursor.index(offset, worker_count);
            let worker = &mut workers[index];
            if worker.done {
                continue;
            }
            any_alive = true;
            match worker.tick(&emit) {
                Ok(processed) => processed_any |= processed,
                Err(e) => {
                    tick_err = Some(e);
                    break;
                }
            }
        }
        poll_cursor.advance(worker_count);
        if tick_err.is_some() || !any_alive {
            break;
        }
        if !processed_any {
            // 所有输入暂时为空时让出 CPU；不再由每个流各阻塞 50ms。
            std::thread::park_timeout(Duration::from_millis(2));
        }
    }

    for w in workers.iter_mut() {
        w.shutdown(&emit);
    }
    plugin_executor.shutdown(!cancel.load(Ordering::Relaxed), Duration::from_millis(500));
    // 会话统计事件（每条流一条）：质量评估 / 历史回溯的基础数据
    for w in &workers {
        let StreamStatisticsSnapshot {
            total_ms,
            speech_ms,
            final_segments,
            samples,
            avg_rms,
            max_rms,
            non_speech_avg_rms,
            words,
            questions,
        } = w.statistics.snapshot(w.clock.sample_rate());
        let (vad_threshold, ..) = cfg.vad.effective();
        fire(&emit, DomainEvent::SessionStats {
            speaker_label: w.speaker_label.clone(),
            total_ms,
            speech_ms,
            final_segments,
            samples,
            avg_rms,
            max_rms,
            non_speech_avg_rms,
            recording: w.recording_path.as_ref().map(|p| p.display().to_string()),
            vad_preset: format!("{:?}", cfg.vad.preset).to_lowercase(),
            vad_threshold,
            words,
            questions,
        });
        log::info!(
            "会话统计[{}] total={}ms speech={}ms({:.0}%) segs={} avg_rms={:.4} max_rms={:.4} 背景噪音={:.4} words={} questions={} recording={:?}",
            w.speaker_label,
            total_ms,
            speech_ms,
            if total_ms > 0 { speech_ms as f64 / total_ms as f64 * 100.0 } else { 0.0 },
            final_segments,
            avg_rms,
            max_rms,
            non_speech_avg_rms,
            words,
            questions,
            w.recording_path,
        );
    }
    fire(&emit, DomainEvent::Status {
        stage: StatusStage::Idle,
        message: "已停止".into(),
    });
    if let Some(e) = tick_err {
        return Err(e);
    }
    Ok(())
}

/// 构造 final 段回调：骨架同步发，最终（LLM）交给会话级有界执行器。
fn make_on_final(
    cfg: &LivePipelineConfig,
    emit: &EventSink,
    executor: plugin_executor::PluginExecutorHandle,
) -> Arc<dyn Fn(&TranscriptSegment) + Send + Sync> {
    let cfg = cfg.clone();
    let emit = emit.clone();
    Arc::new(move |seg: &TranscriptSegment| {
        // 唯一来源：注册表。阶段 5 之前这里还要 chain 上 cfg.plugins
        // （service.rs 手工装配的 term/translator/brief），那条路已删除。
        for plugin in cfg.hooks.observers() {
            if seg.is_partial && !plugin.accepts_speculative() {
                continue;
            }
            if !plugin.should_trigger(seg) {
                continue;
            }
            log::debug!("插件[{}] 触发: 段=[{}] {}", plugin.name(), seg.speaker_label, seg.text.chars().take(60).collect::<String>());
            // 骨架（本地即时，无 HTTP）
            for skel in plugin.skeleton(seg) {
                emit(skel);
            }
            executor.submit(plugin.clone(), cfg.plugin_ctx.clone(), emit.clone(), seg.clone());
        }
    })
}

#[cfg(test)]
mod stop_timeout_tests {
    use super::*;

    #[test]
    fn join_with_timeout_returns_true_when_thread_exits() {
        let h = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(20)));
        assert!(join_with_timeout(h, Duration::from_millis(500)));
    }

    #[test]
    fn join_with_timeout_returns_false_when_exceeded() {
        let h = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(400)));
        assert!(!join_with_timeout(h, Duration::from_millis(50)));
    }

}
