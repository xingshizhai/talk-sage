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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use talksage_asr::{EngineKind, EngineOptions, EnginePool};
use talksage_audio::{AudioHub, Preprocessor};
use talksage_config::{DenoiseConfig, EndpointConfig, VadConfig};
use talksage_core::{AudioClock, DomainEvent, StatusStage, TranscriptSegment};
use talksage_plugins::SegmentObserver;

pub mod finalize;
pub mod offline;
pub mod runtime;
pub mod service;
pub mod speaker;

pub use runtime::SessionRuntime;
pub use service::{ClientCapture, RecoveryReport, RunningListen, StartListen, TalkSageService};

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
    /// 会议辅助插件（final 段后触发）。
    pub plugins: Vec<Arc<dyn SegmentObserver>>,
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

/// Whisper Flow 文本稳定思想的低开销版本：不重跑音频窗口，只观察原生流式
/// hypothesis，并要求同时出现短暂停顿后才提交。
struct StableEndpoint {
    config: EndpointConfig,
    stable_samples: u64,
    quiet_samples: u64,
}

impl StableEndpoint {
    fn new(config: EndpointConfig) -> Self {
        Self { config, stable_samples: 0, quiet_samples: 0 }
    }

    fn reset(&mut self) {
        self.stable_samples = 0;
        self.quiet_samples = 0;
    }

    fn enabled(&self) -> bool {
        self.config.enabled
    }

    fn max_wait_ms(&self) -> u64 {
        self.config.stable_ms
    }

    fn observe(&mut self, unchanged_nonempty: bool, rms: f32, chunk_samples: u64, segment_samples: u64) -> bool {
        if !self.config.enabled {
            return false;
        }
        if unchanged_nonempty {
            self.stable_samples += chunk_samples;
        } else {
            self.stable_samples = 0;
        }
        if rms <= self.config.quiet_rms {
            self.quiet_samples += chunk_samples;
        } else {
            self.quiet_samples = 0;
        }
        let ms = |samples: u64| AudioClock::samples_to_ms(talksage_audio::TARGET_SAMPLE_RATE, samples);
        let long_enough = ms(segment_samples) >= self.config.min_segment_ms;
        let stable_pause = ms(self.stable_samples) >= self.config.stable_ms
            && ms(self.quiet_samples) >= self.config.quiet_ms;
        let forced_pause = ms(self.quiet_samples) >= self.config.force_quiet_ms;
        long_enough && (stable_pause || forced_pause)
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
    endpoint: StableEndpoint,
    /// Silero 已检测到段尾；等待流式文本稳定后再提交。
    pending_vad_endpoint: bool,
    pending_endpoint_samples: u64,
    last_partial: String,
    speaker_id: u32,
    speaker_label: String,
    terminology: talksage_config::TerminologyConfig,
    done: bool,
    chunk_interval: Duration,
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
    /// 当前语音段样本数。
    seg_samples: u64,
    /// 当前语音段能量平方和。
    seg_rms_acc: f64,
    /// 该流总样本数。
    total_samples: u64,
    /// 语音段样本数。
    speech_samples: u64,
    /// 能量平方和（avg_rms = sqrt(sum/total)）。
    rms_sum: f64,
    /// 峰值块 RMS。
    max_rms: f32,
    /// 最终段数量。
    final_segments: usize,
    /// 非语音块能量平方和（背景噪音水平；质量评估自动阈值用）。
    non_speech_rms_sum: f64,
    /// 非语音块数。
    non_speech_blocks: u64,
    /// 运行期可调参数（噪音电平阈值等）。
    runtime: Arc<RuntimeParams>,
    /// 说话人识别器（共享；None = 未启用）。
    speaker: Option<speaker::SharedSpeaker>,
    /// 是否允许把当前流识别为已注册主人；客户/回环流始终为 false。
    speaker_recognize_owner: bool,
    /// 当前语音段音频缓冲（说话人识别用，≤30s）。
    seg_audio: Vec<f32>,
    /// 最近一块 RMS（f32 bits；Level 事件用）。
    level: Arc<AtomicU32>,
    /// ASR 引擎池（Some = 引擎常驻复用，shutdown 时归还）。
    engine_pool: Option<Arc<EnginePool>>,
    /// 引擎模型目录（归还引擎池用）。
    engine_dir: Option<PathBuf>,
    engine_options: EngineOptions,
    /// 插件钩子（filter 链）。与其它流共享同一批 filter 实例。
    hooks: talksage_plugins::HookRegistry,
    /// final 段累计词数（会话指标用；借鉴 Call.md）。
    words: usize,
    /// final 段累计问句数（会话指标用）。
    questions: usize,
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
            endpoint: StableEndpoint::new(endpoint.clone()),
            pending_vad_endpoint: false,
            pending_endpoint_samples: 0,
            last_partial: String::new(),
            speaker_id: cfg.speaker_id,
            speaker_label: cfg.speaker_label.clone(),
            terminology: cfg.terminology.clone(),
            done: false,
            chunk_interval: Duration::from_millis(chunk_ms),
            #[cfg(windows)]
            loopback: None,
            on_final: None,
            recorder,
            recording_path,
            seg_start_sample: 0,
            seg_samples: 0,
            seg_rms_acc: 0.0,
            total_samples: 0,
            speech_samples: 0,
            rms_sum: 0.0,
            max_rms: 0.0,
            final_segments: 0,
            non_speech_rms_sum: 0.0,
            non_speech_blocks: 0,
            runtime,
            speaker,
            speaker_recognize_owner,
            seg_audio: Vec::new(),
            level,
            engine_pool,
            engine_dir: Some(cfg.model_dir.clone()),
            engine_options: cfg.engine_options.clone(),
            engine: Some(engine),
            hooks,
            words: 0,
            questions: 0,
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
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.done = true;
                    return Ok(false);
                }
            }
        } else if let Some(iter) = &mut self.file_chunks {
            if let Some(c) = iter.next() {
                std::thread::sleep(self.chunk_interval); // 模拟实时节奏
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
        self.total_samples = self.clock.accepted();
        self.rms_sum += (block_rms as f64) * (block_rms as f64) * chunk.len() as f64;
        if block_rms > self.max_rms {
            self.max_rms = block_rms;
        }
        // 电平指示（Level 事件用）
        self.level.store(block_rms.to_bits(), Ordering::Relaxed);
        // 非语音块（VAD 判定前 in_speech=false）→ 背景噪音水平
        if !self.in_speech {
            self.non_speech_blocks += 1;
            self.non_speech_rms_sum += (block_rms as f64) * (block_rms as f64) * chunk.len() as f64;
        }

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
            self.last_partial.clear();
            self.endpoint.reset();
            self.pending_vad_endpoint = false;
            self.pending_endpoint_samples = 0;
            if let Some(e) = &mut self.engine {
                e.reset();
            }
            self.seg_start_sample = chunk_start;
            self.seg_samples = 0;
            self.seg_rms_acc = 0.0;
            self.seg_audio.clear();

            let buffered_samples = self.pre_roll_samples as u64;
            self.seg_start_sample = chunk_start.saturating_sub(buffered_samples.saturating_sub(chunk.len() as u64));
            for buffered in self.pre_roll.drain(..) {
                self.seg_samples += buffered.len() as u64;
                self.speech_samples += buffered.len() as u64;
                self.seg_rms_acc += buffered.iter().map(|&x| x * x).sum::<f32>() as f64;
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
            if self.pending_vad_endpoint {
                self.pending_endpoint_samples += chunk.len() as u64;
            }
            self.seg_samples += chunk.len() as u64;
            self.speech_samples += chunk.len() as u64;
            self.seg_rms_acc += chunk.iter().map(|&x| x * x).sum::<f32>() as f64;
            // 说话人音频缓冲（预处理后，限 30s，说话人识别用）
            if self.speaker.is_some() {
                const MAX_SEG_AUDIO: usize = 480000; // 30s @16k
                let remain = MAX_SEG_AUDIO.saturating_sub(self.seg_audio.len());
                if remain > 0 {
                    self.seg_audio.extend_from_slice(&chunk[..chunk.len().min(remain)]);
                }
            }
            let mut unchanged_nonempty = false;
            if let Some(engine) = &mut self.engine {
                if let Some(text) = engine.accept(&chunk) {
                    let text = self.terminology.correct(text.trim());
                    unchanged_nonempty = !text.is_empty() && text == self.last_partial;
                    if !text.is_empty() && text != self.last_partial {
                        self.last_partial = text.clone();
                        emit(DomainEvent::Segment {
                            speaker_id: self.speaker_id,
                            speaker_label: self.speaker_label.clone(),
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
            endpoint_ready = self.engine.as_ref().is_some_and(|e| e.kind().is_streaming())
                && self.endpoint.observe(
                    unchanged_nonempty,
                    block_rms,
                    chunk.len() as u64,
                    self.seg_samples,
                );
        }

        let mut vad_endpoint = false;
        while !self.vad.is_empty() {
            self.vad.pop();
            vad_endpoint = true;
            self.pending_vad_endpoint = true;
            self.pending_endpoint_samples = 0;
        }

        let is_streaming = self.engine.as_ref().is_some_and(|e| e.kind().is_streaming());
        let realtime_input = self.input_kind != InputKind::File;
        let natural_endpoint = realtime_input && is_streaming && endpoint_ready && !self.pending_vad_endpoint;
        let commit = if !is_streaming || !self.endpoint.enabled() {
            vad_endpoint
        } else {
            natural_endpoint
                || (self.pending_vad_endpoint
                && (endpoint_ready
                    || AudioClock::samples_to_ms(
                        talksage_audio::TARGET_SAMPLE_RATE,
                        self.pending_endpoint_samples,
                    ) >= self.endpoint.max_wait_ms()))
        };
        if commit {
            if natural_endpoint {
                // 主动端点发生时 Silero 仍认为处于同一语音段。清空其内部状态，
                // 防止稍后产生的旧段尾立即切断下一句。
                self.vad.reset();
                self.pre_roll.clear();
                self.pre_roll_samples = 0;
                log::debug!("流[{}] 文本稳定/强停顿主动提交", self.speaker_label);
            } else if is_streaming && self.endpoint.enabled() {
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
            let duration_ms = AudioClock::samples_to_ms(self.clock.sample_rate(), self.seg_samples);
            let ts_ms = self.origin_ms + AudioClock::samples_to_ms(self.clock.sample_rate(), end_sample);
            let rms = if self.seg_samples > 0 {
                (self.seg_rms_acc / self.seg_samples as f64) as f32
            } else {
                0.0
            };
            // 说话人判定（声纹）：先识别主人，再区分其他说话人。
            //
            // 这里只**查询**：标签既要进事件，也要参与跨流去重的 speaker_id 比较，
            // 所以必须在建事件之前拿到。但注册（分配"客户N"、写入声纹库）推迟到
            // filter 链放行之后 —— 否则被吞掉的段会留下一个幻影说话人，之后真实
            // 的段可能匹配上它。
            let mut label = self.speaker_label.clone();
            let sid = self.speaker_id;
            let mut pending_speaker: Option<speaker::SpeakerQuery> = None;
            let mut speaker_diagnostic: Option<(speaker::SpeakerDecision, Option<f32>)> = None;
            if let Some(sp) = &self.speaker {
                let query = sp.query_for_role(
                    &self.seg_audio,
                    &self.speaker_label,
                    self.speaker_recognize_owner,
                );
                let identified = query.label().to_string();
                speaker_diagnostic = Some((query.decision(), query.similarity()));
                pending_speaker = Some(query);
                if identified == "我" {
                    label = "我".into();
                } else if identified.starts_with("客户") {
                    // speaker_id 表示稳定的业务角色/音频通道，不能被声纹聚类编号覆盖。
                    // 客户编号只用于显示；翻译、指标与跨流去重仍按原通道判断。
                    label = identified.clone();
                } else {
                    label = identified.clone();
                }
            }
            log::info!(
                "段完成[{}] 说话人判定=[{label}] 声纹={speaker_diagnostic:?} 时长={}ms rms={rms:.4} 字数={} 文本={}",
                self.speaker_label,
                duration_ms,
                final_text.chars().count(),
                final_text.chars().take(60).collect::<String>(),
            );
            let seg = TranscriptSegment {
                speaker_id: sid,
                speaker_label: label.clone(),
                text: final_text.clone(),
                is_partial: false,
                ts_ms,
                duration_ms,
                rms,
            };
            // filter 链在产生点施加：被吞掉的事件既不 emit，也不触发 observer。
            // 这一点必须保持——短段抑制原本就同时拦住两者。
            let ev = DomainEvent::Segment {
                speaker_id: seg.speaker_id,
                speaker_label: seg.speaker_label.clone(),
                text: seg.text.clone(),
                is_partial: false,
                ts_ms: seg.ts_ms,
                duration_ms: seg.duration_ms,
                rms: seg.rms,
                revision: 0,
                start_sample: self.seg_start_sample,
                end_sample,
            };
            let Some(ev) = self.hooks.apply_filters(ev) else {
                // 被吞掉：不计统计、不 emit、不触发 observer，但仍要收尾引擎状态。
                //
                // 注意：下面三行收尾与函数末尾那份是**同一段逻辑的两个副本**
                // （提前 return 导致）。改动收尾行为时两处必须一起改，
                // 只改一处会让「被吞掉的段」和「正常段」的引擎状态发散。
                self.last_partial.clear();
                self.endpoint.reset();
                self.pending_vad_endpoint = false;
                self.pending_endpoint_samples = 0;
                if let Some(e) = &mut self.engine {
                    e.reset();
                }
                self.seg_audio.clear();
                return;
            };
            // filter 放行 → 这一段真的存在，此刻才注册说话人。
            if let (Some(sp), Some(query)) = (&self.speaker, &pending_speaker) {
                sp.commit(query);
            }
            // filter 是**变换**而不仅是丢弃：observer 与统计计数器都必须看
            // filter 之后的数据。否则第一个做改写的 filter（脱敏/标点/规范化）
            // 一上线，落库与 sink 的文本就会和插件、words/questions 静默错位。
            let seg = filtered_segment(&ev).unwrap_or(seg);
            self.final_segments += 1;
            self.words += talksage_core::metrics::count_words(&seg.text);
            if talksage_core::metrics::is_question_text(&seg.text) {
                self.questions += 1;
            }
            emit(ev);
            if let Some(hook) = &self.on_final {
                hook(&seg);
            }
        }
        self.last_partial.clear();
        self.endpoint.reset();
        self.pending_vad_endpoint = false;
        self.pending_endpoint_samples = 0;
        if let Some(e) = &mut self.engine {
            e.reset();
        }
        self.seg_audio.clear();
    }

    /// 流级统计（会话结束回溯用）。
    /// 返回 (total_ms, speech_ms, final_segments, samples, avg_rms, max_rms, non_speech_avg_rms, words, questions)
    fn session_stats(&self) -> (u64, u64, usize, u64, f32, f32, f32, usize, usize) {
        let total_ms = self.total_samples * 1000 / talksage_audio::TARGET_SAMPLE_RATE as u64;
        let speech_ms = self.speech_samples * 1000 / talksage_audio::TARGET_SAMPLE_RATE as u64;
        let avg_rms = if self.total_samples > 0 {
            (self.rms_sum / self.total_samples as f64) as f32
        } else {
            0.0
        };
        let non_speech_avg_rms = if self.non_speech_blocks > 0 {
            (self.non_speech_rms_sum / (self.non_speech_blocks as u64 * 1600) as f64) as f32
        } else {
            avg_rms
        };
        (total_ms, speech_ms, self.final_segments, self.total_samples, avg_rms, self.max_rms, non_speech_avg_rms, self.words, self.questions)
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
        self.pending_vad_endpoint = false;
        self.pending_endpoint_samples = 0;
        self.level.store(0.0f32.to_bits(), Ordering::Relaxed);
    }

    /// 暂停时丢弃实时设备产生的数据，防止恢复后识别暂停期间的积压音频。
    fn drain_paused(&mut self) {
        if let Some(rx) = &self.rx_audio {
            while rx.try_recv().is_ok() {}
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
            w.on_final = Some(make_on_final(&cfg, &emit, cancel.clone()));
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
                w.on_final = Some(make_on_final(&cfg, &emit, cancel.clone()));
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
        for w in workers.iter_mut() {
            if w.done {
                continue;
            }
            any_alive = true;
            if let Err(e) = w.tick(&emit) {
                tick_err = Some(e);
                break;
            }
        }
        if tick_err.is_some() || !any_alive {
            break;
        }
    }

    for w in workers.iter_mut() {
        w.shutdown(&emit);
    }
    // 会话统计事件（每条流一条）：质量评估 / 历史回溯的基础数据
    for w in &workers {
        let (total_ms, speech_ms, final_segments, samples, avg_rms, max_rms, non_speech_avg_rms, words, questions) = w.session_stats();
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

/// 构造 final 段回调：骨架同步发，最终（LLM）在独立线程执行。
fn make_on_final(
    cfg: &LivePipelineConfig,
    emit: &EventSink,
    cancel: Arc<AtomicBool>,
) -> Arc<dyn Fn(&TranscriptSegment) + Send + Sync> {
    let cfg = cfg.clone();
    let emit = emit.clone();
    Arc::new(move |seg: &TranscriptSegment| {
        // 两个来源：cfg.plugins 是阶段 5 之前遗留的手工装配（term/translator/
        // brief），cfg.hooks.observers() 是注册表提供的。两者都要派发，否则
        // 直接构造 LivePipelineConfig 的测试会拿不到注册表里的 observer。
        for plugin in cfg.plugins.iter().chain(cfg.hooks.observers()) {
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
            // 最终（可能 LLM）：独立线程，不阻塞管道 / 音频回调
            let plugin = plugin.clone();
            let ctx = cfg.plugin_ctx.clone();
            let emit = emit.clone();
            let seg = seg.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                let t0 = Instant::now();
                let result = plugin.run(&seg, &ctx);
                if cancel.load(Ordering::Relaxed) {
                    log::info!("插件[{}] 会话已停止，丢弃结果", plugin.name());
                    return;
                }
                if t0.elapsed() > PLUGIN_RUN_TIMEOUT {
                    log::warn!("插件[{}] 超时 {:?}，丢弃结果", plugin.name(), t0.elapsed());
                    return;
                }
                log::info!("插件[{}] 完成: 耗时={:?} 有结果={}", plugin.name(), t0.elapsed(), result.is_some());
                if let Some(ev) = result {
                    emit(ev);
                }
            });
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

    #[test]
    fn stable_endpoint_requires_both_text_stability_and_quiet_audio() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 300,
            quiet_ms: 200,
            min_segment_ms: 1000,
            ..EndpointConfig::default()
        });
        let chunk = 1600; // 100ms
        for _ in 0..3 {
            assert!(!endpoint.observe(true, 0.03, chunk, 16000));
        }
        assert!(!endpoint.observe(true, 0.001, chunk, 17600));
        assert!(endpoint.observe(true, 0.001, chunk, 19200));
    }

    #[test]
    fn changed_hypothesis_resets_stability() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 200,
            quiet_ms: 200,
            min_segment_ms: 0,
            ..EndpointConfig::default()
        });
        assert!(!endpoint.observe(true, 0.001, 1600, 1600));
        assert!(!endpoint.observe(false, 0.001, 1600, 3200));
        assert!(!endpoint.observe(true, 0.001, 1600, 4800));
        assert!(endpoint.observe(true, 0.001, 1600, 6400));
    }

    #[test]
    fn long_quiet_pause_commits_even_while_hypothesis_changes() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 400,
            quiet_ms: 400,
            force_quiet_ms: 700,
            min_segment_ms: 1000,
            ..EndpointConfig::default()
        });
        let chunk = 1600; // 100ms
        for index in 0..6 {
            assert!(!endpoint.observe(index % 2 == 0, 0.001, chunk, 16000 + index * chunk));
        }
        assert!(endpoint.observe(false, 0.001, chunk, 25600));
    }

    #[test]
    fn natural_endpoint_respects_minimum_segment_duration() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 200,
            quiet_ms: 200,
            force_quiet_ms: 300,
            min_segment_ms: 1000,
            ..EndpointConfig::default()
        });
        for _ in 0..5 {
            assert!(!endpoint.observe(true, 0.001, 1600, 8000));
        }
        assert!(endpoint.observe(true, 0.001, 1600, 16000));
    }
}
