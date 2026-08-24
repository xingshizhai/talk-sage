//! 要点抽取插件：committed 段 → 本地规则分类 → DomainEvent::KeyPoint。
//!
//! 纯本地、无 LLM。会后整理（notes）消费落库后的要点，不再从转写重抽。

use std::sync::Mutex;

use serde_json::json;
use talksage_core::{DomainEvent, KeyPointAggregator, ResultStatus, TranscriptSegment};

use crate::registry::{HookRegistry, Plugin, PluginConfig, SegmentObserver};
use crate::PluginContext;

pub struct KeyPointExtractorObserver {
    aggregator: Mutex<KeyPointAggregator>,
}

impl Default for KeyPointExtractorObserver {
    fn default() -> Self {
        Self {
            aggregator: Mutex::new(KeyPointAggregator::new()),
        }
    }
}

impl SegmentObserver for KeyPointExtractorObserver {
    fn name(&self) -> &'static str {
        "key_point_extractor"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        !seg.is_partial && !seg.text.trim().is_empty()
    }

    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let added = self.aggregator.lock().unwrap().push(&seg.text, seg.ts_ms);
        added
            .into_iter()
            .map(|kp| DomainEvent::KeyPoint {
                result_id: kp.result_id,
                status: ResultStatus::Final,
                category: kp.category,
                content: kp.content,
                ts_ms: kp.ts_ms,
            })
            .collect()
    }

    fn run(&self, _seg: &TranscriptSegment, _ctx: &PluginContext) -> anyhow::Result<Option<DomainEvent>> {
        Ok(None)
    }
}

pub struct KeyPointExtractorPlugin;

impl Plugin for KeyPointExtractorPlugin {
    fn descriptor(&self) -> &'static crate::PluginDescriptor {
        static D: crate::PluginDescriptor = crate::PluginDescriptor {
            id: "key_point_extractor",
            label: "要点聚合",
            description: "用本地规则从转写提取问句、要求、决策、行动与技术要点",
            category: crate::PluginCategory::Analysis,
            phase: crate::PluginPhase::Observer,
            capabilities: &[],
            host_managed: &[],
            after: &[],
        };
        &D
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true }))
    }

    fn register(&self, _cfg: &PluginConfig, _ctx: &PluginContext, hooks: &mut HookRegistry) {
        hooks.add_observer(std::sync::Arc::new(KeyPointExtractorObserver::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::KeyPointCategory;

    fn seg(text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: 1,
            speaker_label: "客户".into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms: 1000,
            duration_ms: 800,
            rms: 0.2,
        }
    }

    #[test]
    fn emits_final_key_points_from_rules() {
        let p = KeyPointExtractorObserver::default();
        assert!(p.should_trigger(&seg("We need NPI samples by Friday.")));
        let evs = p.skeleton(&seg("We need NPI samples by Friday."));
        assert!(!evs.is_empty());
        match &evs[0] {
            DomainEvent::KeyPoint {
                status: ResultStatus::Final,
                category: KeyPointCategory::Requirement,
                content,
                ..
            } => assert!(content.contains("NPI") || content.contains("need") || content.contains("Need")),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(p.run(&seg("We need NPI samples by Friday."), &PluginContext::new()).unwrap().is_none());
    }

    #[test]
    fn ignores_partial_and_noise() {
        let p = KeyPointExtractorObserver::default();
        let mut partial = seg("We need NPI samples by Friday.");
        partial.is_partial = true;
        assert!(!p.should_trigger(&partial));
        assert!(p.skeleton(&seg("嗯嗯嗯对那个技术嗯嗯嗯")).is_empty());
    }

    #[test]
    fn plugin_registers_one_observer() {
        let p = KeyPointExtractorPlugin;
        assert_eq!(p.id(), "key_point_extractor");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &PluginContext::new(), &mut hooks);
        assert_eq!(hooks.observers().len(), 1);
    }
}
