//! TalkSage v2 实时管道：AudioHub → VAD 分段 → 流式 ASR → 领域事件。
//!
//! 双流架构：
//!   user   （麦克风，speaker_id=0，中文 paraformer）→ 用户自己的语音
//!   client （系统回环/文件，speaker_id=1，英文 zipformer）→ 客户语音
//!
//! 每条流独立 VAD 分段 + 流式 ASR（增量 partial → 段结束 final）。

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use talksage_asr::{EngineKind, SherpaStreamingEngine, StreamingASREngine};
use talksage_audio::{AudioHub, Preprocessor};
use talksage_config::{DenoiseConfig, VadConfig};
use talksage_core::{DomainEvent, StatusStage, TranscriptSegment};
use talksage_plugins::AnalyzerPlugin;

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
}

/// 实时管道：持有组件并在专用线程中运行事件循环。
pub struct LivePipeline {
    cfg: Arc<LivePipelineConfig>,
    tx_stop: Option<mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LivePipeline {
    pub fn new(cfg: LivePipelineConfig) -> Self {
        Self {
            cfg: Arc::new(cfg),
            tx_stop: None,
            handle: None,
        }
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
    engine: SherpaStreamingEngine,
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
    ) -> anyhow::Result<Self> {
        let (threshold, min_speech, min_silence, window, max_speech) = vad_cfg.effective();
        log::info!(
            "流[{}] VAD 参数: preset={:?} threshold={threshold} min_speech={min_speech}s min_silence={min_silence}s window={window} max_speech={max_speech}s",
            cfg.speaker_label,
            vad_cfg.preset,
        );
        let vad = create_vad(vad_model, threshold, min_speech, min_silence, window, max_speech)?;
        let engine = SherpaStreamingEngine::new(cfg.engine_kind, &cfg.model_dir, asr_threads.max(1) as i32)?;
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
            engine,
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

        // 录音：原始 PCM（预处理前），方便后续裁剪/降噪对比
        if let Some(rec) = &mut self.recorder {
            let _ = rec.write(&chunk);
        }

        // 背景噪音预处理（高通/噪声门），再进 VAD/ASR
        self.preprocessor.process(&mut chunk);

        self.vad.accept_waveform(&chunk);

        if self.vad.detected() && !self.in_speech {
            self.in_speech = true;
            self.last_partial.clear();
            self.engine.reset();
        }

        if self.in_speech {
            if let Some(text) = self.engine.accept(&chunk) {
                let text = text.trim().to_string();
                if !text.is_empty() && text != self.last_partial {
                    self.last_partial = text.clone();
                    emit(DomainEvent::Segment {
                        speaker_id: self.speaker_id,
                        speaker_label: self.speaker_label.clone(),
                        text,
                        is_partial: true,
                        ts_ms: now_ms(),
                    });
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
        self.engine.finish();
        let final_text = self.engine.accept(&[]).unwrap_or_default().trim().to_string();
        if !final_text.is_empty() {
            let seg = TranscriptSegment {
                speaker_id: self.speaker_id,
                speaker_label: self.speaker_label.clone(),
                text: final_text.clone(),
                is_partial: false,
                ts_ms: now_ms(),
            };
            emit(DomainEvent::Segment {
                speaker_id: seg.speaker_id,
                speaker_label: seg.speaker_label.clone(),
                text: seg.text.clone(),
                is_partial: false,
                ts_ms: seg.ts_ms,
            });
            if let Some(hook) = &self.on_final {
                hook(&seg);
            }
        }
        self.last_partial.clear();
        self.engine.reset();
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

    /// 关闭流：收尾未完成的语音段 + 结束录音（停止监听/输入结束时调用）。
    fn shutdown(&mut self, emit: &EventSink) {
        self.finish_speech(emit);
        self.stop();
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
    // 录音时间戳：整次监听共用一个
    let rec_ts = {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dt = chrono_like_ts(now);
        dt
    };
    let build = |sc: &StreamConfig| -> anyhow::Result<StreamWorker> {
        let t0 = std::time::Instant::now();
        // 每条流一个录音文件：{ts}_{speaker_label}.wav
        let rec_path = cfg.recording_dir.as_ref().map(|dir| {
            let safe = sc.speaker_label.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
            dir.join(format!("{rec_ts}_{safe}.wav"))
        });
        let mut w = StreamWorker::new(sc, &cfg.vad, &cfg.denoise, cfg.asr_threads, &cfg.vad_model, cfg.chunk_ms, rec_path)?;
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
    loop {
        match rx_stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
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
