//! 单流音频与转录统计。

use talksage_core::AudioClock;

#[derive(Debug, Clone, Copy)]
pub(super) struct StreamStatisticsSnapshot {
    pub total_ms: u64,
    pub speech_ms: u64,
    pub final_segments: usize,
    pub samples: u64,
    pub avg_rms: f32,
    pub max_rms: f32,
    pub non_speech_avg_rms: f32,
    pub words: usize,
    pub questions: usize,
}

#[derive(Default)]
pub(super) struct StreamStatistics {
    total_samples: u64,
    speech_samples: u64,
    energy_sum: f64,
    max_rms: f32,
    non_speech_energy_sum: f64,
    non_speech_samples: u64,
    segment_samples: u64,
    segment_energy_sum: f64,
    final_segments: usize,
    words: usize,
    questions: usize,
}

impl StreamStatistics {
    pub(super) fn observe_block(&mut self, rms: f32, samples: usize, in_speech: bool) {
        let samples = samples as u64;
        let energy = (rms as f64) * (rms as f64) * samples as f64;
        self.total_samples += samples;
        self.energy_sum += energy;
        self.max_rms = self.max_rms.max(rms);
        if !in_speech {
            self.non_speech_samples += samples;
            self.non_speech_energy_sum += energy;
        }
    }

    pub(super) fn start_segment(&mut self) {
        self.segment_samples = 0;
        self.segment_energy_sum = 0.0;
    }

    pub(super) fn observe_speech(&mut self, samples: &[f32]) {
        let count = samples.len() as u64;
        self.segment_samples += count;
        self.speech_samples += count;
        self.segment_energy_sum += samples
            .iter()
            .map(|&sample| (sample as f64) * (sample as f64))
            .sum::<f64>();
    }

    pub(super) fn segment_samples(&self) -> u64 {
        self.segment_samples
    }

    pub(super) fn segment_rms(&self) -> f32 {
        rms(self.segment_energy_sum, self.segment_samples)
    }

    pub(super) fn record_committed_segment(&mut self, text: &str) {
        self.final_segments += 1;
        self.words += talksage_core::metrics::count_words(text);
        if talksage_core::metrics::is_question_text(text) {
            self.questions += 1;
        }
    }

    pub(super) fn snapshot(&self, sample_rate: u32) -> StreamStatisticsSnapshot {
        let avg_rms = rms(self.energy_sum, self.total_samples);
        StreamStatisticsSnapshot {
            total_ms: AudioClock::samples_to_ms(sample_rate, self.total_samples),
            speech_ms: AudioClock::samples_to_ms(sample_rate, self.speech_samples),
            final_segments: self.final_segments,
            samples: self.total_samples,
            avg_rms,
            max_rms: self.max_rms,
            non_speech_avg_rms: if self.non_speech_samples > 0 {
                rms(self.non_speech_energy_sum, self.non_speech_samples)
            } else {
                avg_rms
            },
            words: self.words,
            questions: self.questions,
        }
    }
}

fn rms(energy_sum: f64, samples: u64) -> f32 {
    if samples == 0 {
        0.0
    } else {
        (energy_sum / samples as f64).sqrt() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_uses_sample_count_not_a_fixed_chunk_size() {
        let mut stats = StreamStatistics::default();
        stats.observe_block(0.2, 800, false);
        stats.observe_block(0.4, 2400, false);
        let snapshot = stats.snapshot(16000);
        let expected = ((0.2f32.powi(2) * 800.0 + 0.4f32.powi(2) * 2400.0) / 3200.0).sqrt();
        assert!((snapshot.non_speech_avg_rms - expected).abs() < 1e-6);
        assert!((snapshot.avg_rms - expected).abs() < 1e-6);
    }

    #[test]
    fn segment_rms_is_root_mean_square() {
        let mut stats = StreamStatistics::default();
        stats.start_segment();
        stats.observe_speech(&[0.5, -0.5, 0.5, -0.5]);
        assert!((stats.segment_rms() - 0.5).abs() < 1e-6);
        assert_eq!(stats.segment_samples(), 4);
    }

    #[test]
    fn committed_text_updates_counts_only_once() {
        let mut stats = StreamStatistics::default();
        stats.record_committed_segment("这是问题吗？");
        let snapshot = stats.snapshot(16000);
        assert_eq!(snapshot.final_segments, 1);
        assert_eq!(snapshot.questions, 1);
        assert!(snapshot.words > 0);
    }
}
