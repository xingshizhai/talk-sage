//! 单条 ASR 句段的文本稳定性与 VAD 收尾状态。

use talksage_config::EndpointConfig;
use talksage_core::AudioClock;

use crate::endpoint::StableEndpoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PartialUpdate {
    Empty,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EndpointDecision {
    pub commit: bool,
    pub natural: bool,
}

pub(super) struct SegmentLifecycle {
    endpoint: StableEndpoint,
    pending_vad_endpoint: bool,
    pending_endpoint_samples: u64,
    last_partial: String,
}

impl SegmentLifecycle {
    pub(super) fn new(config: EndpointConfig) -> Self {
        Self {
            endpoint: StableEndpoint::new(config),
            pending_vad_endpoint: false,
            pending_endpoint_samples: 0,
            last_partial: String::new(),
        }
    }

    pub(super) fn begin(&mut self) {
        self.reset();
    }

    pub(super) fn reset(&mut self) {
        self.last_partial.clear();
        self.endpoint.reset();
        self.pending_vad_endpoint = false;
        self.pending_endpoint_samples = 0;
    }

    pub(super) fn accept_partial(&mut self, text: &str) -> PartialUpdate {
        if text.is_empty() {
            return PartialUpdate::Empty;
        }
        if text == self.last_partial {
            PartialUpdate::Unchanged
        } else {
            self.last_partial.clear();
            self.last_partial.push_str(text);
            PartialUpdate::Changed
        }
    }

    pub(super) fn advance_pending(&mut self, samples: u64) {
        if self.pending_vad_endpoint {
            self.pending_endpoint_samples += samples;
        }
    }

    pub(super) fn observe_endpoint(
        &mut self,
        partial: PartialUpdate,
        rms: f32,
        chunk_samples: u64,
        segment_samples: u64,
    ) -> bool {
        self.endpoint.observe(
            partial == PartialUpdate::Unchanged,
            rms,
            chunk_samples,
            segment_samples,
        )
    }

    pub(super) fn mark_vad_endpoint(&mut self) {
        self.pending_vad_endpoint = true;
        self.pending_endpoint_samples = 0;
    }

    pub(super) fn endpoint_enabled(&self) -> bool {
        self.endpoint.enabled()
    }

    pub(super) fn decide(
        &self,
        is_streaming: bool,
        realtime_input: bool,
        endpoint_ready: bool,
        vad_endpoint: bool,
    ) -> EndpointDecision {
        if !is_streaming || !self.endpoint.enabled() {
            return EndpointDecision {
                commit: vad_endpoint,
                natural: false,
            };
        }
        let natural = realtime_input && endpoint_ready && !self.pending_vad_endpoint;
        let pending_ready = self.pending_vad_endpoint
            && (endpoint_ready
                || AudioClock::samples_to_ms(
                    talksage_audio::TARGET_SAMPLE_RATE,
                    self.pending_endpoint_samples,
                ) >= self.endpoint.max_wait_ms());
        EndpointDecision {
            commit: natural || pending_ready,
            natural,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_update_distinguishes_changed_and_stable_text() {
        let mut segment = SegmentLifecycle::new(EndpointConfig::default());
        assert_eq!(segment.accept_partial(""), PartialUpdate::Empty);
        assert_eq!(segment.accept_partial("你好"), PartialUpdate::Changed);
        assert_eq!(segment.accept_partial("你好"), PartialUpdate::Unchanged);
        assert_eq!(segment.accept_partial("你好啊"), PartialUpdate::Changed);
    }

    #[test]
    fn reset_clears_pending_vad_and_partial_stability() {
        let mut segment = SegmentLifecycle::new(EndpointConfig::default());
        segment.accept_partial("文本");
        segment.mark_vad_endpoint();
        segment.reset();
        assert_eq!(segment.accept_partial("文本"), PartialUpdate::Changed);
        let decision = segment.decide(true, true, true, false);
        assert!(decision.natural);
    }

    #[test]
    fn non_streaming_engine_commits_only_on_vad_endpoint() {
        let segment = SegmentLifecycle::new(EndpointConfig::default());
        assert!(!segment.decide(false, true, true, false).commit);
        assert!(segment.decide(false, true, false, true).commit);
    }
}
