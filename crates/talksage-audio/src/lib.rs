//! TalkSage v2 音频采集中枢。
//!
//! M1：麦克风采集（cpal）→ 单声道 → 重采样到 16kHz → 按固定时长块
//! 通过 mpsc 通道交给 pipeline 线程（回调线程不做重活）。
//!
//! 回环采集（WASAPI loopback / ScreenCaptureKit）为 M1b 预留。
//!
//! 录音与音频处理：wav 读写（wav）、静音裁剪（silence_trim）。

use std::sync::mpsc;
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub mod silence_trim;
pub mod wav;

/// 目标采样率（sherpa-onnx 模型统一 16kHz）。
pub const TARGET_SAMPLE_RATE: u32 = 16000;

#[cfg(windows)]
mod loopback;

#[cfg(windows)]
pub use loopback::LoopbackCapture;

/// 音频预处理（背景噪音处理）。
///
/// - 高通滤波：去除低频轰鸣/空调声（一阶 Butterworth 双二阶）。
/// - 噪声门：块 RMS 低于阈值视为静音（抑制稳态背景噪音）。
///
/// 在 pipeline 每块进入 VAD/ASR 前调用。
pub struct Preprocessor {
    highpass: Option<HighPass>,
    gate_threshold: f32,
}

impl Preprocessor {
    /// `denoise_enabled`：总开关；`highpass`：高通开关；`cutoff_hz` 截止频率。
    pub fn new(denoise_enabled: bool, highpass: bool, cutoff_hz: f32, gate_threshold: f32) -> Self {
        Self {
            highpass: if denoise_enabled && highpass {
                Some(HighPass::new(cutoff_hz))
            } else {
                None
            },
            gate_threshold: if denoise_enabled { gate_threshold } else { 0.0 },
        }
    }

    /// 就地处理一块音频（f32，任意长度）。
    pub fn process(&mut self, samples: &mut [f32]) {
        if let Some(hp) = &mut self.highpass {
            hp.process(samples);
        }
        if self.gate_threshold > 0.0 {
            // 块级噪声门：整块 RMS 低于阈值 → 静音（保守，避免切词）
            let rms = if samples.is_empty() {
                0.0
            } else {
                (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
            };
            if rms < self.gate_threshold {
                samples.fill(0.0);
            }
        }
    }
}

/// 一阶高通滤波器（双二阶，直接 I 型）。
struct HighPass {
    // 状态变量
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
    // 系数
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl HighPass {
    /// 16kHz 下按截止频率计算系数（一阶 Butterworth 高通）。
    fn new(cutoff_hz: f32) -> Self {
        let sample_rate = TARGET_SAMPLE_RATE as f32;
        let w0 = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate;
        let cos_w0 = w0.cos();
        let alpha = (w0 / 2.0).sin();
        // 一阶低通原型 → 高通系数
        let b0 = (1.0 + cos_w0) / 2.0;
        let b1 = -(1.0 + cos_w0);
        let b2 = (1.0 + cos_w0) / 2.0;
        let a0 = 1.0 + alpha;
        Self {
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    fn process(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            let x = *s;
            let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
            self.x2 = self.x1;
            self.x1 = x;
            self.y2 = self.y1;
            self.y1 = y;
            *s = y;
        }
    }
}

/// 线性插值重采样（f32 mono）。源/目标采样率相等时原样返回。
/// 公开给外部（headless 转写 API 把非 16k 上传音频归一化）。
pub fn resample_linear(src: &[f32], src_sr: u32, dst_sr: u32) -> Vec<f32> {
    if src.is_empty() || src_sr == dst_sr {
        return src.to_vec();
    }
    let ratio = src_sr as f64 / dst_sr as f64;
    let out_len = (src.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        if idx + 1 < src.len() {
            let frac = (pos - idx as f64) as f32;
            out.push(src[idx] * (1.0 - frac) + src[idx + 1] * frac);
        } else {
            out.push(src[idx]);
        }
    }
    out
}

/// 线性插值重采样器（f32 mono）。
pub(crate) struct LinearResampler {
    src_sr: u32,
    dst_sr: u32,
}

impl LinearResampler {
    fn new(src_sr: u32, dst_sr: u32) -> Self {
        Self { src_sr, dst_sr }
    }

    /// 把 src 重采样到目标采样率。
    fn process(&mut self, src: &[f32]) -> Vec<f32> {
        resample_linear(src, self.src_sr, self.dst_sr)
    }
}

/// 音频采集中枢：cpal 回调 → 16k mono 块 → mpsc 通道。
pub struct AudioHub {
    /// 目标采样率（16k）。
    pub sample_rate: u32,
    /// 每个块的目标采样数（chunk_ms * sr / 1000）。
    chunk_samples: usize,
    /// 发给 pipeline 的块通道。
    tx: mpsc::Sender<Vec<f32>>,
    /// cpal 输入流。
    stream: Option<cpal::Stream>,
}

impl AudioHub {
    /// 创建采集中枢，返回 (hub, 块接收端)。
    pub fn new(chunk_ms: u64) -> (Self, mpsc::Receiver<Vec<f32>>) {
        let (tx, rx) = mpsc::channel();
        let chunk_samples = (TARGET_SAMPLE_RATE as u64 * chunk_ms / 1000) as usize;
        (
            Self {
                sample_rate: TARGET_SAMPLE_RATE,
                chunk_samples,
                tx,
                stream: None,
            },
            rx,
        )
    }

    /// 启动麦克风采集。`device_name` 为 None 时用系统默认输入设备。
    pub fn start(&mut self, device_name: Option<&str>) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let host = cpal::default_host();
        let device = match device_name {
            Some(name) => host
                .input_devices()?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("未找到输入设备: {name}"))?,
            None => host
                .default_input_device()
                .ok_or_else(|| anyhow::anyhow!("无默认输入设备"))?,
        };
        let config = device.default_input_config()?;
        let src_sr = config.sample_rate().0;
        let channels = config.channels() as usize;

        log::info!(
            "麦克风: {} @ {}Hz {}ch ({:?})",
            device.name().unwrap_or_default(),
            src_sr,
            channels,
            config.sample_format()
        );

        let tx = self.tx.clone();
        let chunk_samples = self.chunk_samples;
        let mut resampler = LinearResampler::new(src_sr, TARGET_SAMPLE_RATE);
        let mut pending: Vec<f32> = Vec::with_capacity(chunk_samples);

        // 输入回调（cpal 音频线程；只做轻量处理）
        let err_fn = |e| log::error!("音频流错误: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _| {
                        collect_and_send(data, channels, &mut resampler, &mut pending, chunk_samples, &tx);
                    },
                    err_fn,
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _| {
                        let f: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                        collect_and_send(&f, channels, &mut resampler, &mut pending, chunk_samples, &tx);
                    },
                    err_fn,
                    None,
                )?
            }
            other => anyhow::bail!("不支持的采样格式: {other:?}"),
        };

        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    /// 停止采集。
    pub fn stop(&mut self) {
        if let Some(s) = self.stream.take() {
            drop(s);
        }
    }
}

/// 采集回调公共逻辑：mono 化 → 重采样 → 按块发送。
#[allow(clippy::too_many_arguments)]
fn collect_and_send(
    data: &[f32],
    channels: usize,
    resampler: &mut LinearResampler,
    pending: &mut Vec<f32>,
    chunk_samples: usize,
    tx: &mpsc::Sender<Vec<f32>>,
) {
    // mono 化（平均）
    let mono: Vec<f32> = if channels > 1 {
        data.chunks(channels)
            .map(|c| c.iter().sum::<f32>() / c.len() as f32)
            .collect()
    } else {
        data.to_vec()
    };
    let resampled = resampler.process(&mono);
    pending.extend_from_slice(&resampled);
    // 按块切分发送
    let mut i = 0;
    while pending.len() - i >= chunk_samples {
        let chunk: Vec<f32> = pending[i..i + chunk_samples].to_vec();
        i += chunk_samples;
        if tx.send(chunk).is_err() {
            break;
        }
    }
    if i > 0 {
        pending.drain(..i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_passthrough_same_rate() {
        let mut r = LinearResampler::new(16000, 16000);
        let src: Vec<f32> = vec![0.1, -0.2, 0.3];
        let out = r.process(&src);
        assert_eq!(out, src);
    }

    #[test]
    fn resampler_downsample_48k_to_16k_length() {
        let mut r = LinearResampler::new(48000, 16000);
        let src: Vec<f32> = (0..4800).map(|i| (i as f32 / 4800.0 * std::f32::consts::TAU).sin()).collect();
        let out = r.process(&src);
        assert_eq!(out.len(), 1600); // 4800/3
    }

    #[test]
    fn resampler_linear_interpolation_value() {
        let mut r = LinearResampler::new(2, 4); // 上采样 2→4
        let src = vec![0.0, 10.0];
        let out = r.process(&src);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 5.0).abs() < 1e-5); // 采样点 0.5 → 线性插值 5.0
        assert_eq!(out[2], 10.0);
    }

    #[test]
    fn chunking_mono_exact_blocks() {
        let (tx, rx) = mpsc::channel();
        let mut pending = Vec::new();
        let mut resampler = LinearResampler::new(16000, 16000);
        let data: Vec<f32> = (0..3200).map(|i| i as f32).collect(); // 200ms @16k = 3200
        collect_and_send(&data, 1, &mut resampler, &mut pending, 1600, &tx);
        assert!(pending.is_empty());
        let c1 = rx.recv().unwrap();
        let c2 = rx.recv().unwrap();
        assert_eq!(c1.len(), 1600);
        assert_eq!(c2.len(), 1600);
        assert!(rx.try_recv().is_err());
        assert_eq!(c1[0], 0.0);
        assert_eq!(c2[0], 1600.0);
    }

    #[test]
    fn chunking_stereo_mixes_to_mono() {
        let (tx, rx) = mpsc::channel();
        let mut pending = Vec::new();
        let mut resampler = LinearResampler::new(16000, 16000);
        // 立体声：左 0,2,4..；右 1,3,5.. → mono 平均值
        let data: Vec<f32> = (0..640).map(|i| i as f32).collect(); // 320 帧立体声
        collect_and_send(&data, 2, &mut resampler, &mut pending, 320, &tx);
        assert!(pending.is_empty());
        let c = rx.recv().unwrap();
        assert_eq!(c.len(), 320);
        assert!((c[0] - 0.5).abs() < 1e-5);
        assert!((c[1] - 2.5).abs() < 1e-5);
    }

    #[test]
    fn chunking_partial_block_kept_in_pending() {
        let (tx, rx) = mpsc::channel();
        let mut pending = Vec::new();
        let mut resampler = LinearResampler::new(16000, 16000);
        let data: Vec<f32> = (0..1000).map(|i| i as f32).collect(); // 不足一块
        collect_and_send(&data, 1, &mut resampler, &mut pending, 1600, &tx);
        assert!(rx.try_recv().is_err());
        assert_eq!(pending.len(), 1000);
    }

    #[test]
    fn highpass_reduces_low_frequency_content() {
        let mut hp = HighPass::new(100.0);
        let sr = 16000.0;
        let n = 1600; // 100ms
        // 低频 50Hz 正弦（应被大幅衰减）+ 高频 2000Hz 正弦（应保留）
        let mut samples: Vec<f32> = (0..n)
            .map(|i| {
                0.5 * (2.0 * std::f32::consts::PI * 50.0 * i as f32 / sr).sin()
                    + 0.5 * (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / sr).sin()
            })
            .collect();
        hp.process(&mut samples);
        // 稳定后（前 400 样本为暂态），计算 50Hz 与 2000Hz 段能量
        let stable = &samples[400..];
        let energy = |freq: f32| -> f32 {
            // 逐点乘参考正弦并积分（粗略频域能量）
            let mut acc = 0.0f32;
            for (k, s) in stable.iter().enumerate() {
                acc += s * (2.0 * std::f32::consts::PI * freq * (k as f32 + 400.0) / sr).sin();
            }
            acc.abs()
        };
        let e50 = energy(50.0);
        let e2000 = energy(2000.0);
        assert!(e50 < e2000, "50Hz 应被衰减: e50={e50} e2000={e2000}");
        // 输入混合信号 50/2000 等幅，高通后高频占比应显著更高
        assert!(e2000 > e50 * 2.0, "高频能量应显著高于低频: e50={e50} e2000={e2000}");
    }

    #[test]
    fn noise_gate_silences_quiet_blocks() {
        let mut p = Preprocessor::new(true, false, 100.0, 0.01);
        let quiet: Vec<f32> = vec![0.001; 1600]; // RMS 0.001 < 0.01
        let mut q = quiet.clone();
        p.process(&mut q);
        assert!(q.iter().all(|&s| s == 0.0), "低电平块应被静音");

        let loud: Vec<f32> = vec![0.1; 1600]; // RMS 0.1 > 0.01
        let mut l = loud.clone();
        let mut p2 = Preprocessor::new(true, false, 100.0, 0.01);
        p2.process(&mut l);
        assert!(l.iter().any(|&s| s != 0.0), "高电平块应保留");
    }

    #[test]
    fn preprocessor_disabled_passthrough() {
        let mut p = Preprocessor::new(false, true, 100.0, 0.01);
        let src: Vec<f32> = vec![0.5, -0.5, 0.25];
        let mut out = src.clone();
        p.process(&mut out);
        assert_eq!(out, src, "关闭时不应改动");
    }
}
