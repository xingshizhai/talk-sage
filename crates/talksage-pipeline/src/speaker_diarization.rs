//! 会后离线说话人分离。输出纯时间轴，不修改实时转写；调用方可用重叠率将
//! 转写段映射到稳定 speaker id，从而避免为了精确边界拖慢实时字幕。

use std::path::Path;

use anyhow::{Context, Result};
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractorConfig,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiarizationSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker_id: u32,
}

pub fn diarize_wav(
    wav: &Path,
    segmentation_model: &Path,
    embedding_model: &Path,
    num_speakers: Option<u32>,
) -> Result<Vec<DiarizationSegment>> {
    let (sample_rate, samples) = talksage_audio::read_wav(wav)
        .with_context(|| format!("读取待分离音频失败: {}", wav.display()))?;
    let samples = talksage_audio::resample_linear(
        &samples,
        sample_rate,
        talksage_audio::TARGET_SAMPLE_RATE,
    );
    diarize_samples(
        &samples,
        segmentation_model,
        embedding_model,
        num_speakers,
    )
}

pub fn diarize_samples(
    samples: &[f32],
    segmentation_model: &Path,
    embedding_model: &Path,
    num_speakers: Option<u32>,
) -> Result<Vec<DiarizationSegment>> {
    anyhow::ensure!(segmentation_model.is_file(), "缺少说话人分割模型: {}", segmentation_model.display());
    anyhow::ensure!(embedding_model.is_file(), "缺少声纹模型: {}", embedding_model.display());
    anyhow::ensure!(!samples.is_empty(), "音频为空");

    let config = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                model: Some(segmentation_model.to_string_lossy().into_owned()),
            },
            num_threads: 2,
            debug: false,
            provider: Some("cpu".into()),
        },
        embedding: SpeakerEmbeddingExtractorConfig {
            model: Some(embedding_model.to_string_lossy().into_owned()),
            num_threads: 2,
            debug: false,
            provider: Some("cpu".into()),
        },
        clustering: FastClusteringConfig {
            num_clusters: num_speakers.map(|n| n as i32).unwrap_or(-1),
            threshold: 0.5,
        },
        min_duration_on: 0.3,
        min_duration_off: 0.5,
    };
    let diarizer = OfflineSpeakerDiarization::create(&config)
        .ok_or_else(|| anyhow::anyhow!("创建离线说话人分离器失败"))?;
    anyhow::ensure!(diarizer.sample_rate() == 16_000, "分离模型采样率不是 16kHz");
    let result = diarizer.process(samples)
        .ok_or_else(|| anyhow::anyhow!("离线说话人分离失败"))?;
    Ok(result
        .sort_by_start_time()
        .into_iter()
        .filter(|seg| seg.end > seg.start && seg.speaker >= 0)
        .map(|seg| DiarizationSegment {
            start_ms: (seg.start.max(0.0) * 1000.0).round() as u64,
            end_ms: (seg.end.max(0.0) * 1000.0).round() as u64,
            speaker_id: seg.speaker as u32,
        })
        .collect())
}

/// 将一个转写时间区间映射到重叠时长最大的 diarization speaker。
pub fn dominant_speaker(
    start_ms: u64,
    end_ms: u64,
    timeline: &[DiarizationSegment],
) -> Option<u32> {
    let duration = end_ms.saturating_sub(start_ms);
    if duration == 0 {
        return None;
    }
    let mut overlaps = std::collections::HashMap::<u32, u64>::new();
    for seg in timeline {
        let overlap = end_ms.min(seg.end_ms).saturating_sub(start_ms.max(seg.start_ms));
        if overlap > 0 {
            *overlaps.entry(seg.speaker_id).or_default() += overlap;
        }
    }
    let (speaker, overlap) = overlaps.into_iter().max_by_key(|(_, overlap)| *overlap)?;
    // 少量擦边不应覆盖实时标签；至少一半转写区间由该 speaker 覆盖才接受。
    (overlap.saturating_mul(2) >= duration).then_some(speaker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dominant_speaker_uses_largest_overlap() {
        let timeline = vec![
            DiarizationSegment { start_ms: 0, end_ms: 900, speaker_id: 0 },
            DiarizationSegment { start_ms: 900, end_ms: 2_000, speaker_id: 1 },
        ];
        assert_eq!(dominant_speaker(500, 1_500, &timeline), Some(1));
        assert_eq!(dominant_speaker(2_100, 2_500, &timeline), None);
        assert_eq!(dominant_speaker(1_500, 3_500, &timeline), None, "覆盖不足一半应保留实时标签");
    }
}
