//! 简单 WAV 读写（16-bit PCM，任意采样率；录音输出固定 16kHz mono）。
//!
//! 用于：监听时录音保存（WavRecorder）+ 静音裁剪工具（read_wav）。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result};

/// 写入 RIFF 头（44 字节 PCM16）后的数据偏移。
const HEADER_LEN: u64 = 44;

/// 录音中的临时后缀。见 [`WavRecorder`] 的原子收尾说明。
pub const PART_SUFFIX: &str = ".part";

/// 给录音路径追加 `.part` 后缀。
pub fn part_path_of(path: &Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(PART_SUFFIX);
    std::path::PathBuf::from(s)
}

/// 录音器：边写边维护 data 大小，`finish()` 时回填 RIFF 头。
///
/// **原子收尾**：录音期间写入 `<path>.part`，`finish()` 回填头后原子改名到 `<path>`。
/// 因此「最终路径存在」等价于「文件完整可读」；进程崩溃只会残留 `.part`
/// （头部为占位零），可被启动扫描识别并修复，而不会留下一个看似正常、
/// 实为坏头的 `.wav`。
pub struct WavRecorder {
    file: File,
    /// 最终路径（改名目标）。
    final_path: std::path::PathBuf,
    /// 录音期间的实际写入路径（`final_path` + `.part`）。
    part_path: std::path::PathBuf,
    sample_rate: u32,
    channels: u16,
    data_bytes: u64,
    samples_written: u64,
}

impl WavRecorder {
    /// 创建录音文件（PCM16 mono，任意采样率）。已存在的同名 `.part` 会被覆盖。
    pub fn create(path: &Path, sample_rate: u32) -> Result<Self> {
        Self::create_with_channels(path, sample_rate, 1)
    }

    /// 创建指定声道数的 PCM16 录音器。`write` 接收交错采样。
    pub fn create_with_channels(path: &Path, sample_rate: u32, channels: u16) -> Result<Self> {
        anyhow::ensure!(channels > 0, "WAV 声道数必须大于 0");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建录音目录失败: {}", parent.display()))?;
        }
        let part_path = part_path_of(path);
        let mut file = File::create(&part_path)
            .with_context(|| format!("创建录音文件失败: {}", part_path.display()))?;
        // 占位头（finish 时回填）
        file.write_all(&[0u8; HEADER_LEN as usize])?;
        file.flush()?;
        Ok(Self {
            file,
            final_path: path.to_path_buf(),
            part_path,
            sample_rate,
            channels,
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

    /// 完成：回填 RIFF/fmt/data 头，然后把 `.part` 原子改名到最终路径。
    pub fn finish(mut self) -> Result<()> {
        self.write_header()?;
        // 先落盘再改名：改名成功即代表最终文件内容完整。
        self.file.sync_all().ok();
        std::fs::rename(&self.part_path, &self.final_path).with_context(|| {
            format!("录音改名失败: {} → {}", self.part_path.display(), self.final_path.display())
        })?;
        Ok(())
    }

    fn write_header(&mut self) -> Result<()> {
        write_pcm16_header(&mut self.file, self.sample_rate, self.channels, self.data_bytes)?;
        Ok(())
    }
}

/// 把两条 16k mono 分轨生成双声道主录音：左声道为第一条流，右声道为第二条流。
/// 较短分轨用静音补齐。保留分轨而不做破坏性的单声道叠加，也避免同时讲话削波。
pub fn create_stereo_master(left: &Path, right: &Path, output: &Path) -> Result<()> {
    let (mut left_file, left_sr, left_samples) = open_mono_pcm16_data(left)?;
    let (mut right_file, right_sr, right_samples) = open_mono_pcm16_data(right)?;
    anyhow::ensure!(left_sr == right_sr, "分轨采样率不一致: {left_sr} != {right_sr}");
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = part_path_of(output);
    let result = (|| -> Result<()> {
        let mut out = File::create(&part)?;
        // 先写可识别的 stereo 占位头；若进程崩溃，启动恢复能保留正确声道数。
        write_pcm16_header(&mut out, left_sr, 2, 0)?;
        let total_frames = left_samples.max(right_samples);
        const FRAMES_PER_CHUNK: u64 = 4096;
        let mut frame = 0u64;
        while frame < total_frames {
            let frames = (total_frames - frame).min(FRAMES_PER_CHUNK) as usize;
            let mut left_buf = vec![0u8; frames * 2];
            let mut right_buf = vec![0u8; frames * 2];
            let left_frames = (left_samples.saturating_sub(frame)).min(frames as u64) as usize;
            let right_frames = (right_samples.saturating_sub(frame)).min(frames as u64) as usize;
            left_file.read_exact(&mut left_buf[..left_frames * 2])?;
            right_file.read_exact(&mut right_buf[..right_frames * 2])?;
            let mut stereo = Vec::with_capacity(frames * 4);
            for i in 0..frames {
                stereo.extend_from_slice(&left_buf[i * 2..i * 2 + 2]);
                stereo.extend_from_slice(&right_buf[i * 2..i * 2 + 2]);
            }
            out.write_all(&stereo)?;
            frame += frames as u64;
        }
        write_pcm16_header(&mut out, left_sr, 2, total_frames * 4)?;
        out.sync_all().ok();
        drop(out);
        std::fs::rename(&part, output)?;
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&part).ok();
    }
    result
}

/// 打开本项目录制的固定头 mono PCM16 WAV，并定位到 data 起点。
fn open_mono_pcm16_data(path: &Path) -> Result<(File, u32, u64)> {
    let mut file = File::open(path).with_context(|| format!("打开录音分轨失败: {}", path.display()))?;
    let mut header = [0u8; HEADER_LEN as usize];
    file.read_exact(&mut header)?;
    anyhow::ensure!(&header[0..4] == b"RIFF" && &header[8..12] == b"WAVE", "无效 WAV: {}", path.display());
    let channels = u16::from_le_bytes(header[22..24].try_into().unwrap());
    let sample_rate = u32::from_le_bytes(header[24..28].try_into().unwrap());
    let bits = u16::from_le_bytes(header[34..36].try_into().unwrap());
    anyhow::ensure!(channels == 1 && bits == 16 && &header[36..40] == b"data", "主录音仅支持本项目生成的 mono PCM16 分轨: {}", path.display());
    let samples = u32::from_le_bytes(header[40..44].try_into().unwrap()) as u64 / 2;
    Ok((file, sample_rate, samples))
}

/// 扫描录音目录，把崩溃残留的 `.part` 补头转正。返回恢复出的最终路径。
///
/// 进程异常退出时 [`WavRecorder::finish`] 没跑完，`.part` 的 RIFF 头还是占位零。
/// 这里按实际文件长度算出 data 大小补回头部，再原子改名到最终路径 —— 音频本身
/// 是顺序写入的，除最后一块外都完好，值得抢救。只有头没有数据的直接删除。
pub fn recover_orphan_recordings(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut recovered = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(recovered), // 目录不存在 = 没有录音要恢复
    };
    for entry in entries.flatten() {
        let part = entry.path();
        if part.extension().and_then(|e| e.to_str()) != Some("part") {
            continue;
        }
        match recover_part_file(&part) {
            Ok(Some(final_path)) => recovered.push(final_path),
            Ok(None) => {}
            Err(e) => log::warn!("恢复残留录音失败 {}: {e}", part.display()),
        }
    }
    recovered.sort();
    Ok(recovered)
}

/// 修复单个 `.part`：补 RIFF 头并改名。空录音返回 `Ok(None)` 并删除。
fn recover_part_file(part: &Path) -> Result<Option<std::path::PathBuf>> {
    let final_path = part.with_extension(""); // 去掉 .part
    let len = std::fs::metadata(part)?.len();
    if len <= HEADER_LEN {
        std::fs::remove_file(part).ok();
        log::info!("清理空的残留录音: {}", part.display());
        return Ok(None);
    }
    let data_bytes = len - HEADER_LEN;
    // 普通实时录音头是全零占位；会后 stereo 主录音会预写可识别的双声道头。
    let mut existing = [0u8; HEADER_LEN as usize];
    File::open(part)?.read_exact(&mut existing)?;
    let valid_header = &existing[0..4] == b"RIFF" && &existing[8..12] == b"WAVE";
    let sample_rate = if valid_header {
        u32::from_le_bytes(existing[24..28].try_into().unwrap()).max(1)
    } else {
        crate::TARGET_SAMPLE_RATE
    };
    let channels = if valid_header {
        u16::from_le_bytes(existing[22..24].try_into().unwrap()).max(1)
    } else {
        1
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(part)
        .with_context(|| format!("打开残留录音失败: {}", part.display()))?;
    write_pcm16_header(&mut file, sample_rate, channels, data_bytes)?;
    file.sync_all().ok();
    drop(file);
    std::fs::rename(part, &final_path)
        .with_context(|| format!("残留录音改名失败: {}", part.display()))?;
    log::info!(
        "已恢复残留录音: {}（{} 采样 ≈ {:.1}s）",
        final_path.display(),
        data_bytes / 2 / channels as u64,
        data_bytes as f64 / 2.0 / channels as f64 / sample_rate as f64
    );
    Ok(Some(final_path))
}

/// 写 44 字节 PCM16 RIFF 头到文件开头。
fn write_pcm16_header(file: &mut File, sample_rate: u32, channels: u16, data_bytes: u64) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(b"RIFF")?;
    let riff_size = 36u32.saturating_add(data_bytes as u32);
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVE")?;
    file.write_all(b"fmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?; // PCM
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&(sample_rate * channels as u32 * 2).to_le_bytes())?;
    file.write_all(&(channels * 2).to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?; // bits per sample
    file.write_all(b"data")?;
    file.write_all(&(data_bytes as u32).to_le_bytes())?;
    file.flush()?;
    Ok(())
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
    fn stereo_master_keeps_tracks_and_pads_the_shorter_side() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-master-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let left = dir.join("left.wav");
        let right = dir.join("right.wav");
        let master = dir.join("master.wav");
        let mut l = WavRecorder::create(&left, 16000).unwrap();
        l.write(&[0.2; 4]).unwrap();
        l.finish().unwrap();
        let mut r = WavRecorder::create(&right, 16000).unwrap();
        r.write(&[-0.2; 2]).unwrap();
        r.finish().unwrap();

        create_stereo_master(&left, &right, &master).unwrap();
        let raw = std::fs::read(&master).unwrap();
        assert_eq!(u16::from_le_bytes(raw[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(raw[40..44].try_into().unwrap()), 16); // 4 帧 * 2 声道 * 2 bytes
        let (_, downmixed) = read_wav(&master).unwrap();
        assert_eq!(downmixed.len(), 4);
        assert!(downmixed[0].abs() < 2.0 / 32768.0);
        assert!((downmixed[3] - 0.1).abs() < 2.0 / 32768.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_preserves_a_stereo_part_header() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-stereo-recover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let final_path = dir.join("master.wav");
        let part = part_path_of(&final_path);
        let mut file = File::create(&part).unwrap();
        write_pcm16_header(&mut file, 16000, 2, 0).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        file.write_all(&[0u8; 16]).unwrap();
        drop(file);
        recover_orphan_recordings(&dir).unwrap();
        let raw = std::fs::read(&final_path).unwrap();
        assert_eq!(u16::from_le_bytes(raw[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(raw[40..44].try_into().unwrap()), 16);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 录音期间只应存在 `.part`；`finish()` 后原子改名到最终路径。
    /// 这样「最终路径存在」= 「文件完整」，崩溃残留一眼可辨。
    #[test]
    fn recording_is_atomic_part_then_rename() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("atomic.wav");
        let part = dir.join("atomic.wav.part");

        let mut rec = WavRecorder::create(&path, 16000).unwrap();
        rec.write(&vec![0.1; 320]).unwrap();
        // 收尾前：只有 .part，最终路径还不存在
        assert!(part.is_file(), "录音期间应写入 .part: {}", part.display());
        assert!(!path.exists(), "收尾前不应出现最终文件: {}", path.display());

        rec.finish().unwrap();
        // 收尾后：最终文件可读，.part 已消失
        assert!(path.is_file(), "finish 后应存在最终文件");
        assert!(!part.exists(), "finish 后 .part 应已改名消失");
        let (sr, samples) = read_wav(&path).unwrap();
        assert_eq!(sr, 16000);
        assert_eq!(samples.len(), 320);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 未调用 `finish()` 就 drop（崩溃/异常路径）：残留 `.part`，
    /// 不产生看起来正常、实为坏头的 `.wav`。
    #[test]
    fn dropping_without_finish_leaves_part_not_wav() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aborted.wav");
        let part = dir.join("aborted.wav.part");

        {
            let mut rec = WavRecorder::create(&path, 16000).unwrap();
            rec.write(&vec![0.2; 160]).unwrap();
        } // drop without finish

        assert!(part.is_file(), "异常中断应残留 .part 供恢复");
        assert!(!path.exists(), "异常中断不应产生最终 .wav");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 崩溃残留的 `.part`（头部为占位零）应能按文件长度补回 RIFF 头并转正。
    #[test]
    fn recovers_orphan_part_into_readable_wav() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-recover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("crashed.wav");

        // 模拟崩溃：写了音频但没 finish
        {
            let mut rec = WavRecorder::create(&path, 16000).unwrap();
            rec.write(&vec![0.25; 800]).unwrap();
        }
        assert!(part_path_of(&path).is_file(), "前置条件：应有 .part 残留");

        let recovered = recover_orphan_recordings(&dir).unwrap();
        assert_eq!(recovered.len(), 1, "应恢复 1 个残留录音: {recovered:?}");
        assert_eq!(recovered[0], path);
        assert!(!part_path_of(&path).exists(), "恢复后 .part 应消失");

        let (sr, samples) = read_wav(&path).unwrap();
        assert_eq!(sr, 16000);
        assert_eq!(samples.len(), 800, "恢复后采样数应与崩溃前写入量一致");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 只有头、没有音频数据的 `.part`（刚创建就崩）没有恢复价值，直接删除。
    #[test]
    fn empty_part_is_discarded_not_promoted() {
        let dir = std::env::temp_dir().join(format!("talksage-wav-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.wav");
        drop(WavRecorder::create(&path, 16000).unwrap());

        let recovered = recover_orphan_recordings(&dir).unwrap();
        assert!(recovered.is_empty(), "空录音不应被转正: {recovered:?}");
        assert!(!path.exists(), "空录音不应产生 .wav");
        assert!(!part_path_of(&path).exists(), "空 .part 应被清理");

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
