//! 本地媒体文件解码：WAV / MP3 / MP4(AAC) → mono f32 PCM。
//!
//! 使用纯 Rust Symphonia，避免桌面应用依赖用户额外安装 ffmpeg。

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_AAC, CODEC_TYPE_MP3};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// 读取导入文件，返回原始采样率和单声道 f32 PCM。
pub fn read_audio_file(path: &Path) -> Result<(u32, Vec<f32>)> {
    let extension = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "wav" => crate::wav::read_wav(path),
        "mp3" | "mp4" | "m4a" => decode_compressed(path, &extension),
        _ => anyhow::bail!("不支持的录音格式: .{extension}（支持 WAV、MP3、MP4/M4A）"),
    }
}

fn decode_compressed(path: &Path, extension: &str) -> Result<(u32, Vec<f32>)> {
    let source =
        File::open(path).with_context(|| format!("打开媒体文件失败: {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| format!("无法识别媒体格式: {}", path.display()))?;
    let mut format = probed.format;
    // 会议 MP4 通常同时有视频轨和音轨，不能盲目使用容器第一轨。
    let track = format
        .tracks()
        .iter()
        .find(|track| matches!(track.codec_params.codec, CODEC_TYPE_AAC | CODEC_TYPE_MP3))
        .ok_or_else(|| anyhow::anyhow!("媒体文件中没有可用音轨: {}", path.display()))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .with_context(|| format!("不支持该音轨编码: {}", path.display()))?;

    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut mono = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::ResetRequired) => {
                anyhow::bail!("媒体文件在中途更换音轨参数，暂不支持: {}", path.display())
            }
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(error) => return Err(error).context("读取媒体音轨失败"),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(error)) => {
                log::warn!("跳过无法解码的音频包: {error}");
                continue;
            }
            Err(error) => return Err(error).context("解码媒体音轨失败"),
        };
        let spec = *decoded.spec();
        if sample_rate == 0 {
            sample_rate = spec.rate;
        }
        anyhow::ensure!(spec.rate == sample_rate, "音轨采样率在中途发生变化");
        append_mono(&mut mono, decoded, spec);
    }
    anyhow::ensure!(
        sample_rate > 0 && !mono.is_empty(),
        "媒体文件中没有解码出音频: {}",
        path.display()
    );
    log::info!(
        "媒体解码完成: {} sr={} samples={}",
        path.display(),
        sample_rate,
        mono.len()
    );
    Ok((sample_rate, mono))
}

fn append_mono(
    output: &mut Vec<f32>,
    decoded: symphonia::core::audio::AudioBufferRef<'_>,
    spec: SignalSpec,
) {
    let channels = spec.channels.count();
    let mut interleaved = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
    interleaved.copy_interleaved_ref(decoded);
    for frame in interleaved.samples().chunks_exact(channels) {
        output.push(frame.iter().copied().sum::<f32>() / channels as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::read_audio_file;
    use std::process::Command;

    /// 本地有 ffmpeg 时生成真实 MP3/MP4 样本，验证容器探测和解码器组合。
    /// CI 未安装 ffmpeg 时跳过，产品运行时不依赖 ffmpeg。
    #[test]
    fn decodes_mp3_and_mp4_aac_when_fixture_encoder_is_available() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("talksage-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for extension in ["mp3", "mp4"] {
            let path = dir.join(format!("tone.{extension}"));
            let mut command = Command::new("ffmpeg");
            command.args(["-v", "error"]);
            if extension == "mp4" {
                // 真实会议视频常见形式：视频轨 + AAC 音轨。
                command.args([
                    "-f",
                    "lavfi",
                    "-i",
                    "color=black:size=32x32:rate=10:duration=0.2",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=0.2",
                    "-shortest",
                    "-c:v",
                    "mpeg4",
                    "-c:a",
                    "aac",
                ]);
            } else {
                command.args(["-f", "lavfi", "-i", "sine=frequency=440:duration=0.2"]);
            }
            let status = command.arg("-y").arg(&path).status().unwrap();
            assert!(status.success(), "生成 {extension} 测试文件失败");
            let (sample_rate, samples) = read_audio_file(&path).unwrap();
            assert!(sample_rate > 0);
            assert!(samples.len() > sample_rate as usize / 10);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_unsupported_extension_with_supported_formats_in_message() {
        let error = read_audio_file(std::path::Path::new("recording.ogg"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("WAV、MP3、MP4/M4A"));
    }
}
