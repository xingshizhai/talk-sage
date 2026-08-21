//! 实时段内换人检测。声纹提取由 `SpeakerIdentifier` 完成；本模块只管理
//! 滑动窗口节拍、稳定锚点和连续低相似度确认，因此可不用模型确定性测试。

use std::sync::mpsc;
use std::thread::JoinHandle;

use crate::speaker::SharedSpeaker;

const SAMPLE_RATE: usize = 16_000;
// 1.5s 窗口允许自然语音中夹少量静音后，仍满足 SpeakerIdentifier 的 1s
// 有效人声质量门；用恰好 1s 会要求窗口几乎没有任何低能量帧。
pub(super) const WINDOW_SAMPLES: usize = SAMPLE_RATE * 3 / 2;
const STEP_SAMPLES: usize = SAMPLE_RATE / 2;
const MIN_TURN_SAMPLES: usize = SAMPLE_RATE * 2;
const CHANGE_THRESHOLD: f32 = 0.38;
const REQUIRED_MISMATCHES: u8 = 2;

struct ChangeJob {
    generation: u64,
    total_samples: usize,
    audio: Vec<f32>,
}

struct ChangeResult {
    generation: u64,
    changed: bool,
}

/// 每条启用声纹的流一个 worker。输入队列容量为 1：推理慢时保留正在算的窗口，
/// 新窗口直接跳过，不允许声纹辅助任务给实时 ASR 制造无界积压。
pub(super) struct SpeakerChangeWorker {
    tx: Option<mpsc::SyncSender<ChangeJob>>,
    rx: mpsc::Receiver<ChangeResult>,
    join: Option<JoinHandle<()>>,
    next_submit: usize,
    generation: u64,
}

impl SpeakerChangeWorker {
    pub(super) fn start(speaker: SharedSpeaker) -> Option<Self> {
        let (tx_job, rx_job) = mpsc::sync_channel::<ChangeJob>(1);
        let (tx_result, rx_result) = mpsc::channel::<ChangeResult>();
        let join = std::thread::Builder::new()
            .name("speaker-change".into())
            .spawn(move || run_worker(speaker, rx_job, tx_result))
            .ok()?;
        Some(Self {
            tx: Some(tx_job),
            rx: rx_result,
            join: Some(join),
            next_submit: WINDOW_SAMPLES,
            generation: 0,
        })
    }

    /// 到节拍时复制最新的 1.5s 窗口。队列忙则跳过；下一次从当前时刻继续，
    /// 不追赶历史任务。
    pub(super) fn submit_if_due(&mut self, segment_audio: &[f32]) {
        if segment_audio.len() < self.next_submit {
            return;
        }
        self.next_submit = segment_audio.len().saturating_add(STEP_SAMPLES);
        let start = segment_audio.len().saturating_sub(WINDOW_SAMPLES);
        let job = ChangeJob {
            generation: self.generation,
            total_samples: segment_audio.len(),
            audio: segment_audio[start..].to_vec(),
        };
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(job);
        }
    }

    /// 丢弃旧 generation 的迟到结果；任一当前结果确认换人即返回 true。
    pub(super) fn poll_changed(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.rx.try_recv() {
            if result_is_current(self.generation, &result) {
                changed |= result.changed;
            }
        }
        changed
    }

    pub(super) fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.next_submit = WINDOW_SAMPLES;
        while self.rx.try_recv().is_ok() {}
    }
}

fn result_is_current(generation: u64, result: &ChangeResult) -> bool {
    result.generation == generation
}

impl Drop for SpeakerChangeWorker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_worker(
    speaker: SharedSpeaker,
    rx: mpsc::Receiver<ChangeJob>,
    tx: mpsc::Sender<ChangeResult>,
) {
    let mut detector = SpeakerChangeDetector::default();
    let mut generation = None;
    while let Ok(job) = rx.recv() {
        if generation != Some(job.generation) {
            detector.reset();
            generation = Some(job.generation);
        }
        let changed = speaker
            .compute_embedding(&job.audio)
            .is_some_and(|embedding| detector.observe(job.total_samples, embedding));
        if tx.send(ChangeResult { generation: job.generation, changed }).is_err() {
            return;
        }
    }
}

pub(super) struct SpeakerChangeDetector {
    anchor: Option<Vec<f32>>,
    mismatches: u8,
}

impl Default for SpeakerChangeDetector {
    fn default() -> Self {
        Self { anchor: None, mismatches: 0 }
    }
}

impl SpeakerChangeDetector {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// 连续两个窗口明显偏离锚点才报告换人；单个异常窗口按抖动处理。
    pub(super) fn observe(&mut self, total_samples: usize, embedding: Vec<f32>) -> bool {
        let Some(embedding) = normalize(embedding) else {
            return false;
        };
        let Some(anchor) = &self.anchor else {
            self.anchor = Some(embedding);
            return false;
        };
        if cosine_similarity(anchor, &embedding) >= CHANGE_THRESHOLD {
            self.mismatches = 0;
            self.anchor = normalized_blend(anchor, &embedding, 0.1);
            return false;
        }
        if total_samples < MIN_TURN_SAMPLES {
            return false;
        }
        self.mismatches = self.mismatches.saturating_add(1);
        self.mismatches >= REQUIRED_MISMATCHES
    }
}

fn normalize(mut embedding: Vec<f32>) -> Option<Vec<f32>> {
    let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON {
        return None;
    }
    for value in &mut embedding {
        *value /= norm;
    }
    Some(embedding)
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return -1.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalized_blend(a: &[f32], b: &[f32], weight: f32) -> Option<Vec<f32>> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut blended = a.iter().zip(b)
        .map(|(x, y)| x * (1.0 - weight) + y * weight)
        .collect::<Vec<_>>();
    let norm = blended.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON {
        return None;
    }
    for value in &mut blended {
        *value /= norm;
    }
    Some(blended)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_two_consecutive_mismatches_after_minimum_turn() {
        let mut detector = SpeakerChangeDetector::default();
        assert!(!detector.observe(WINDOW_SAMPLES, vec![1.0, 0.0]));
        assert!(!detector.observe(MIN_TURN_SAMPLES, vec![0.0, 1.0]));
        assert!(detector.observe(MIN_TURN_SAMPLES + STEP_SAMPLES, vec![0.0, 1.0]));
    }

    #[test]
    fn matching_window_clears_a_pending_change() {
        let mut detector = SpeakerChangeDetector::default();
        assert!(!detector.observe(WINDOW_SAMPLES, vec![1.0, 0.0]));
        assert!(!detector.observe(MIN_TURN_SAMPLES, vec![0.0, 1.0]));
        assert!(!detector.observe(MIN_TURN_SAMPLES + STEP_SAMPLES, vec![1.0, 0.0]));
        assert!(!detector.observe(MIN_TURN_SAMPLES + STEP_SAMPLES * 2, vec![0.0, 1.0]));
    }

    #[test]
    fn normalization_makes_similarity_scale_independent() {
        let mut detector = SpeakerChangeDetector::default();
        assert!(!detector.observe(WINDOW_SAMPLES, vec![10.0, 0.0]));
        assert!(!detector.observe(MIN_TURN_SAMPLES, vec![0.1, 0.0]));
    }

    #[test]
    fn stale_generation_result_is_rejected() {
        let stale = ChangeResult { generation: 4, changed: true };
        assert!(!result_is_current(5, &stale));
        let current = ChangeResult { generation: 5, changed: true };
        assert!(result_is_current(5, &current));
    }
}
