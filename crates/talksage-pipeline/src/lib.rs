//! TalkSage v2 实时管道：AudioHub → VAD 分段 → 流式 ASR → 领域事件。
//!
//! 双流架构：
//!   user   （麦克风，speaker_id=0，中文 paraformer）→ 用户自己的语音
//!   client （系统回环/文件，speaker_id=1，英文 zipformer）→ 客户语音
//!
//! 每条流独立 VAD 分段 + 流式 ASR（增量 partial → 段结束 final）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use talksage_asr::{EngineKind, EnginePool, SherpaStreamingEngine, StreamingASREngine};
use talksage_audio::{AudioHub, Preprocessor};
use talksage_config::{DenoiseConfig, VadConfig};
use talksage_core::{DomainEvent, StatusStage, TranscriptSegment};
use talksage_plugins::AnalyzerPlugin;

pub mod speaker;

/// 运行期可调参数（监听中可实时修改，无需重启；跨线程共享）。
///
/// 参考 WhisperLiveKit 的"会话状态与计算解耦"思想：把运行期可调状态集中管理，
/// 未来新增参数（实时切换 VAD 灵敏度、降噪强度等）只需在此扩展。
#[derive(Default)]
pub struct RuntimeParams {
    /// 噪音电平阈值（f32 bits；0 = 关闭）：块 RMS 低于该值的音频静音。
    pub noise_level: Arc<AtomicU32>,
}

impl RuntimeParams {
    pub fn with_noise_level(level: f32) -> Self {
        Self {
            noise_level: Arc::new(AtomicU32::new(level.clamp(0.0, 0.5).to_bits())),
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
    /// ASR 推理线程数。
    pub asr_threads: usize,
    /// 用户流（中文）。
    pub user: StreamConfig,
    /// 客户流（英文，可选）。
    pub client: Option<StreamConfig>,
    /// 会议辅助插件（final 段后触发）。
    pub plugins: Vec<Arc<dyn AnalyzerPlugin>>,
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
}

/// 实时管道：持有组件并在专用线程中运行事件循环。
pub struct LivePipeline {
    cfg: Arc<LivePipelineConfig>,
    tx_stop: Option<mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// 运行期参数（与 cfg 共享，`set_noise_level` 实时更新）。
    runtime: Arc<RuntimeParams>,
}

impl LivePipeline {
    pub fn new(cfg: LivePipelineConfig) -> Self {
        let runtime = cfg.runtime.clone();
        Self {
            cfg: Arc::new(cfg),
            tx_stop: None,
            handle: None,
            runtime,
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

    /// 在专用线程中启动管道；返回后可用 `stop()` 停止。
    pub fn start(&mut self, emit: EventSink) -> anyhow::Result<()> {
        let (tx_stop, rx_stop) = mpsc::channel::<()>();
        let cfg = self.cfg.clone();
        let handle = std::thread::Builder::new()
            .name("talksage-pipeline".into())
            .spawn(move || {
                if let Err(e) = run_loop(cfg, rx_stop, emit) {
                    log::error!("pipeline 退出异常: {e}");
                }
            })?;
        self.tx_stop = Some(tx_stop);
        self.handle = Some(handle);
        Ok(())
    }

    /// 停止管道：发送停止信号并**等待线程结束**（保证录音收尾/文件头回填完成）。
    pub fn stop(&mut self) {
        if let Some(tx) = self.tx_stop.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
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

/// 单条流的运行时状态。
struct StreamWorker {
    vad: VoiceActivityDetector,
    /// ASR 引擎（Option 以便归还引擎池）。
    engine: Option<SherpaStreamingEngine>,
    preprocessor: Preprocessor,
    mic_device: Option<String>,
    input_kind: InputKind,
    hub: Option<AudioHub>,
    rx_audio: Option<mpsc::Receiver<Vec<f32>>>,
    file_chunks: Option<std::vec::IntoIter<Vec<f32>>>,
    in_speech: bool,
    last_partial: String,
    speaker_id: u32,
    speaker_label: String,
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
    /// 当前语音段开始时刻（now_ms）。
    seg_start_ms: u64,
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
    /// 当前语音段音频缓冲（说话人识别用，≤30s）。
    seg_audio: Vec<f32>,
    /// 最近一块 RMS（f32 bits；Level 事件用）。
    level: Arc<AtomicU32>,
    /// ASR 引擎池（Some = 引擎常驻复用，shutdown 时归还）。
    engine_pool: Option<Arc<EnginePool>>,
    /// 引擎模型目录（归还引擎池用）。
    engine_dir: Option<PathBuf>,
}

impl StreamWorker {
    fn new(
        cfg: &StreamConfig,
        vad_cfg: &VadConfig,
        denoise: &DenoiseConfig,
        asr_threads: usize,
        vad_model: &PathBuf,
        chunk_ms: u64,
        recording_path: Option<PathBuf>,
        runtime: Arc<RuntimeParams>,
        speaker: Option<speaker::SharedSpeaker>,
        level: Arc<AtomicU32>,
        engine_pool: Option<Arc<EnginePool>>,
    ) -> anyhow::Result<Self> {
        let (threshold, min_speech, min_silence, window, max_speech) = vad_cfg.effective();
        log::info!(
            "流[{}] VAD 参数: preset={:?} threshold={threshold} min_speech={min_speech}s min_silence={min_silence}s window={window} max_speech={max_speech}s",
            cfg.speaker_label,
            vad_cfg.preset,
        );
        let vad = create_vad(vad_model, threshold, min_speech, min_silence, window, max_speech)?;
        // ASR 引擎：优先从引擎池复用（热启动，参考 WhisperLiveKit 引擎单例），否则新建
        let engine = match &engine_pool {
            Some(pool) => pool.acquire(cfg.engine_kind, &cfg.model_dir, asr_threads.max(1) as i32)?,
            None => SherpaStreamingEngine::new(cfg.engine_kind, &cfg.model_dir, asr_threads.max(1) as i32)?,
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
            hub: None,
            rx_audio: None,
            file_chunks,
            in_speech: false,
            last_partial: String::new(),
            speaker_id: cfg.speaker_id,
            speaker_label: cfg.speaker_label.clone(),
            done: false,
            chunk_interval: Duration::from_millis(chunk_ms),
            #[cfg(windows)]
            loopback: None,
            on_final: None,
            recorder,
            recording_path,
            seg_start_ms: 0,
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
            seg_audio: Vec::new(),
            level,
            engine_pool,
            engine_dir: Some(cfg.model_dir.clone()),
            engine: Some(engine),
        })
    }

    /// 启动音频输入（麦克风/回环）。
    fn start_input(&mut self, chunk_ms: u64) -> anyhow::Result<()> {
        match &self.input_kind {
            // 麦克风模式
            InputKind::Mic => {
                let (mut hub, rx) = AudioHub::new(chunk_ms);
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
        self.total_samples += chunk.len() as u64;
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

        self.vad.accept_waveform(&chunk);

        if self.vad.detected() && !self.in_speech {
            self.in_speech = true;
            self.last_partial.clear();
            if let Some(e) = &mut self.engine {
                e.reset();
            }
            self.seg_start_ms = now_ms();
            self.seg_samples = 0;
            self.seg_rms_acc = 0.0;
            self.seg_audio.clear();
        }

        if self.in_speech {
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
            if let Some(engine) = &mut self.engine {
                if let Some(text) = engine.accept(&chunk) {
                    let text = text.trim().to_string();
                    if !text.is_empty() && text != self.last_partial {
                        self.last_partial = text.clone();
                        emit(DomainEvent::Segment {
                            speaker_id: self.speaker_id,
                            speaker_label: self.speaker_label.clone(),
                            text,
                            is_partial: true,
                            ts_ms: now_ms(),
                            duration_ms: 0,
                            rms: 0.0,
                        });
                    }
                }
            }
        }

        while !self.vad.is_empty() {
            self.vad.pop();
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
            Some(engine) => {
                engine.finish();
                engine.accept(&[]).unwrap_or_default().trim().to_string()
            }
            None => String::new(),
        };
        if !final_text.is_empty() {
            // 段统计：时长（VAD 起点到当前）+ 段能量 RMS
            let duration_ms = if self.seg_start_ms > 0 {
                now_ms().saturating_sub(self.seg_start_ms)
            } else {
                self.seg_samples * 1000 / talksage_audio::TARGET_SAMPLE_RATE as u64
            };
            let rms = if self.seg_samples > 0 {
                (self.seg_rms_acc / self.seg_samples as f64) as f32
            } else {
                0.0
            };
            // 说话人判定（声纹）：先识别主人，再区分其他说话人
            let mut label = self.speaker_label.clone();
            let mut sid = self.speaker_id;
            if let Some(sp) = &self.speaker {
                let identified = sp.identify(&self.seg_audio, &self.speaker_label);
                if identified == "我" {
                    label = "我".into();
                    sid = 0;
                } else if let Some(rest) = identified.strip_prefix("客户") {
                    if let Ok(n) = rest.parse::<u32>() {
                        label = identified.clone();
                        sid = n;
                    } else {
                        label = identified.clone();
                    }
                } else {
                    label = identified.clone();
                }
            }
            log::info!(
                "段完成[{}] 说话人判定=[{label}] 时长={}ms rms={rms:.4} 字数={} 文本={}",
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
                ts_ms: now_ms(),
                duration_ms,
                rms,
            };
            self.final_segments += 1;
            emit(DomainEvent::Segment {
                speaker_id: seg.speaker_id,
                speaker_label: seg.speaker_label.clone(),
                text: seg.text.clone(),
                is_partial: false,
                ts_ms: seg.ts_ms,
                duration_ms: seg.duration_ms,
                rms: seg.rms,
            });
            if let Some(hook) = &self.on_final {
                hook(&seg);
            }
        }
        self.last_partial.clear();
        if let Some(e) = &mut self.engine {
            e.reset();
        }
        self.seg_audio.clear();
    }

    /// 流级统计（会话结束回溯用）。
    /// 返回 (total_ms, speech_ms, final_segments, samples, avg_rms, max_rms, non_speech_avg_rms)
    fn session_stats(&self) -> (u64, u64, usize, u64, f32, f32, f32) {
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
        (total_ms, speech_ms, self.final_segments, self.total_samples, avg_rms, self.max_rms, non_speech_avg_rms)
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

    /// 关闭流：收尾未完成的语音段 + 结束录音 + 归还引擎（停止监听/输入结束时调用）。
    fn shutdown(&mut self, emit: &EventSink) {
        self.finish_speech(emit);
        self.stop();
        // 归还 ASR 引擎到池（常驻复用；下次监听热启动）
        if let (Some(pool), Some(dir), Some(engine)) = (self.engine_pool.take(), self.engine_dir.take(), self.engine.take()) {
            pool.release(engine.kind(), &dir, engine);
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
}

fn run_loop(cfg: Arc<LivePipelineConfig>, rx_stop: mpsc::Receiver<()>, emit: EventSink) -> anyhow::Result<()> {
    fn fire(emit: &EventSink, ev: DomainEvent) {
        emit(ev);
    }

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
        let dt = chrono_like_ts(now);
        dt
    };
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
            cfg.asr_threads,
            &cfg.vad_model,
            cfg.chunk_ms,
            rec_path,
            cfg.runtime.clone(),
            shared_speaker.clone(),
            level,
            cfg.engine_pool.clone(),
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
            w.on_final = Some(make_on_final(&cfg, &emit));
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
                w.on_final = Some(make_on_final(&cfg, &emit));
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
    loop {
        match rx_stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
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
        let (total_ms, speech_ms, final_segments, samples, avg_rms, max_rms, non_speech_avg_rms) = w.session_stats();
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
        });
        log::info!(
            "会话统计[{}] total={}ms speech={}ms({:.0}%) segs={} avg_rms={:.4} max_rms={:.4} 背景噪音={:.4} recording={:?}",
            w.speaker_label,
            total_ms,
            speech_ms,
            if total_ms > 0 { speech_ms as f64 / total_ms as f64 * 100.0 } else { 0.0 },
            final_segments,
            avg_rms,
            max_rms,
            non_speech_avg_rms,
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
fn make_on_final(cfg: &LivePipelineConfig, emit: &EventSink) -> Arc<dyn Fn(&TranscriptSegment) + Send + Sync> {
    let cfg = cfg.clone();
    let emit = emit.clone();
    Arc::new(move |seg: &TranscriptSegment| {
        for plugin in &cfg.plugins {
            if !plugin.should_trigger(seg) {
                continue;
            }
            log::debug!("插件[{}] 触发: 段=[{}] {}", plugin.name(), seg.speaker_label, seg.text.chars().take(60).collect::<String>());
            // 骨架（本地即时）
            if let Some(skel) = plugin.skeleton(seg) {
                emit(skel);
            }
            // 最终（可能 LLM）：独立线程，不阻塞管道
            let plugin = plugin.clone();
            let ctx = cfg.plugin_ctx.clone();
            let emit = emit.clone();
            let seg = seg.clone();
            std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                let result = plugin.run(&seg, &ctx);
                log::info!("插件[{}] 完成: 耗时={:?} 有结果={}", plugin.name(), t0.elapsed(), result.is_some());
                if let Some(ev) = result {
                    emit(ev);
                }
            });
        }
    })
}
