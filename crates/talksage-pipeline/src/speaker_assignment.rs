//! final 句段的说话人分配阶段。
//!
//! 查询声纹不会立刻修改聚类库；只有下游 filter 接受句段后才执行 commit。

use crate::speaker::{SharedSpeaker, SpeakerDecision};
use talksage_core::{AudioSource, SpeakerAttribution, VoiceIdentity};

fn is_stable_voice_decision(decision: SpeakerDecision) -> bool {
    matches!(
        decision,
        SpeakerDecision::OwnerMatch
            | SpeakerDecision::ExistingMatch
            | SpeakerDecision::GrayZoneReuse
            | SpeakerDecision::CandidateConfirmed
    )
}

pub(super) struct SpeakerAssignment {
    source_id: u32,
    label: String,
    diagnostic: Option<(SpeakerDecision, Option<f32>)>,
    attribution: SpeakerAttribution,
    commit: Option<Box<dyn FnOnce() + Send>>,
}

impl SpeakerAssignment {
    pub(super) fn resolve(
        speaker: Option<SharedSpeaker>,
        audio: &[f32],
        source_id: u32,
        source: AudioSource,
        fallback_label: &str,
        recognize_owner: bool,
    ) -> Self {
        let Some(speaker) = speaker else {
            return Self {
                source_id,
                label: fallback_label.to_string(),
                diagnostic: None,
                attribution: SpeakerAttribution::from_legacy(source, fallback_label),
                commit: None,
            };
        };
        // 单麦克风多人模式且未注册主人时，不能把未确认的新身份回退成“我”。
        // 主人声纹只是把某个聚类命名为“我”的可选增强，不是多人聚类前置条件。
        let fallback = if recognize_owner && !speaker.has_owner() && fallback_label == "我" {
            "讲话者"
        } else {
            fallback_label
        };
        let query = speaker.query_for_role(audio, fallback, recognize_owner);
        let label = query.label().to_string();
        let diagnostic = Some((query.decision(), query.similarity()));
        let mut attribution = SpeakerAttribution::from_legacy(source, &label);
        if is_stable_voice_decision(query.decision()) {
            attribution.voice = Some(VoiceIdentity {
                id: if label == "我" {
                    "owner".into()
                } else {
                    label.clone()
                },
                confidence: query.similarity(),
            });
        }
        let commit_speaker = speaker.clone();
        let commit = Box::new(move || {
            commit_speaker.commit(&query);
        });
        Self {
            source_id,
            label,
            diagnostic,
            attribution,
            commit: Some(commit),
        }
    }

    /// 音频来源/业务通道 id 保持稳定，不被声纹聚类编号覆盖。
    pub(super) fn source_id(&self) -> u32 {
        self.source_id
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) fn diagnostic(&self) -> Option<(SpeakerDecision, Option<f32>)> {
        self.diagnostic
    }

    pub(super) fn attribution(&self) -> &SpeakerAttribution {
        &self.attribution
    }

    /// filter 接受句段以后才调用；assignment 被直接丢弃不会留下声纹状态。
    pub(super) fn commit(mut self) {
        if let Some(commit) = self.commit.take() {
            commit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn fallback_preserves_source_identity_and_label() {
        let assignment =
            SpeakerAssignment::resolve(None, &[], 7, AudioSource::SystemLoopback, "客户", false);
        assert_eq!(assignment.source_id(), 7);
        assert_eq!(assignment.label(), "客户");
        assert_eq!(assignment.diagnostic(), None);
        assert_eq!(assignment.attribution().source, AudioSource::SystemLoopback);
        assert_eq!(
            assignment.attribution().role,
            talksage_core::SpeakerRole::Client
        );
    }

    #[test]
    fn dropping_assignment_does_not_run_deferred_commit() {
        let commits = Arc::new(AtomicUsize::new(0));
        let counter = commits.clone();
        let assignment = SpeakerAssignment {
            source_id: 0,
            label: "客户1".into(),
            diagnostic: None,
            attribution: SpeakerAttribution::default(),
            commit: Some(Box::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            })),
        };
        drop(assignment);
        assert_eq!(commits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn accepted_assignment_commits_exactly_once() {
        let commits = Arc::new(AtomicUsize::new(0));
        let counter = commits.clone();
        let assignment = SpeakerAssignment {
            source_id: 0,
            label: "客户1".into(),
            diagnostic: None,
            attribution: SpeakerAttribution::default(),
            commit: Some(Box::new(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            })),
        };
        assignment.commit();
        assert_eq!(commits.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn provisional_or_fallback_decisions_are_not_exposed_as_stable_voice_ids() {
        for decision in [
            SpeakerDecision::LowQualityFallback,
            SpeakerDecision::CandidateStarted,
            SpeakerDecision::SpeakerLimitFallback,
        ] {
            assert!(!is_stable_voice_decision(decision));
        }
        assert!(is_stable_voice_decision(SpeakerDecision::OwnerMatch));
        assert!(is_stable_voice_decision(SpeakerDecision::CandidateConfirmed));
    }
}
