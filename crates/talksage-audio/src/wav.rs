//! 简单 WAV 读写（16-bit PCM，任意采样率；录音输出固定 16kHz mono）。
//!
//! 用于：监听时录音保存（WavRecorder）+ 静音裁剪工具（read_wav）。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result};

/// 写入 RIFF 头（44 字节 PCM16）后的数据偏移。
const HEADER_LEN: u64 = 44;

/// 录音器：边写边维护 data 大小，`finish()` 时回填 RIFF 头。
pub struct WavRecorder {
    file: File,
    sample_rate: u32,
    channels: u16,
    data_bytes: u64,
    samples_written: u64,
}

impl WavRecorder {
    /// 创建录音文件（PCM16 mono，任意采样率）。已存在的文件会被覆盖。
    pub fn create(path: &Path, sample_rate: u32) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建录音目录失败: {}", parent.display()))?;
        }
        let mut file = File::create(path)
            .with_context(|| format!("创建录音文件失败: {}", path.display()))?;
        // 占位头（finish 时回填）
        file.write_all(&[0u8; HEADER_LEN as usize])?;
        file.flush()?;
        Ok(Self {
            file,
            sample_rate,
            channels: 1,
            data_bytes: 0,
            samples_written: 0,
        })
    }

    /// 写入一块 f32 采样（归一化到 [-1, 1]），转为 i16 PCM。
    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        let mut buf = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            buf.extend_from_slice(&v.to_le_bytes());
        }
        self.file.write_all(&buf)?;
        self.data_bytes += buf.len() as u64;
        self.samples_written += samples.len() as u64;
        Ok(())
    }

    /// 已写入的采样数。
    pub fn samples_written(&self) -> u64 {
        self.samples_written
    }

    /// 完成：回填 RIFF/RIFF 大小/fmt/data 头后关闭文件。
    pub fn finish(mut self) -> Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(b"RIFF")?;
        // RIFF chunk size（u32，data_bytes 超出则截断告警）
        let riff_size = 36u32.checked_add(self.data_bytes as u32).unwrap_or(u32::MAX);
        self.file.write_all(&riff_size.to_le_bytes())?;
        self.file.write_all(b"WAVE")?;
        self.file.write_all(b"fmt ")?;
        self.file.write_all(&16u32.to_le_bytes())?; // fmt chunk size
        self.file.write_all(&1u16.to_le_bytes())?; // PCM
        self.file.write_all(&self.channels.to_le_bytes())?;
        self.file.write_all(&self.sample_rate.to_le_bytes())?;
        let byte_rate = self.sample_rate * self.channels as u32 * 2;
        self.file.write_all(&byte_rate.to_le_bytes())?;
        let block_align = self.channels * 2;
        self.file.write_all(&block_align.to_le_bytes())?;
        self.file.write_all(&16u16.to_le_bytes())?; // bits per sample
        self.file.write_all(b"data")?;
        self.file.write_all(&(self.data_bytes as u32).to_le_bytes())?;
        self.file.flush()?;
        Ok(())
    }
}

/// 读取 PCM16 WAV：返回 (采样率, mono f32 采样)。
/// 立体声按平均混为 mono。
pub fn read_wav(path: &Path) -> Result<(u32, Vec<f32>)> {
    let mut f = File::open(path).with_context(|| format!("打开 wav 失败: {}", path.display()))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if buf.len() < HEADER_LEN as usize || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        anyhow::bail!("不是有效的 RIFF/WAVE 文件: {}", path.display());
    }
    // 扫描 chunk：fmt + data
    let mut pos = 12usize;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data: Vec<u8> = Vec::new();
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let size = u32::from_le_bytes(buf[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if body + size > buf.len() {
            break;
        }
        match id {
            b"fmt " => {
                if size >= 16 {
                    let format = u16::from_le_bytes(buf[body..body + 2].try_into().unwrap());
                    if format != 1 {
                        anyhow::bail!("仅支持 PCM 格式（format={format}）");
                    }
                    channels = u16::from_le_bytes(buf[body + 2..body + 4].try_into().unwrap());
                    sample_rate = u32::from_le_bytes(buf[body + 4..body + 8].try_into().unwrap());
                    bits = u16::from_le_bytes(buf[body + 14..body + 16].try_into().unwrap());
                }
            }
            b"data" => {
                data.extend_from_slice(&buf[body..body + size]);
            }
            _ => {}
        }
        pos = body + size + (size % 2); // chunk 对齐（奇数长度补 1 字节）
    }
    if sample_rate == 0 || bits != 16 {
        anyhow::bail!("wav 缺少 fmt 信息或非 16-bit: {}（sr={sample_rate} bits={bits}）", path.display());
    }

    // PCM16 → f32 mono
    let frames = data.len() / 2;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let sample = i16::from_le_bytes([data[i * 2], data[i * 2 + 1]]) as f32 / 32768.0;
        out.push(sample);
    }
    if channels > 1 {
        // 帧交错：取每帧多声道平均
        let frames_n = out.len() / channels as usize;
        let mut mono = Vec::with_capacity(frames_n);
        for fr in 0..frames_n {
            let mut acc = 0.0f32;
            for ch in 0..channels as usize {
                acc += out[fr * channels as usize + ch];
            }
            mono.push(acc / channels as f32);
        }
        out = mono;
    }
    Ok((sample_rate, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_roundtrip_preserves_samples() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.wav");
        let mut rec = WavRecorder::create(&path, 16000).unwrap();
        let src: Vec<f32> = (0..1600)
            .map(|i| ((i as f32 / 1600.0) * std::f32::consts::TAU).sin() * 0.8)
            .collect();
        rec.write(&src).unwrap();
        assert_eq!(rec.samples_written(), 1600);
        rec.finish().unwrap();

        let (sr, samples) = read_wav(&path).unwrap();
        assert_eq!(sr, 16000);
        assert_eq!(samples.len(), 1600);
        for (a, b) in samples.iter().zip(src.iter()) {
            assert!((a - b).abs() < 2.0 / 32768.0, "样本误差过大: {a} vs {b}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recorder_writes_valid_header_sizes() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("header.wav");
        let mut rec = WavRecorder::create(&path, 8000).unwrap();
        rec.write(&vec![0.0; 100]).unwrap();
        rec.finish().unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(&raw[0..4], b"RIFF");
        assert_eq!(&raw[8..12], b"WAVE");
        // data size = 100 samples * 2 bytes
        let data_size = u32::from_le_bytes(raw[40..44].try_into().unwrap());
        assert_eq!(data_size, 200);
        // 文件总长 = 44 + 200
        assert_eq!(raw.len(), 244);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_wav_stereo_mixes_to_mono() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-test3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 手工构造 16-bit stereo PCM（左=1.0 满幅，右=0.5）
        let path = dir.join("stereo.wav");
        let mut raw = Vec::new();
        raw.extend_from_slice(b"RIFF");
        let data_bytes = 2u32 * 2 * 2; // 2 帧 × 2 声道 × 2 字节
        raw.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        raw.extend_from_slice(b"WAVEfmt ");
        raw.extend_from_slice(&16u32.to_le_bytes());
        raw.extend_from_slice(&1u16.to_le_bytes());
        raw.extend_from_slice(&2u16.to_le_bytes());
        raw.extend_from_slice(&16000u32.to_le_bytes());
        raw.extend_from_slice(&(16000u32 * 2 * 2).to_le_bytes());
        raw.extend_from_slice(&4u16.to_le_bytes());
        raw.extend_from_slice(&16u16.to_le_bytes());
        raw.extend_from_slice(b"data");
        raw.extend_from_slice(&data_bytes.to_le_bytes());
        raw.extend_from_slice(&32767i16.to_le_bytes());
        raw.extend_from_slice(&16384i16.to_le_bytes());
        raw.extend_from_slice(&(-32767i16).to_le_bytes());
        raw.extend_from_slice(&(-16384i16).to_le_bytes());
        std::fs::write(&path, &raw).unwrap();

        let (sr, samples) = read_wav(&path).unwrap();
        assert_eq!(sr, 16000);
        assert_eq!(samples.len(), 2);
        let expect0 = (32767.0 + 16384.0) / 2.0 / 32768.0;
        let expect1 = (-32767.0 - 16384.0) / 2.0 / 32768.0;
        assert!((samples[0] - expect0).abs() < 1e-4);
        assert!((samples[1] - expect1).abs() < 1e-4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
