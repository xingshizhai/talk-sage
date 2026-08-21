//! 流式 ASR 端点检测状态机。
//!
//! VAD 负责判断“是否有人声”，本模块负责判断“当前语义段是否可以提交”。
//! 提交必须满足最短句长，并满足文本稳定静音或强制静音中的一个条件。

use talksage_config::EndpointConfig;
use talksage_core::AudioClock;

/// Whisper Flow 文本稳定思想的低开销版本：不重跑音频窗口，只观察原生流式
/// hypothesis，并要求同时出现短暂停顿后才提交。
pub(super) struct StableEndpoint {
    config: EndpointConfig,
    stable_samples: u64,
    quiet_samples: u64,
}

impl StableEndpoint {
    pub(super) fn new(config: EndpointConfig) -> Self {
        Self {
            config,
            stable_samples: 0,
            quiet_samples: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        self.stable_samples = 0;
        self.quiet_samples = 0;
    }

    pub(super) fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub(super) fn max_wait_ms(&self) -> u64 {
        self.config.stable_ms
    }

    pub(super) fn observe(
        &mut self,
        unchanged_nonempty: bool,
        rms: f32,
        chunk_samples: u64,
        segment_samples: u64,
    ) -> bool {
        if !self.config.enabled {
            return false;
        }
        if unchanged_nonempty {
            self.stable_samples += chunk_samples;
        } else {
            self.stable_samples = 0;
        }
        if rms <= self.config.quiet_rms {
            self.quiet_samples += chunk_samples;
        } else {
            self.quiet_samples = 0;
        }

        let ms =
            |samples: u64| AudioClock::samples_to_ms(talksage_audio::TARGET_SAMPLE_RATE, samples);
        let long_enough = ms(segment_samples) >= self.config.min_segment_ms;
        let stable_pause = ms(self.stable_samples) >= self.config.stable_ms
            && ms(self.quiet_samples) >= self.config.quiet_ms;
        let forced_pause = ms(self.quiet_samples) >= self.config.force_quiet_ms;
        long_enough && (stable_pause || forced_pause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_both_text_stability_and_quiet_audio() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 300,
            quiet_ms: 200,
            min_segment_ms: 1000,
            ..EndpointConfig::default()
        });
        let chunk = 1600; // 100ms
        for _ in 0..3 {
            assert!(!endpoint.observe(true, 0.03, chunk, 16000));
        }
        assert!(!endpoint.observe(true, 0.001, chunk, 17600));
        assert!(endpoint.observe(true, 0.001, chunk, 19200));
    }

    #[test]
    fn changed_hypothesis_resets_stability() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 200,
            quiet_ms: 200,
            min_segment_ms: 0,
            ..EndpointConfig::default()
        });
        assert!(!endpoint.observe(true, 0.001, 1600, 1600));
        assert!(!endpoint.observe(false, 0.001, 1600, 3200));
        assert!(!endpoint.observe(true, 0.001, 1600, 4800));
        assert!(endpoint.observe(true, 0.001, 1600, 6400));
    }

    #[test]
    fn long_quiet_pause_commits_even_while_hypothesis_changes() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 400,
            quiet_ms: 400,
            force_quiet_ms: 700,
            min_segment_ms: 1000,
            ..EndpointConfig::default()
        });
        let chunk = 1600; // 100ms
        for index in 0..6 {
            assert!(!endpoint.observe(index % 2 == 0, 0.001, chunk, 16000 + index * chunk));
        }
        assert!(endpoint.observe(false, 0.001, chunk, 25600));
    }

    #[test]
    fn natural_endpoint_respects_minimum_segment_duration() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            stable_ms: 200,
            quiet_ms: 200,
            force_quiet_ms: 300,
            min_segment_ms: 1000,
            ..EndpointConfig::default()
        });
        for _ in 0..5 {
            assert!(!endpoint.observe(true, 0.001, 1600, 8000));
        }
        assert!(endpoint.observe(true, 0.001, 1600, 16000));
    }

    #[test]
    fn disabled_endpoint_never_commits() {
        let mut endpoint = StableEndpoint::new(EndpointConfig {
            enabled: false,
            ..EndpointConfig::default()
        });
        assert!(!endpoint.observe(true, 0.0, 16000, 16000));
    }
}
