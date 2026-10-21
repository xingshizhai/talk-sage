//! 会话运行时：包装 `LivePipeline`，维护 committed / hypothesis / revision。
//!
//! 适配器不得直接 `LivePipeline::new`；import / bench / listen 经此边界。

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use talksage_core::{DomainEvent, SessionSnapshot, StatusStage, TranscriptState};

use crate::{EventSink, LivePipeline, LivePipelineConfig};

/// 监听计时：从第一次进入录音/导入起算，暂停的那段不计入。
///
/// 锁屏或 WebView 重建后，界面用这个时长续上，而不是从 0 再计一场。
struct ListenClock {
    stage: StatusStage,
    started_at_ms: u64,
    pause_accum_ms: u64,
    paused_since_ms: Option<u64>,
}

impl ListenClock {
    fn new() -> Self {
        Self {
            stage: StatusStage::Starting,
            started_at_ms: 0,
            pause_accum_ms: 0,
            paused_since_ms: None,
        }
    }

    fn observe(&mut self, stage: StatusStage, now_ms: u64) {
        match stage {
            StatusStage::Recording | StatusStage::Importing => {
                if self.started_at_ms == 0 {
                    self.started_at_ms = now_ms;
                }
                if let Some(since) = self.paused_since_ms.take() {
                    self.pause_accum_ms = self.pause_accum_ms.saturating_add(now_ms.saturating_sub(since));
                }
            }
            StatusStage::Paused => {
                if self.started_at_ms == 0 {
                    self.started_at_ms = now_ms;
                }
                if self.paused_since_ms.is_none() {
                    self.paused_since_ms = Some(now_ms);
                }
            }
            StatusStage::Starting | StatusStage::AsrLoading | StatusStage::AsrReady | StatusStage::Idle => {}
        }
        self.stage = stage;
    }

    fn elapsed_ms(&self, now_ms: u64) -> u64 {
        if self.started_at_ms == 0 {
            return 0;
        }
        let open_pause = self.paused_since_ms.map(|since| now_ms.saturating_sub(since)).unwrap_or(0);
        now_ms
            .saturating_sub(self.started_at_ms)
            .saturating_sub(self.pause_accum_ms)
            .saturating_sub(open_pause)
    }
}

/// 统一运行边界：内部仍是现有 `LivePipeline` 线程模型。
pub struct SessionRuntime {
    pipeline: LivePipeline,
    transcript: Arc<Mutex<TranscriptState>>,
    clock: Arc<Mutex<ListenClock>>,
}

impl SessionRuntime {
    pub fn new(cfg: LivePipelineConfig) -> Self {
        Self {
            pipeline: LivePipeline::new(cfg),
            transcript: Arc::new(Mutex::new(TranscriptState::default())),
            clock: Arc::new(Mutex::new(ListenClock::new())),
        }
    }

    pub fn start(&mut self, emit: EventSink) -> Result<()> {
        let transcript = self.transcript.clone();
        let clock = self.clock.clone();
        let wrap: EventSink = Arc::new(move |ev: DomainEvent| {
            if let DomainEvent::Status { stage: s, .. } = &ev {
                clock.lock().unwrap().observe(*s, wall_ms());
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

    /// 继续等待管道线程退出（`stop_with_timeout` 超时后调用）。
    pub fn join_remaining(&mut self, timeout: Duration) -> bool {
        self.pipeline.join_remaining(timeout)
    }

    pub fn set_noise_level(&self, level: f32) {
        self.pipeline.set_noise_level(level);
    }

    pub fn noise_level(&self) -> f32 {
        self.pipeline.noise_level()
    }

    pub fn set_paused(&self, paused: bool) {
        self.pipeline.set_paused(paused);
    }

    pub fn set_playback_speed(&self, speed: f32) {
        self.pipeline.set_playback_speed(speed);
    }

    pub fn is_paused(&self) -> bool {
        self.pipeline.is_paused()
    }

    /// 管道线程已经自己退出（例如采集通道断开）。句柄还在，但循环已经结束。
    pub fn has_exited(&self) -> bool {
        self.pipeline.has_exited()
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let stage = self.clock.lock().unwrap().stage;
        self.transcript.lock().unwrap().snapshot(stage)
    }

    /// 最近的已定稿转写，供当场讲解一个知识点时带上会议语境。从尾部截到 `max_chars`。
    pub fn recent_transcript(&self, max_chars: usize) -> String {
        let state = self.transcript.lock().unwrap();
        let mut lines = Vec::new();
        let mut count = 0usize;
        for seg in state.committed.iter().rev() {
            let text = seg.text.trim();
            if text.is_empty() {
                continue;
            }
            let line = format!("{}：{text}", seg.speaker_label);
            let len = line.chars().count();
            if count > 0 && count + len > max_chars {
                break;
            }
            count += len;
            lines.push(line);
            if count >= max_chars {
                break;
            }
        }
        lines.reverse();
        lines.join("\n")
    }

    /// 本次监听的有效时长。暂停中的时间不计入。
    pub fn elapsed_ms(&self) -> u64 {
        let clock = self.clock.lock().unwrap();
        clock.elapsed_ms(wall_ms())
    }
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn stamp_revision(ev: DomainEvent, revision: u64) -> DomainEvent {
    match ev {
        DomainEvent::Segment {
            speaker_id,
            speaker_label,
            speaker_attribution,
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
            speaker_attribution,
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
    use super::{stamp_revision, ListenClock};
    use crate::RuntimeParams;
    use std::sync::atomic::Ordering;
    use talksage_core::{DomainEvent, StatusStage};

    #[test]
    fn stamps_revision_on_segment() {
        let ev = DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            speaker_attribution: None,
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

    #[test]
    fn listen_clock_skips_paused_time() {
        let mut clock = ListenClock::new();
        clock.observe(StatusStage::Recording, 1_000);
        assert_eq!(clock.elapsed_ms(6_000), 5_000);
        clock.observe(StatusStage::Paused, 6_000);
        assert_eq!(clock.elapsed_ms(9_000), 5_000);
        clock.observe(StatusStage::Recording, 9_000);
        assert_eq!(clock.elapsed_ms(11_000), 7_000);
    }

    #[test]
    fn pause_flag_is_shared_and_defaults_to_running() {
        let params = RuntimeParams::default();
        assert!(!params.paused.load(Ordering::Acquire));
        params.paused.store(true, Ordering::Release);
        assert!(params.paused.load(Ordering::Acquire));
    }
}
