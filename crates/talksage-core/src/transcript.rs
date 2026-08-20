//! 稳定文本与假设尾巴：插件 / SQLite 只消费 committed。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{DomainEvent, StatusStage, TranscriptSegment};

/// 每说话人一条可覆盖的 hypothesis 尾巴。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HypothesisSpan {
    pub speaker_id: u32,
    pub speaker_label: String,
    pub text: String,
    pub ts_ms: u64,
    #[serde(default)]
    pub start_sample: u64,
    #[serde(default)]
    pub end_sample: u64,
}

/// 会话转写状态（Rust 侧累加器；与前端按 speaker 持有尾巴对齐）。
#[derive(Debug, Clone, Default)]
pub struct TranscriptState {
    pub revision: u64,
    pub committed: Vec<TranscriptSegment>,
    pub hypothesis: HashMap<u32, HypothesisSpan>,
    pub processed_until_sample: u64,
    pub committed_until_sample: u64,
}

impl TranscriptState {
    pub fn apply(&mut self, ev: &DomainEvent) {
        let DomainEvent::Segment {
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
        } = ev
        else {
            return;
        };
        self.revision = self.revision.saturating_add(1);
        self.processed_until_sample = self.processed_until_sample.max(*end_sample);
        if *is_partial {
            self.hypothesis.insert(
                *speaker_id,
                HypothesisSpan {
                    speaker_id: *speaker_id,
                    speaker_label: speaker_label.clone(),
                    text: text.clone(),
                    ts_ms: *ts_ms,
                    start_sample: *start_sample,
                    end_sample: *end_sample,
                },
            );
            return;
        }
        self.hypothesis.remove(speaker_id);
        self.committed_until_sample = self.committed_until_sample.max(*end_sample);
        self.committed.push(TranscriptSegment {
            speaker_id: *speaker_id,
            speaker_label: speaker_label.clone(),
            text: text.clone(),
            is_partial: false,
            ts_ms: *ts_ms,
            duration_ms: *duration_ms,
            rms: *rms,
        });
    }

    pub fn snapshot(&self, stage: StatusStage) -> SessionSnapshot {
        let mut hypothesis: Vec<HypothesisSpan> = self.hypothesis.values().cloned().collect();
        hypothesis.sort_by_key(|h| h.speaker_id);
        SessionSnapshot {
            revision: self.revision,
            committed: self.committed.clone(),
            hypothesis,
            processed_until_sample: self.processed_until_sample,
            committed_until_sample: self.committed_until_sample,
            stage,
        }
    }
}

/// 订阅时先发的当前态（committed + 各说话人 hypothesis）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub revision: u64,
    pub committed: Vec<TranscriptSegment>,
    pub hypothesis: Vec<HypothesisSpan>,
    #[serde(default)]
    pub processed_until_sample: u64,
    #[serde(default)]
    pub committed_until_sample: u64,
    pub stage: StatusStage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DomainEvent;

    fn partial(speaker: u32, text: &str, end: u64) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id: speaker,
            speaker_label: "我".into(),
            text: text.into(),
            is_partial: true,
            ts_ms: 1_000 + end / 16,
            duration_ms: 0,
            rms: 0.0,
            revision: 0,
            start_sample: 0,
            end_sample: end,
        }
    }

    fn committed(speaker: u32, text: &str, start: u64, end: u64) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id: speaker,
            speaker_label: "我".into(),
            text: text.into(),
            is_partial: false,
            ts_ms: 1_000 + end / 16,
            duration_ms: (end - start) * 1000 / 16_000,
            rms: 0.1,
            revision: 0,
            start_sample: start,
            end_sample: end,
        }
    }

    #[test]
    fn hypothesis_replaced_then_committed() {
        let mut st = TranscriptState::default();
        st.apply(&partial(0, "昨", 1600));
        st.apply(&partial(0, "昨天是", 3200));
        assert_eq!(st.hypothesis[&0].text, "昨天是");
        assert_eq!(st.committed.len(), 0);
        st.apply(&committed(0, "昨天是星期一。", 0, 4800));
        assert!(st.hypothesis.is_empty());
        assert_eq!(st.committed.len(), 1);
        assert_eq!(st.committed[0].text, "昨天是星期一。");
        assert_eq!(st.committed_until_sample, 4800);
        assert!(st.revision >= 3);
    }

    #[test]
    fn dual_speaker_hypothesis_independent() {
        let mut st = TranscriptState::default();
        st.apply(&DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            text: "我们".into(),
            is_partial: true,
            ts_ms: 10,
            duration_ms: 0,
            rms: 0.0,
            revision: 0,
            start_sample: 0,
            end_sample: 100,
        });
        st.apply(&DomainEvent::Segment {
            speaker_id: 1,
            speaker_label: "客户".into(),
            text: "We need".into(),
            is_partial: true,
            ts_ms: 12,
            duration_ms: 0,
            rms: 0.0,
            revision: 0,
            start_sample: 0,
            end_sample: 120,
        });
        assert_eq!(st.hypothesis.len(), 2);
        let snap = st.snapshot(StatusStage::Recording);
        assert_eq!(snap.hypothesis.len(), 2);
    }
}
