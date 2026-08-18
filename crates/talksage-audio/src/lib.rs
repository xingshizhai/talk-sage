//! TalkSage v2 音频采集中枢。
//!
//! M1：麦克风采集（cpal）→ 单声道 → 重采样到 16kHz → 按固定时长块
//! 通过 mpsc 通道交给 pipeline 线程（回调线程不做重活）。
//!
//! 回环采集（WASAPI loopback / ScreenCaptureKit）为 M1b 预留。

use std::sync::mpsc;
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// 目标采样率（sherpa-onnx 模型统一 16kHz）。
pub const TARGET_SAMPLE_RATE: u32 = 16000;

#[cfg(windows)]
mod loopback;

#[cfg(windows)]
pub use loopback::LoopbackCapture;

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
        if src.is_empty() {
            return Vec::new();
        }
        if self.src_sr == self.dst_sr {
            return src.to_vec();
        }
        let ratio = self.src_sr as f64 / self.dst_sr as f64;
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
}
