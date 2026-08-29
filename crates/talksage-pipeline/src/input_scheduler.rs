//! 输入轮询与文件实时节拍。

use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
pub(super) enum AudioPoll {
    Chunk(Vec<f32>),
    Empty,
    Disconnected,
}

pub(super) fn poll_audio(rx: &mpsc::Receiver<Vec<f32>>) -> AudioPoll {
    match rx.try_recv() {
        Ok(chunk) => AudioPoll::Chunk(chunk),
        Err(mpsc::TryRecvError::Empty) => AudioPoll::Empty,
        Err(mpsc::TryRecvError::Disconnected) => AudioPoll::Disconnected,
    }
}

/// 每轮把第一优先级向后移动一格，避免固定偏向 user 流。
#[derive(Default)]
pub(super) struct RoundRobin {
    start: usize,
}

impl RoundRobin {
    pub(super) fn index(&self, offset: usize, worker_count: usize) -> usize {
        (self.start + offset) % worker_count
    }

    pub(super) fn advance(&mut self, worker_count: usize) {
        if worker_count > 0 {
            self.start = (self.start + 1) % worker_count;
        }
    }
}

/// 文件输入按音频块时长逐块放行，但不在 Pipeline 线程内 sleep。
pub(super) struct FilePacer {
    interval: Duration,
    next_at: Instant,
}

impl FilePacer {
    pub(super) fn new(interval: Duration) -> Self {
        Self::new_at(interval, Instant::now())
    }

    fn new_at(interval: Duration, now: Instant) -> Self {
        Self {
            interval,
            next_at: now,
        }
    }

    pub(super) fn due(&self) -> bool {
        self.due_at(Instant::now())
    }

    fn due_at(&self, now: Instant) -> bool {
        now >= self.next_at
    }

    pub(super) fn consumed(&mut self, speed: f32) {
        // “极速”仍保留最小节拍，避免长录音瞬间灌满 ASR 命令队列。
        let effective_speed = if speed <= 0.0 { 8.0 } else { speed };
        self.next_at += self.interval.div_f32(effective_speed);
    }

    /// 暂停期间把 deadline 保持在未来，恢复后不会追赶暂停期间的旧时钟。
    pub(super) fn postpone(&mut self) {
        self.postpone_at(Instant::now());
    }

    fn postpone_at(&mut self, now: Instant) {
        self.next_at = now + self.interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_poll_reports_all_channel_states_without_waiting() {
        let (tx, rx) = mpsc::channel();
        assert_eq!(poll_audio(&rx), AudioPoll::Empty);
        tx.send(vec![0.25, -0.25]).unwrap();
        assert_eq!(poll_audio(&rx), AudioPoll::Chunk(vec![0.25, -0.25]));
        drop(tx);
        assert_eq!(poll_audio(&rx), AudioPoll::Disconnected);
    }

    #[test]
    fn round_robin_rotates_the_first_stream() {
        let mut cursor = RoundRobin::default();
        assert_eq!(
            (0..2).map(|i| cursor.index(i, 2)).collect::<Vec<_>>(),
            [0, 1]
        );
        cursor.advance(2);
        assert_eq!(
            (0..2).map(|i| cursor.index(i, 2)).collect::<Vec<_>>(),
            [1, 0]
        );
    }

    #[test]
    fn file_pacer_postpone_prevents_pause_catch_up() {
        let base = Instant::now();
        let interval = Duration::from_millis(100);
        let mut pacer = FilePacer::new_at(interval, base);
        assert!(pacer.due_at(base));
        pacer.consumed(1.0);
        assert!(!pacer.due_at(base + Duration::from_millis(99)));
        pacer.postpone_at(base + Duration::from_secs(10));
        assert!(!pacer.due_at(base + Duration::from_secs(10)));
        assert!(pacer.due_at(base + Duration::from_millis(10_100)));
    }

    #[test]
    fn file_pacer_scales_interval_with_speed() {
        let base = Instant::now();
        let mut pacer = FilePacer::new_at(Duration::from_millis(100), base);
        pacer.consumed(2.0);
        assert!(!pacer.due_at(base + Duration::from_millis(49)));
        assert!(pacer.due_at(base + Duration::from_millis(50)));
    }
}
