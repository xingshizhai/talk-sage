//! 静音裁剪：用 silero VAD 检测语音区间，去掉无声音的部分。
//!
//! 用途：把会议录音压缩成紧凑的测试素材（录制 → 裁剪 → 回放验证闭环）。
//! 与 pipeline 共用同一套 VAD 参数（灵敏度/最小语音/最小静音/窗口）。

use std::path::Path;

use anyhow::{Context, Result};
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};

use talksage_config::VadConfig;

use crate::wav::{read_wav, WavRecorder};
use crate::LinearResampler;

/// 裁剪统计。
#[derive(Debug, Clone)]
pub struct TrimStats {
    /// 输入采样数（16k）。
    pub input_samples: u64,
    /// 输出采样数（16k）。
    pub output_samples: u64,
    /// 输入时长（ms）。
    pub input_ms: u64,
    /// 输出时长（ms）。
    pub output_ms: u64,
    /// 去掉的静音时长（ms）。
    pub removed_ms: u64,
    /// 语音段数量。
    pub speech_segments: usize,
    /// 输出文件路径。
    pub output_path: String,
}

impl TrimStats {
    /// 压缩率（0..1，越大越紧凑）。
    pub fn compression_ratio(&self) -> f32 {
        if self.input_samples == 0 {
            0.0
        } else {
            self.output_samples as f32 / self.input_samples as f32
        }
    }
}

/// 每块时长（ms），与 pipeline 一致。
const CHUNK_MS: u64 = 100;
/// 每段前后额外保留的静音（ms），避免切掉音头/音尾。
const PAD_MS: u64 = 300;

/// 创建 silero VAD（参数与 pipeline 一致）。
fn create_vad(model: &Path, vad: &VadConfig) -> Result<VoiceActivityDetector> {
    let (threshold, min_speech, min_silence, window, max_speech) = vad.effective();
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

/// 静音裁剪：读入 wav（任意采样率，自动重采样到 16k）→ VAD 检测语音区间 →
/// 拼接输出到 `output`（16k mono PCM16）。
///
/// 返回裁剪统计。若输入无语音，输出为空文件并给出提示（调用方自行判断）。
pub fn trim_silence(input: &Path, output: &Path, vad_model: &Path, vad: &VadConfig) -> Result<TrimStats> {
    let (sr, samples) = read_wav(input).with_context(|| format!("读取音频失败: {}", input.display()))?;
    let samples: Vec<f32> = if sr != crate::TARGET_SAMPLE_RATE {
        log::warn!("输入采样率 {sr}Hz ≠ 16kHz，自动重采样");
        LinearResampler::new(sr, crate::TARGET_SAMPLE_RATE).process(&samples)
    } else {
        samples
    };
    let input_samples = samples.len() as u64;

    // 逐块 VAD
    let chunk_size = (crate::TARGET_SAMPLE_RATE as usize) * (CHUNK_MS as usize) / 1000;
    let vad = create_vad(vad_model, vad)?;
    let mut flags: Vec<bool> = Vec::with_capacity(samples.len() / chunk_size + 1);
    for chunk in samples.chunks(chunk_size) {
        vad.accept_waveform(chunk);
        flags.push(vad.detected());
        while !vad.is_empty() {
            vad.pop();
        }
    }

    // 语音区间：膨胀（前后 pad 块）+ 合并
    let pad_blocks = (PAD_MS / CHUNK_MS) as usize;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < flags.len() {
        if !flags[i] {
            i += 1;
            continue;
        }
        let start = i.saturating_sub(pad_blocks);
        let mut j = i;
        while j < flags.len() && flags[j] {
            j += 1;
        }
        let end = (j + pad_blocks).min(flags.len());
        if let Some(last) = ranges.last_mut() {
            if start <= last.1 {
                last.1 = end.max(last.1);
            } else {
                ranges.push((start, end));
            }
        } else {
            ranges.push((start, end));
        }
        i = j;
    }

    // 拼接输出
    let mut out: Vec<f32> = Vec::new();
    for (s, e) in &ranges {
        let from = s * chunk_size;
        let to = (e * chunk_size).min(samples.len());
        if from < to {
            out.extend_from_slice(&samples[from..to]);
        }
    }

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建输出目录失败: {}", parent.display()))?;
    }
    let mut rec = WavRecorder::create(output, crate::TARGET_SAMPLE_RATE)?;
    rec.write(&out)?;
    rec.finish()?;

    let output_samples = out.len() as u64;
    let ms = |n: u64| n * 1000 / crate::TARGET_SAMPLE_RATE as u64;
    Ok(TrimStats {
        input_samples,
        output_samples,
        input_ms: ms(input_samples),
        output_ms: ms(output_samples),
        removed_ms: ms(input_samples.saturating_sub(output_samples)),
        speech_segments: ranges.len(),
        output_path: output.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 探测 VAD 模型路径（TALKSAGE_MODELS_DIR 或仓库根 models/）。
    fn vad_model_path() -> Option<PathBuf> {
        if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
            let p = PathBuf::from(d).join("silero-vad").join("silero_vad.onnx");
            if p.is_file() {
                return Some(p);
            }
        }
        let candidates = [
            PathBuf::from("models/silero-vad/silero_vad.onnx"),
            PathBuf::from("../models/silero-vad/silero_vad.onnx"),
            PathBuf::from("../../models/silero-vad/silero_vad.onnx"),
        ];
        candidates.into_iter().find(|p| p.is_file())
    }

    /// 探测真实语音素材（paraformer 测试音频 0.wav，silero 可稳定识别）。
    fn speech_asset() -> Option<PathBuf> {
        if let Ok(d) = std::env::var("TALKSAGE_MODELS_DIR") {
            let p = PathBuf::from(d).join("sherpa-onnx-streaming-paraformer-zh").join("0.wav");
            if p.is_file() {
                return Some(p);
            }
        }
        let candidates = [
            PathBuf::from("models/sherpa-onnx-streaming-paraformer-zh/0.wav"),
            PathBuf::from("../models/sherpa-onnx-streaming-paraformer-zh/0.wav"),
            PathBuf::from("../../models/sherpa-onnx-streaming-paraformer-zh/0.wav"),
        ];
        candidates.into_iter().find(|p| p.is_file())
    }

    #[test]
    fn trim_removes_silence_with_real_vad() {
        let (Some(model), Some(speech)) = (vad_model_path(), speech_asset()) else {
            eprintln!("skipped: 未找到 silero VAD 模型或语音素材（TALKSAGE_MODELS_DIR 或 models/）");
            return;
        };
        let (sr, speech) = crate::wav::read_wav(&speech).unwrap();
        assert_eq!(sr, crate::TARGET_SAMPLE_RATE);
        let dir = std::env::temp_dir().join(format!("talksage-trim-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.wav");
        let output = dir.join("out.wav");
        {
            let mut rec = WavRecorder::create(&input, 16000).unwrap();
            rec.write(&speech).unwrap(); // 语音 1
            rec.write(&vec![0.0005; 16000 * 2]).unwrap(); // 2s 静音
            rec.write(&speech).unwrap(); // 语音 2
            rec.finish().unwrap();
        }

        let stats = trim_silence(&input, &output, &model, &VadConfig::default()).unwrap();
        let speech_ms = speech.len() as u64 * 1000 / 16000;
        // 输出应显著短于输入（去掉了 2s 静音），且语音部分保留
        assert!(stats.input_ms > speech_ms * 2 + 1500, "输入时长异常: {}ms", stats.input_ms);
        assert!(stats.output_ms < stats.input_ms, "输出应短于输入: {} vs {}", stats.output_ms, stats.input_ms);
        assert!(
            stats.output_ms >= speech_ms + 500,
            "语音部分应保留: 输出 {}ms vs 语音 {}ms",
            stats.output_ms,
            speech_ms
        );
        assert!(stats.compression_ratio() < 0.9, "压缩率应小于 0.9: {}", stats.compression_ratio());
        assert!(stats.speech_segments >= 1);
        assert!(output.is_file());
        assert!(std::fs::metadata(&output).unwrap().len() > 44);

        // 输出文件可读且内容非全静音
        let (_sr, out_samples) = read_wav(&output).unwrap();
        let rms = (out_samples.iter().map(|&s| s * s).sum::<f32>() / out_samples.len() as f32).sqrt();
        assert!(rms > 0.02, "输出应有语音能量: rms={rms}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trim_empty_input_produces_empty_output() {
        let Some(model) = vad_model_path() else {
            eprintln!("skipped: 未找到 silero VAD 模型");
            return;
        };
        let dir = std::env::temp_dir().join(format!("talksage-trim-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("silence.wav");
        let output = dir.join("out.wav");
        {
            let mut rec = WavRecorder::create(&input, 16000).unwrap();
            rec.write(&vec![0.0001; 16000 * 2]).unwrap(); // 2s 纯静音
            rec.finish().unwrap();
        }
        let stats = trim_silence(&input, &output, &model, &VadConfig::default()).unwrap();
        assert_eq!(stats.speech_segments, 0);
        assert_eq!(stats.output_samples, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
