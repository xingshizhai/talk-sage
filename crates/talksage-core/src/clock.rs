//! 音频采样时钟：时间轴跟采样走，墙上时钟只测耗时。

use std::sync::atomic::{AtomicU64, Ordering};

/// 会话内音频时钟（每条流一个；`accepted_samples` 从该流第一块起算）。
///
/// `ts_ms = origin_ms + samples_to_ms(end_sample)`，可追溯到采样点。
pub struct AudioClock {
    sample_rate: u32,
    accepted_samples: AtomicU64,
}

impl AudioClock {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            accepted_samples: AtomicU64::new(0),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 已接受的采样数。
    pub fn accepted(&self) -> u64 {
        self.accepted_samples.load(Ordering::Relaxed)
    }

    /// 累加一块采样，返回这块开始前的采样位置。
    pub fn accept(&self, n: u64) -> u64 {
        self.accepted_samples.fetch_add(n, Ordering::Relaxed)
    }

    /// 已接受音频对应的毫秒（从流起点）。
    pub fn ms(&self) -> u64 {
        Self::samples_to_ms(self.sample_rate, self.accepted())
    }

    pub fn samples_to_ms(sample_rate: u32, samples: u64) -> u64 {
        let sr = sample_rate.max(1) as u64;
        samples.saturating_mul(1000) / sr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_k_one_second() {
        let c = AudioClock::new(16_000);
        assert_eq!(c.accept(16_000), 0);
        assert_eq!(c.accepted(), 16_000);
        assert_eq!(c.ms(), 1_000);
        assert_eq!(AudioClock::samples_to_ms(16_000, 8_000), 500);
    }

    #[test]
    fn timestamps_are_monotonic() {
        let c = AudioClock::new(16_000);
        let a = c.accept(1600);
        let b = c.accept(1600);
        assert!(b > a);
        assert_eq!(c.ms(), 200);
    }
}
