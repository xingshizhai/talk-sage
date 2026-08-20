//! 会话运行时：包装 `LivePipeline`，维护 committed / hypothesis / revision。
//!
//! 适配器不得直接 `LivePipeline::new`；import / bench / listen 经此边界。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use talksage_core::{DomainEvent, SessionSnapshot, StatusStage, TranscriptState};

use crate::{EventSink, LivePipeline, LivePipelineConfig};

/// 统一运行边界：内部仍是现有 `LivePipeline` 线程模型。
pub struct SessionRuntime {
    pipeline: LivePipeline,
    transcript: Arc<Mutex<TranscriptState>>,
    stage: Arc<Mutex<StatusStage>>,
}

impl SessionRuntime {
    pub fn new(cfg: LivePipelineConfig) -> Self {
        Self {
            pipeline: LivePipeline::new(cfg),
            transcript: Arc::new(Mutex::new(TranscriptState::default())),
            stage: Arc::new(Mutex::new(StatusStage::Starting)),
        }
    }

    pub fn start(&mut self, emit: EventSink) -> Result<()> {
        let transcript = self.transcript.clone();
        let stage = self.stage.clone();
        let wrap: EventSink = Arc::new(move |ev: DomainEvent| {
            if let DomainEvent::Status { stage: s, .. } = &ev {
                *stage.lock().unwrap() = *s;
            }
            let stamped = if matches!(&ev, DomainEvent::Segment { .. }) {
                let mut t = transcript.lock().unwrap();
                t.apply(&ev);
                stamp_revision(ev, t.revision)
            } else {
                ev
            };
            emit(stamped);
        });
        self.pipeline.start(wrap)
    }

    pub fn stop(&mut self) {
        let _ = self.stop_with_timeout(crate::STOP_JOIN_TIMEOUT);
    }

    /// 停止管道并在时限内 join。超时返回 `false`。
    pub fn stop_with_timeout(&mut self, timeout: Duration) -> bool {
        self.pipeline.stop_with_timeout(timeout)
    }

    pub fn set_noise_level(&self, level: f32) {
        self.pipeline.set_noise_level(level);
    }

    pub fn noise_level(&self) -> f32 {
        self.pipeline.noise_level()
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let stage = *self.stage.lock().unwrap();
        self.transcript.lock().unwrap().snapshot(stage)
    }
}

fn stamp_revision(ev: DomainEvent, revision: u64) -> DomainEvent {
    match ev {
        DomainEvent::Segment {
            speaker_id,
            speaker_label,
            text,
            is_partial,
            ts_ms,
            duration_ms,
            rms,
            start_sample,
            end_sample,
            ..
        } => DomainEvent::Segment {
            speaker_id,
            speaker_label,
            text,
            is_partial,
            ts_ms,
            duration_ms,
            rms,
            revision,
            start_sample,
            end_sample,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::stamp_revision;
    use talksage_core::DomainEvent;

    #[test]
    fn stamps_revision_on_segment() {
        let ev = DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            text: "hi".into(),
            is_partial: false,
            ts_ms: 10,
            duration_ms: 400,
            rms: 0.1,
            revision: 0,
            start_sample: 0,
            end_sample: 6400,
        };
        let stamped = stamp_revision(ev, 7);
        match stamped {
            DomainEvent::Segment { revision, .. } => assert_eq!(revision, 7),
            other => panic!("{other:?}"),
        }
    }
}
