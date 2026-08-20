//! WASAPI 系统回环采集（Windows loopback）。
//!
//! 捕获系统扬声器输出（视频会议中客户声音的来源），重采样到 16kHz mono
//! 并按块通过 mpsc 通道发送，接口与 `AudioHub`（麦克风）对齐。
//!
//! 参考 Meetily `audio/capture/system.rs` 的 WASAPI loopback 做法。

#![cfg(windows)]

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use windows::Win32::Media::Audio::{
    eConsole, eRender, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient,
    IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

use crate::{CaptureTx, LinearResampler};

/// WASAPI 回环采集器。
pub struct LoopbackCapture {
    chunk_samples: usize,
    tx: CaptureTx,
    thread: Option<JoinHandle<()>>,
    tx_stop: Option<mpsc::Sender<()>>,
}

impl LoopbackCapture {
    /// 创建回环采集器（Windows 专属；非 Windows 平台 start 会报错）。
    pub fn new(chunk_ms: u64) -> (Self, mpsc::Receiver<Vec<f32>>) {
        let (tx, rx) = crate::capture_channel();
        let chunk_samples = (crate::TARGET_SAMPLE_RATE as u64 * chunk_ms / 1000) as usize;
        (
            Self {
                chunk_samples,
                tx,
                thread: None,
                tx_stop: None,
            },
            rx,
        )
    }

    /// 启动采集线程（默认渲染设备）。
    pub fn start(&mut self) -> Result<()> {
        if self.thread.is_some() {
            return Ok(());
        }
        let (tx_stop, rx_stop) = mpsc::channel::<()>();
        let tx = self.tx.clone();
        let chunk_samples = self.chunk_samples;
        let handle = std::thread::Builder::new()
            .name("talksage-loopback".into())
            .spawn(move || {
                if let Err(e) = loopback_loop(rx_stop, tx, chunk_samples) {
                    log::error!("回环采集线程退出异常: {e}");
                }
            })?;
        self.thread = Some(handle);
        self.tx_stop = Some(tx_stop);
        Ok(())
    }

    /// 采集 overrun 累计（队列满丢帧次数）。
    pub fn overruns(&self) -> u64 {
        self.tx.overruns()
    }

    /// 停止采集。
    pub fn stop(&mut self) {
        if let Some(tx) = self.tx_stop.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

/// 解析 WAVEFORMATEX：返回 (采样率, 声道数, 每样本字节, 是否 float32)。
fn parse_format(format: &WAVEFORMATEX) -> (u32, u16, u16, bool) {
    let sample_rate = format.nSamplesPerSec;
    let channels = format.nChannels;
    let bits = format.wBitsPerSample;
    let mut is_float = false;
    let tag = format.wFormatTag as u32;
    if tag == WAVE_FORMAT_IEEE_FLOAT {
        is_float = true;
    } else if tag == WAVE_FORMAT_EXTENSIBLE {
        // WAVEFORMATEXTENSIBLE：SubFormat 决定数据类型（packed 结构，需未对齐读取）
        unsafe {
            let ext = &*(format as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE);
            let sub_format = std::ptr::read_unaligned(std::ptr::addr_of!(ext.SubFormat));
            if sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                is_float = true;
            }
        }
    }
    let bytes_per_sample = (bits / 8) as u16;
    (sample_rate, channels, bytes_per_sample, is_float)
}

/// 采集主循环（专用线程内）。
fn loopback_loop(
    rx_stop: mpsc::Receiver<()>,
    tx: CaptureTx,
    chunk_samples: usize,
) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }

    // 默认渲染设备（扬声器/耳机输出 = 回环源）
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }?;
    let audio_client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };

    // 混合格式（GetMixFormat 返回堆上分配的 WAVEFORMATEX 指针）
    let mix_format_ptr = unsafe { audio_client.GetMixFormat() }?;
    let mix_format = unsafe { &*mix_format_ptr };
    let (src_sr, channels, bytes_per_sample, is_float) = parse_format(mix_format);
    log::info!(
        "回环设备: {}Hz {}ch {}bit float={is_float}",
        src_sr,
        channels,
        bytes_per_sample * 8
    );

    // 初始化共享模式 loopback
    let buffer_duration = 200_000i64; // 100ns 单位 → 200ms
    unsafe {
        audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            buffer_duration,
            0,
            mix_format_ptr,
            None,
        )?;
    }

    let capture: IAudioCaptureClient = unsafe { audio_client.GetService() }?;
    unsafe { audio_client.Start()? };

    let mut resampler = LinearResampler::new(src_sr, crate::TARGET_SAMPLE_RATE);
    let mut pending: Vec<f32> = Vec::with_capacity(chunk_samples);

    'outer: loop {
        // 停止信号
        match rx_stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break 'outer,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        let packet_size: u32 = unsafe { capture.GetNextPacketSize()? };
        if packet_size == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        let mut data_ptr: *mut u8 = std::ptr::null_mut();
        let mut frames: u32 = 0;
        let mut flags: u32 = 0;
        let mut device_position: u64 = 0;
        let mut qpc_position: u64 = 0;
        unsafe {
            capture.GetBuffer(
                &mut data_ptr,
                &mut frames,
                &mut flags,
                Some(&mut device_position as *mut u64),
                Some(&mut qpc_position as *mut u64),
            )?;
        }
        if frames > 0 && !data_ptr.is_null() {
            let total = frames as usize * channels as usize;
            // 按格式转换为 f32
            let mut mono: Vec<f32> = Vec::with_capacity(frames as usize);
            if is_float {
                let samples = unsafe { std::slice::from_raw_parts(data_ptr as *const f32, total) };
                if channels > 1 {
                    for c in samples.chunks(channels as usize) {
                        mono.push(c.iter().sum::<f32>() / c.len() as f32);
                    }
                } else {
                    mono.extend_from_slice(samples);
                }
            } else if bytes_per_sample == 2 {
                let samples = unsafe { std::slice::from_raw_parts(data_ptr as *const i16, total) };
                for c in samples.chunks(channels as usize) {
                    mono.push(c.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / c.len() as f32);
                }
            } else if bytes_per_sample == 4 {
                let samples = unsafe { std::slice::from_raw_parts(data_ptr as *const i32, total) };
                for c in samples.chunks(channels as usize) {
                    mono.push(c.iter().map(|&s| s as f32 / 2147483648.0).sum::<f32>() / c.len() as f32);
                }
            } else {
                // 8bit PCM
                let samples = unsafe { std::slice::from_raw_parts(data_ptr, total) };
                for c in samples.chunks(channels as usize) {
                    mono.push(c.iter().map(|&s| (s as f32 - 128.0) / 128.0).sum::<f32>() / c.len() as f32);
                }
            }
            unsafe { capture.ReleaseBuffer(frames) }?;

            // 重采样 + 分块发送
            let resampled = resampler.process(&mono);
            pending.extend_from_slice(&resampled);
            let mut i = 0;
            while pending.len() - i >= chunk_samples {
                let chunk: Vec<f32> = pending[i..i + chunk_samples].to_vec();
                i += chunk_samples;
                if !tx.try_push(chunk) && tx.closed() {
                    break 'outer;
                }
            }
            if i > 0 {
                pending.drain(..i);
            }
        } else {
            unsafe { capture.ReleaseBuffer(frames) }?;
        }
    }

    unsafe {
        let _ = audio_client.Stop();
        CoUninitialize();
    }
    Ok(())
}
