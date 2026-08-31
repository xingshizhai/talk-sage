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

pub mod chat;
pub mod finalize;
pub mod knowledge;
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
pub use knowledge::KnowledgeHub;

use input_scheduler::{poll_audio, AudioPoll, FilePacer, RoundRobin};
use segment::{PartialUpdate, SegmentLifecycle};
use speaker_assignment::SpeakerAssignment;
use speaker_change::SpeakerChangeWorker;
use statistics::{StreamStatistics, StreamStatisticsSnapshot};

/// 停止管道时等待工作线程收尾的时限。超时返回，不卡死 UI；后台仍可能继续 ASR `finish`。
pub const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// 插件 `run`（含 LLM）结果超过此时长则丢弃，避免停止后迟到事件。
pub const PLUGIN_RUN_TIMEOUT: Duration = Duration::from_secs(15);

/// 旁路 `join`：`std::thread::JoinHandle` 无超时，用轮询 `is_finished` 实现。
/// 可重入：超时返回 `false` 后句柄仍在，可再次调用继续等待（收尾数据完整性
/// 需要；见 [`LivePipeline::join_remaining`]）。
fn join_with_timeout(handle: &mut std::thread::JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// 按值 join（不重入）：用于不再需要句柄的收尾场景（如插件 worker 池 drain）。
pub(crate) fn join_owned_with_timeout(handle: std::thread::JoinHandle<()>, timeout: Duration) -> bool {
    let mut handle = handle;
    join_with_timeout(&mut handle, timeout)
}

/// 运行期可调参数（监听中可实时修改，无需重启；跨线程共享）。
///
/// 参考 WhisperLiveKit 的"会话状态与计算解耦"思想：把运行期可调状态集中管理，
/// 未来新增参数（实时切换 VAD 灵敏度、降噪强度等）只需在此扩展。
pub struct RuntimeParams {
    /// 噪音电平阈值（f32 bits；0 = 关闭）：块 RMS 低于该值的音频静音。
    pub noise_level: Arc<AtomicU32>,
    /// 暂停时继续排空实时设备队列，但不录音、不识别、不推进音频时钟。
    pub paused: Arc<AtomicBool>,
    /// 文件输入速度（f32 bits，1.0 = 实时，0 = 极速）。
    pub playback_speed: Arc<AtomicU32>,
}

impl Default for RuntimeParams {
    fn default() -> Self {
        Self::with_noise_level(0.0)
    }
}

impl RuntimeParams {
    pub fn with_noise_level(level: f32) -> Self {
        Self {
            noise_level: Arc::new(AtomicU32::new(level.clamp(0.0, 0.5).to_bits())),
            paused: Arc::new(AtomicBool::new(false)),
            playback_speed: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        }
    }

    pub fn set_playback_speed(&self, speed: f32) {
        let speed = if speed <= 0.0 { 0.0 } else { speed.clamp(0.25, 16.0) };
        self.playback_speed.store(speed.to_bits(), Ordering::Release);
    }

    pub fn playback_speed(&self) -> f32 {
        f32::from_bits(self.playback_speed.load(Ordering::Acquire))
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

/// 单次 tick 超过这个时长就告警：事件循环是单线程的，这段时间里转写、另一条流、
/// 采集消费全都停着，采集队列在后面堆积（512 帧 ≈ 51s，超了开始丢语音）。
/// 阿里云引擎收尾最多等 1.5s（见 FINISH_GRACE），所以阈值取在它之上，
/// 避免把"已知的最坏情况"刷成噪音；真正的长阻塞（10s 量级）照样会报出来。
const LOOP_BLOCK_WARN_MS: u64 = 2000;
/// 采集侧超过这个时长没有新帧就告警 —— 那是设备断流，不是消费端卡住。
const CAPTURE_STALL_WARN_MS: u64 = 1500;

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
    /// 阿里云 AccessKey ID（云端 ASR 用）。
    pub aliyun_access_key_id: String,
    /// 阿里云 AccessKey Secret（云端 ASR 用）。
    pub aliyun_access_key_secret: String,
    /// 阿里云语音识别 AppKey。
    pub aliyun_app_key: String,
    /// 启动音频设备前已经解析、校验过的实际执行路线。
    pub asr_route: talksage_asr::AsrRoute,
    /// Tokio 运行时句柄（云端引擎必须；从调用方上下文捕获）。
    pub tokio_handle: Option<tokio::runtime::Handle>,
    /// 插件钩子（filter 链 + observer）。由 TalkSageService 用
    /// talksage_plugins::build_registry 装配。
    ///
    /// **必须整份克隆给每个 StreamWorker**：`HookRegistry` 克隆的是
    /// `Arc<dyn EventFilter>`，两条流因此共享同一个 `CrossStreamDedupFilter`
    /// 实例（内部历史共享）—— 这是跨流去重能工作的前提。
    pub hooks: talksage_plugins::HookRegistry,
    /// 段级引擎（whisper.cpp）的最大段时长（ms）；超限强制切分，避免长段积累。
    /// 0 = 不限制（流式引擎自动禁用此功能）。
    pub force_segment_ms: u64,
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

    pub fn set_playback_speed(&self, speed: f32) {
        self.runtime.set_playback_speed(speed);
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

    /// 停止管道并在时限内 join。超时返回 `false`（不卡死调用方）；
    /// 句柄**保留**，可再调 [`Self::join_remaining`] 继续等待。
    pub fn stop_with_timeout(&mut self, timeout: Duration) -> bool {
        // stop channel 表示正常停止：管道仍需排空已提交的插件任务，
        // 并在 writer 关闭前整理最后一批要点。`cancel` 只留给真正的
        // 强制取消，不能在正常 stop 路径提前置位。
        if let Some(tx) = self.tx_stop.take() {
            let _ = tx.send(());
        }
        match self.handle.as_mut() {
            Some(h) => {
                if join_with_timeout(h, timeout) {
                    self.handle = None;
                    true
                } else {
                    log::warn!("管道停止超时 ({timeout:?})，后台线程仍在收尾");
                    false
                }
            }
            None => true,
        }
    }

    /// 继续等待管道线程退出（`stop_with_timeout` 超时后调用）。
    /// 收尾数据（录音 flush、会话统计）在管道线程内完成，必须在
    /// `finish()` 读取统计之前就绪，否则历史回放会缺主录音/元数据。
    pub fn join_remaining(&mut self, timeout: Duration) -> bool {
        match self.handle.as_mut() {
            Some(h) => {
                if join_with_timeout(h, timeout) {
                    self.handle = None;
                    true
                } else {
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

/// 段结束时随请求送去 ASR 线程的上下文；结果回来时原样带回，
/// 这样发段所需的时间戳/时长/音频都不必在两条线程间共享可变状态。
struct FinishMeta {
    end_sample: u64,
    duration_ms: u64,
    ts_ms: u64,
    rms: f32,
    seg_start_sample: u64,
    /// 说话人识别用的段内音频（未启用声纹时为空）。
    seg_audio: Vec<f32>,
}

enum AsrCmd {
    /// 音频块 + 所属段代号（结果回来时带回，用于丢弃已切段的旧 partial）。
    Accept(Vec<f32>, u64),
    Finish(Box<FinishMeta>),
    Reset,
    Stop,
}

enum AsrOut {
    /// 流式引擎的增量文本 + 它所属的段代号。
    Partial(String, u64),
    /// 段最终结果：已做术语纠正、标点分段与说话人判定。
    Final {
        subs: Vec<(String, u64)>,
        text: String,
        /// 声纹判定也是一次模型推理，同样不该占主线程；commit 留到主线程做。
        assignment: Option<SpeakerAssignment>,
        meta: Box<FinishMeta>,
    },
}

/// 一条流的 ASR 线程句柄。
///
/// 引擎与标点模型都住在那条线程上：段级推理（whisper 一次 2~3s）和标点恢复
/// 因此不再占用采集/VAD 那条线程，界面不会再"卡一下、再一批吐出来"。
struct AsrChannel {
    tx: mpsc::Sender<AsrCmd>,
    rx: mpsc::Receiver<AsrOut>,
    handle: Option<std::thread::JoinHandle<Option<Box<dyn talksage_asr::SegmentEngine>>>>,
    /// 已请求但结果尚未回收的段数（收尾时要等它们回来）。
    pending: usize,
}

impl AsrChannel {
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        label: &str,
        mut engine: Box<dyn talksage_asr::SegmentEngine>,
        punct: Option<talksage_asr::PunctuationRestorer>,
        terminology: talksage_config::TerminologyConfig,
        speaker: Option<speaker::SharedSpeaker>,
        speaker_id: u32,
        audio_source: talksage_core::AudioSource,
        speaker_label: String,
        recognize_owner: bool,
    ) -> Option<Self> {
        let (tx_cmd, rx_cmd) = mpsc::channel::<AsrCmd>();
        let (tx_out, rx_out) = mpsc::channel::<AsrOut>();
        let handle = std::thread::Builder::new()
            .name(format!("talksage-asr-{label}"))
            .spawn(move || {
                while let Ok(cmd) = rx_cmd.recv() {
                    match cmd {
                        AsrCmd::Accept(chunk, gen) => {
                            if let Some(text) = engine.accept(&chunk) {
                                let text = terminology.correct(text.trim());
                                if tx_out.send(AsrOut::Partial(text, gen)).is_err() {
                                    break;
                                }
                            }
                        }
                        AsrCmd::Reset => engine.reset(),
                        AsrCmd::Finish(meta) => {
                            let text = terminology.correct(engine.finish().trim());
                            let subs = match &punct {
                                // 标点恢复同样是一次模型推理，一并留在这条线程上
                                Some(r) => r.restore_and_split(&text, meta.duration_ms, 3),
                                None => vec![(text.clone(), meta.duration_ms)],
                            };
                            // 空文本不会发段，也就不必跑声纹
                            let assignment = (!text.is_empty()).then(|| {
                                SpeakerAssignment::resolve(
                                    speaker.clone(),
                                    &meta.seg_audio,
                                    speaker_id,
                                    audio_source,
                                    &speaker_label,
                                    recognize_owner,
                                )
                            });
                            if tx_out.send(AsrOut::Final { subs, text, assignment, meta }).is_err() {
                                break;
                            }
                        }
                        AsrCmd::Stop => break,
                    }
                }
                Some(engine)
            })
            .map_err(|e| log::error!("ASR 线程创建失败: {e}"))
            .ok()?;
        Some(Self { tx: tx_cmd, rx: rx_out, handle: Some(handle), pending: 0 })
    }

    fn accept(&self, chunk: Vec<f32>, generation: u64) {
        let _ = self.tx.send(AsrCmd::Accept(chunk, generation));
    }

    fn reset(&self) {
        let _ = self.tx.send(AsrCmd::Reset);
    }

    /// 请求收尾当前段；结果稍后从 [`Self::try_recv`] 取回。
    fn request_finish(&mut self, meta: FinishMeta) {
        if self.tx.send(AsrCmd::Finish(Box::new(meta))).is_ok() {
            self.pending += 1;
        }
    }

    fn try_recv(&mut self) -> Option<AsrOut> {
        match self.rx.try_recv() {
            Ok(out) => {
                if matches!(out, AsrOut::Final { .. }) {
                    self.pending = self.pending.saturating_sub(1);
                }
                Some(out)
            }
            Err(_) => None,
        }
    }

    /// 阻塞等一个结果（收尾时用，带超时兜底）。
    fn recv_timeout(&mut self, timeout: Duration) -> Option<AsrOut> {
        match self.rx.recv_timeout(timeout) {
            Ok(out) => {
                if matches!(out, AsrOut::Final { .. }) {
                    self.pending = self.pending.saturating_sub(1);
                }
                Some(out)
            }
            Err(_) => None,
        }
    }

    /// 停线程并取回引擎（归还引擎池用）。
    fn stop(&mut self) -> Option<Box<dyn talksage_asr::SegmentEngine>> {
        let _ = self.tx.send(AsrCmd::Stop);
        self.handle.take().and_then(|h| h.join().ok()).flatten()
    }
}

/// 单条流的运行时状态。
struct StreamWorker {
    vad: VoiceActivityDetector,
    /// 构造期暂存的 ASR 引擎；`spawn_asr` 把它移交给 ASR 线程后为 None。
    engine: Option<Box<dyn talksage_asr::SegmentEngine>>,
    /// 引擎种类（流式与否等判断用）。引擎本体在 ASR 线程上，这里留一份种类。
    engine_kind: talksage_asr::EngineKind,
    /// ASR 线程句柄：accept / finish / 标点恢复都在那边跑。
    asr: Option<AsrChannel>,
    /// 当前段代号：每次收尾自增，用来丢弃切段后才回来的旧 partial。
    asr_generation: u64,
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
    file_total_samples: u64,
    file_processed_samples: u64,
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
    /// 段级引擎的强制切分阈值（ms）；0 = 不限制。
    force_segment_ms: u64,
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
        aliyun_app_key: String,
        asr_route: talksage_asr::AsrRoute,
        aliyun_token_manager: Option<Arc<talksage_asr::aliyun::TokenManager>>,
        tokio_handle: Option<tokio::runtime::Handle>,
    ) -> anyhow::Result<Self> {
        let (threshold, min_speech, min_silence, window, max_speech) = vad_cfg.effective();
        log::info!(
            "流[{}] VAD 参数: preset={:?} threshold={threshold} min_speech={min_speech}s min_silence={min_silence}s window={window} max_speech={max_speech}s",
            cfg.speaker_label,
            vad_cfg.preset,
        );
        let vad = create_vad(vad_model, threshold, min_speech, min_silence, window, max_speech)?;
        // 引擎选择：云端（阿里云）或本地（GPU 自动检测）。
        let threads = asr_threads.max(1) as i32;
        let mut effective_options = cfg.engine_options.clone();
        let (engine, pooled): (Box<dyn talksage_asr::SegmentEngine>, bool) = if asr_route
            == talksage_asr::AsrRoute::AliyunCloud
        {
            log::info!("ASR 引擎：阿里云实时语音识别（云端）");
            use talksage_asr::aliyun::AliyunEngine;
            let handle = tokio_handle.ok_or_else(|| {
                anyhow::anyhow!("云端 ASR 需要 Tokio runtime，当前启动入口未提供")
            })?;
            let token_mgr = aliyun_token_manager.ok_or_else(|| {
                anyhow::anyhow!("云端 ASR 路由缺少共享 TokenManager")
            })?;
            (
                Box::new(AliyunEngine::new(&aliyun_app_key, token_mgr, handle)),
                false,
            )
        } else {
            // whisper.cpp GPU 是独立 adapter，不依赖 sherpa provider。离线 bench
            // 可能构造 CPU route，但实际引擎仍必须如实记录/隔离为对应 GPU 后端。
            let provider = if matches!(cfg.engine_kind, talksage_asr::EngineKind::WhisperMediumMetal | talksage_asr::EngineKind::WhisperLargeV3TurboMetal) {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                { "metal" }
                #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
                { "vulkan" }
                #[cfg(not(any(all(target_os = "macos", target_arch = "aarch64"), all(target_os = "windows", target_arch = "x86_64"))))]
                { "cpu" }
            } else {
                asr_route.provider().expect("local route has provider")
            };
            effective_options.provider = provider.to_string();
            log::info!("ASR 引擎：本地 {:?} (provider={provider})", cfg.engine_kind);
            let engine = match &engine_pool {
                Some(pool) => pool.acquire_with_options(
                    cfg.engine_kind,
                    &cfg.model_dir,
                    threads,
                    &effective_options,
                )?,
                None => talksage_asr::create_engine_with_options(
                    cfg.engine_kind,
                    &cfg.model_dir,
                    threads,
                    &effective_options,
                )?,
            };
            (engine, engine_pool.is_some())
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

        // 文件模式：统一解码 WAV / MP3 / MP4 音轨，转为 16kHz mono 后分块。
        let mut file_chunks = None;
        let mut file_total_samples = 0u64;
        if let AudioInput::File(path) = &cfg.input {
            let (sample_rate, samples) = talksage_audio::read_audio_file(path)
                .map_err(|error| anyhow::anyhow!("读取导入音频失败 {}: {error:#}", path.display()))?;
            let samples = talksage_audio::resample_linear(
                &samples,
                sample_rate,
                talksage_audio::TARGET_SAMPLE_RATE,
            );
            file_total_samples = samples.len() as u64;
            let chunk_size = talksage_audio::TARGET_SAMPLE_RATE as usize * chunk_ms as usize / 1000;
            let chunks: Vec<Vec<f32>> = samples.chunks(chunk_size).map(|c| c.to_vec()).collect();
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
            file_total_samples,
            file_processed_samples: 0,
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
            engine_pool: pooled.then_some(engine_pool).flatten(),
            engine_dir: pooled.then(|| cfg.model_dir.clone()),
            engine_options: effective_options,
            engine_kind: cfg.engine_kind,
            engine: Some(engine),
            asr: None,
            asr_generation: 0,
            hooks,
            punct_restorer: None,
            clock: AudioClock::new(talksage_audio::TARGET_SAMPLE_RATE),
            origin_ms,
            force_segment_ms: 0,
        })
    }

    /// 把引擎与标点模型移交给本流的 ASR 线程。
    ///
    /// 之前这两样都在采集/VAD 那条线程上同步跑：whisper 段级推理一次 2~3s、
    /// 云端收尾还要等网络，期间转写不出字、另一条流也被拖住。
    fn spawn_asr(&mut self) {
        let Some(engine) = self.engine.take() else { return };
        let punct = self.punct_restorer.take();
        match AsrChannel::spawn(
            &self.speaker_label,
            engine,
            punct,
            self.terminology.clone(),
            self.speaker.clone(),
            self.speaker_id,
            self.input_kind.audio_source(),
            self.speaker_label.clone(),
            self.speaker_recognize_owner,
        ) {
            Some(ch) => self.asr = Some(ch),
            None => log::error!("流[{}] ASR 线程启动失败，本流将不产出文本", self.speaker_label),
        }
    }

    /// 启动音频输入（麦克风/回环）。
    fn start_input(&mut self, chunk_ms: u64) -> anyhow::Result<()> {
        match &self.input_kind {
            // 麦克风模式
            InputKind::Mic => {
                let (mut hub, rx) = AudioHub::new_with_gain(chunk_ms, self.input_gain_db);
                // 让采集回调在音频线程直接更新 level，不经过 tick()，避免 ASR 推理阻塞时电平冻结
                hub.set_level(self.level.clone());
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
        // 先收 ASR 线程的结果：没有新音频时也要收，否则静音期间做完的段
        // 要等下一块音频到了才发出来。
        let drained = self.drain_asr(emit);
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
                    pacer.consumed(self.runtime.playback_speed());
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

        if self.input_kind == InputKind::File {
            self.file_processed_samples = (self.file_processed_samples + chunk.len() as u64)
                .min(self.file_total_samples);
            let rate = talksage_audio::TARGET_SAMPLE_RATE as u64;
            emit(DomainEvent::MediaProgress {
                position_ms: self.file_processed_samples * 1000 / rate,
                total_ms: self.file_total_samples * 1000 / rate,
                speed: self.runtime.playback_speed(),
            });
        }

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
            if let Some(asr) = &self.asr {
                asr.reset();
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
                if let Some(asr) = &self.asr {
                    asr.accept(buffered.clone(), self.asr_generation);
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
            if let Some(asr) = &self.asr {
                asr.accept(chunk.clone(), self.asr_generation);
            }
            // 强制切分必须在当前块送入 ASR 之后执行。旧实现先
            // finish + return，会整块丢掉切点附近的音频，表现为句尾字消失。
            if self.force_segment_ms > 0 && !self.engine_kind.is_streaming() {
                let elapsed_ms = AudioClock::samples_to_ms(
                    talksage_audio::TARGET_SAMPLE_RATE,
                    self.statistics.segment_samples(),
                );
                if elapsed_ms >= self.force_segment_ms {
                    log::info!(
                        "流[{}] 段时长 {}ms 超限（max={}ms），当前块已入 ASR，强制切分",
                        self.speaker_label, elapsed_ms, self.force_segment_ms
                    );
                    self.vad.reset();
                    self.pre_roll.clear();
                    self.pre_roll_samples = 0;
                    self.finish_speech(emit);
                    return Ok(true);
                }
            }
            // 送完这块再收一次：刚才那次 accept 可能已经产出 partial
            let partial_update = match self.drain_asr(emit) {
                PartialUpdate::Empty => drained,
                update => update,
            };

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
            endpoint_ready = self.engine_kind.is_streaming()
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

        let is_streaming = self.engine_kind.is_streaming();
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

    /// 收取 ASR 线程的结果：partial 直接发事件，final 走完整的发段流程。
    /// 返回本轮最后一次 partial 的更新状态（端点判定用）。
    fn drain_asr(&mut self, emit: &EventSink) -> PartialUpdate {
        let mut update = PartialUpdate::Empty;
        loop {
            let Some(out) = self.asr.as_mut().and_then(AsrChannel::try_recv) else { break };
            match out {
                AsrOut::Partial(text, generation) => {
                    if generation != self.asr_generation {
                        continue; // 这条 partial 属于已经切走的段，丢弃
                    }
                    update = self.segment.accept_partial(&text);
                    if update == PartialUpdate::Changed {
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
                AsrOut::Final { subs, text, assignment, meta } => {
                    self.emit_final(emit, text, subs, assignment, *meta)
                }
            }
        }
        update
    }

    /// 等待所有已请求但未回收的 final（暂停 / 停止监听时用）。
    ///
    /// 段级引擎一次推理 2~3s，收尾时必须等它回来，否则最后一句话会丢。
    fn drain_asr_blocking(&mut self, emit: &EventSink, budget: Duration) {
        let deadline = std::time::Instant::now() + budget;
        while self.asr.as_ref().is_some_and(|a| a.pending > 0) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                let pending = self.asr.as_ref().map(|a| a.pending).unwrap_or(0);
                log::warn!("流[{}] 收尾等待 ASR 超时，放弃 {} 段", self.speaker_label, pending);
                break;
            }
            let Some(out) = self.asr.as_mut().and_then(|a| a.recv_timeout(left)) else { break };
            match out {
                AsrOut::Partial(..) => {}
                AsrOut::Final { subs, text, assignment, meta } => {
                    self.emit_final(emit, text, subs, assignment, *meta)
                }
            }
        }
    }

    /// 结束当前语音段并发送 final 事件（VAD 段完成或输入结束时调用）。
    /// 请求收尾当前段：把上下文交给 ASR 线程，本线程立刻返回。
    ///
    /// 结果（含推理与标点恢复）稍后由 [`Self::drain_asr`] 取回并发段——
    /// 这正是"不再卡住"的关键：推理期间采集与 VAD 继续跑。
    fn finish_speech(&mut self, _emit: &EventSink) {
        if !self.in_speech {
            return;
        }
        self.in_speech = false;
        let meta = FinishMeta {
            end_sample: self.clock.accepted(),
            duration_ms: AudioClock::samples_to_ms(
                self.clock.sample_rate(),
                self.statistics.segment_samples(),
            ),
            ts_ms: self.origin_ms
                + AudioClock::samples_to_ms(self.clock.sample_rate(), self.clock.accepted()),
            rms: self.statistics.segment_rms(),
            seg_start_sample: self.seg_start_sample,
            seg_audio: std::mem::take(&mut self.seg_audio),
        };
        if let Some(asr) = &mut self.asr {
            asr.request_finish(meta);
        }
        self.asr_generation = self.asr_generation.wrapping_add(1);
        self.reset_segment_state();
    }

    /// ASR 线程返回结果后的发段流程（说话人判定 → filter → 事件 → 插件）。
    fn emit_final(
        &mut self,
        emit: &EventSink,
        final_text: String,
        sub_segments: Vec<(String, u64)>,
        assignment: Option<SpeakerAssignment>,
        meta: FinishMeta,
    ) {
        if let (false, Some(assignment)) = (final_text.is_empty(), assignment) {
            let FinishMeta { end_sample, duration_ms, ts_ms, rms, seg_start_sample, seg_audio: _ } = meta;
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

                let seg = TranscriptSegment { id: None,
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
                    start_sample: seg_start_sample,
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
    }

    /// 所有 final 路径（空文本、filter 吞掉、正常提交）共用同一个收尾出口。
    fn reset_segment_state(&mut self) {
        self.segment.reset();
        if let Some(asr) = &self.asr {
            asr.reset();
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

    /// 距上次采集入队的毫秒数。用来区分两种同样表现为"界面冻住"的故障：
    /// 采集还在推数据 → 是消费端（ASR 推理）卡了；采集也停了 → 是设备断流。
    fn since_last_capture_ms(&self) -> Option<u64> {
        if let Some(h) = &self.hub {
            return h.since_last_push_ms();
        }
        #[cfg(windows)]
        if let Some(l) = &self.loopback {
            return l.since_last_push_ms();
        }
        None
    }

    /// 关闭流：收尾未完成的语音段 + 结束录音 + 归还引擎（停止监听/输入结束时调用）。
    /// 收尾：等 ASR 把已请求的段做完（段级推理一次 2~3s），再停线程取回引擎。
    fn finish_and_wait(&mut self, emit: &EventSink) {
        self.finish_speech(emit);
        self.drain_asr(emit);
        self.drain_asr_blocking(emit, Duration::from_secs(10));
    }

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
        // 先把最后一段做完（含等待推理），否则收尾时会丢掉最后一句
        self.finish_and_wait(emit);
        self.stop();
        // 停 ASR 线程并取回引擎，再归还引擎池（常驻复用；下次监听热启动）
        let engine = self.asr.as_mut().and_then(AsrChannel::stop).or_else(|| self.engine.take());
        if let (Some(pool), Some(dir), Some(engine)) = (self.engine_pool.take(), self.engine_dir.take(), engine) {
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
        // 暂停也要等最后一段出来，否则暂停前那句话会消失
        self.finish_and_wait(emit);
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
    Some(TranscriptSegment { id: None,
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

    // 双流共享 Token 缓存，避免同一会话重复请求阿里云 Token。
    let aliyun_token_manager = (cfg.asr_route == talksage_asr::AsrRoute::AliyunCloud).then(|| {
        Arc::new(talksage_asr::aliyun::TokenManager::new(
            &cfg.aliyun_access_key_id,
            &cfg.aliyun_access_key_secret,
        ))
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
            cfg.aliyun_app_key.clone(),
            cfg.asr_route,
            aliyun_token_manager.clone(),
            cfg.tokio_handle.clone(),
        )?;
        // 段级引擎（whisper.cpp）设置强制切分阈值
        w.force_segment_ms = cfg.force_segment_ms;
        log::info!(
            "流[{}] ASR 切段策略: engine={} max_segment_ms={}",
            sc.speaker_label,
            sc.engine_kind.display_name(),
            w.force_segment_ms,
        );
        // 标点恢复独立于 ASR 类型；离线大模型和云端结果也需要统一语义分句。
        if cfg.punct_enabled {
            if let Some(models_root) = sc.model_dir.parent() {
                w.punct_restorer = talksage_asr::PunctuationRestorer::try_load(models_root);
                if w.punct_restorer.is_some() {
                    log::info!("流[{}] 标点恢复已启用", sc.speaker_label);
                }
            }
        }
        // 引擎与标点模型都就位后再开线程：此后主线程不再直接碰它们
        w.spawn_asr();
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

    // 电平推送独立成线程：它只读两个 atomic，不该被事件循环里的 ASR 推理拖住。
    // 之前挂在循环里，一次长推理会让界面电平也冻住，用户以为整个程序死了。
    let level_stop = Arc::new(AtomicBool::new(false));
    let level_thread = {
        let emit = emit.clone();
        let mic_level = mic_level.clone();
        let loopback_level = loopback_level.clone();
        let paused = cfg.runtime.paused.clone();
        let stop = level_stop.clone();
        std::thread::Builder::new()
            .name("talksage-level".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if !paused.load(Ordering::Acquire) {
                        let mic = f32::from_bits(mic_level.load(Ordering::Relaxed));
                        let loopback = f32::from_bits(loopback_level.load(Ordering::Relaxed));
                        fire(&emit, DomainEvent::Level { mic_rms: mic, loopback_rms: loopback });
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .ok()
    };

    // 事件循环：轮询各流（出错时先收尾再返回，保证录音完成）
    let mut tick_err: Option<anyhow::Error> = None;
    let mut last_stall_check = std::time::Instant::now();
    let mut last_plugin_idle_poll = std::time::Instant::now();
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

        // 真正的尾部超时：即使用户说完后一直静音，也会把不足
        // batch_size 的要点任务交给后台执行器，不再依赖下一段转写。
        if last_plugin_idle_poll.elapsed() >= Duration::from_millis(250) {
            for plugin in cfg.hooks.observers() {
                if plugin.idle_trigger_due() {
                    let idle = TranscriptSegment { id: None,
                        speaker_id: 0,
                        speaker_label: "系统".into(),
                        speaker_attribution: None,
                        text: String::new(),
                        is_partial: false,
                        ts_ms: 0,
                        duration_ms: 0,
                        rms: 0.0,
                    };
                    if !plugin_handle.submit(plugin.clone(), cfg.plugin_ctx.clone(), emit.clone(), idle) {
                        plugin.idle_trigger_rejected();
                    }
                }
            }
            last_plugin_idle_poll = std::time::Instant::now();
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

        // 采集断流巡检：每秒看一次。采集停了 → 设备侧问题；采集在推而这里迟迟
        // 没轮到 → 是消费端（ASR 推理）卡住。两者界面表现一样，日志必须分得开。
        if last_stall_check.elapsed() >= Duration::from_secs(1) {
            for worker in workers.iter() {
                if let Some(idle_ms) = worker.since_last_capture_ms() {
                    if idle_ms >= CAPTURE_STALL_WARN_MS {
                        log::warn!(
                            "流[{}] 采集断流 {}ms（设备侧无数据，非 ASR 阻塞）",
                            worker.speaker_label,
                            idle_ms
                        );
                    }
                }
            }
            last_stall_check = std::time::Instant::now();
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
            let tick_started = std::time::Instant::now();
            let tick_result = worker.tick(&emit);
            let tick_ms = tick_started.elapsed().as_millis() as u64;
            if tick_ms >= LOOP_BLOCK_WARN_MS {
                // 事件循环是单线程的：这段时间里电平之外的一切都停了，
                // 采集队列同时在堆积（512 帧 ≈ 51s，超了就丢帧丢语音）。
                log::warn!(
                    "事件循环被流[{}] 阻塞 {}ms（采集队列在此期间持续堆积）",
                    worker.speaker_label,
                    tick_ms
                );
            }
            match tick_result {
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

    // 电平线程先停：它还在往 emit 上推事件，收尾期间没必要再推
    level_stop.store(true, Ordering::Relaxed);
    if let Some(t) = level_thread {
        if !join_owned_with_timeout(t, Duration::from_millis(500)) {
            log::warn!("电平推送线程未在 500ms 内退出，放弃等待");
        }
    }

    for w in workers.iter_mut() {
        w.shutdown(&emit);
    }
    plugin_executor.shutdown(!cancel.load(Ordering::Relaxed), PLUGIN_RUN_TIMEOUT + Duration::from_secs(1));
    if !cancel.load(Ordering::Relaxed) {
        // executor 已 drain，没有并发 LLM 任务；此时安全整理不足一批的尾段。
        cfg.hooks.flush_key_points_remaining(&cfg.plugin_ctx, &|ev| emit(ev));
    }
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
mod pipeline_config_tests {
    use super::*;

    #[test]
    fn pipeline_config_has_aliyun_fields() {
        // Verify the new fields exist and have the right default values by
        // using a closure that references them (compile-time field name check).
        let _ = |cfg: &LivePipelineConfig| {
            assert!(cfg.aliyun_access_key_id.is_empty());
            assert!(cfg.aliyun_access_key_secret.is_empty());
            assert!(cfg.aliyun_app_key.is_empty());
            assert_eq!(cfg.asr_route, talksage_asr::AsrRoute::Local {
                backend: talksage_asr::GpuBackend::None,
            });
        };
    }
}

#[cfg(test)]
mod stop_timeout_tests {
    use super::*;

    #[test]
    fn join_with_timeout_returns_true_when_thread_exits() {
        let mut h = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(20)));
        assert!(join_with_timeout(&mut h, Duration::from_millis(500)));
    }

    #[test]
    fn join_with_timeout_returns_false_when_exceeded() {
        let mut h = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(400)));
        assert!(!join_with_timeout(&mut h, Duration::from_millis(50)));
        // 句柄保留，可继续等待（收尾数据完整性）
        assert!(join_with_timeout(&mut h, Duration::from_millis(1000)));
    }

}
