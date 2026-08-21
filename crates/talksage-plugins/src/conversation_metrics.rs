//! conversation_metrics：跨段累计会话指标，并按规则产出会中提示。
//!
//! 迁移自 pipeline/src/lib.rs 的 emit 包装。一次计算同时产出 Metrics 与
//! （可选的）Nudge —— 两者共享同一份 seg_log，不重复计算。

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;
use talksage_core::{DomainEvent, NudgeEngine, TranscriptSegment};

use crate::registry::{HookRegistry, Plugin, PluginConfig, SegmentObserver};
use crate::PluginContext;

/// 跨段累计的会话指标 + 提示引擎。
pub struct ConversationMetricsObserver {
    seg_log: Mutex<Vec<TranscriptSegment>>,
    nudge: Mutex<NudgeEngine>,
    /// 会话起点，用于 nudge 的 call_ms。
    started: Instant,
}

impl Default for ConversationMetricsObserver {
    fn default() -> Self {
        Self {
            seg_log: Mutex::new(Vec::new()),
            nudge: Mutex::new(NudgeEngine::default()),
            started: Instant::now(),
        }
    }
}

impl SegmentObserver for ConversationMetricsObserver {
    fn name(&self) -> &'static str {
        "conversation_metrics"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        !seg.is_partial
    }

    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let metrics = {
            let mut log = self.seg_log.lock().unwrap();
            log.push(seg.clone());
            talksage_core::compute_conversation_metrics(&log)
        };
        let mut out = vec![DomainEvent::Metrics {
            metrics: metrics.clone(),
        }];
        let call_ms = self.started.elapsed().as_millis() as u64;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if let Some(nudge) = self.nudge.lock().unwrap().evaluate(&metrics, call_ms, now_ms) {
            log::info!("会中提示[{:?}] {}", nudge.kind, nudge.message);
            out.push(DomainEvent::Nudge { nudge });
        }
        out
    }

    /// 纯本地计算，没有慢路径工作。
    fn run(&self, _seg: &TranscriptSegment, _ctx: &crate::PluginContext) -> anyhow::Result<Option<DomainEvent>> {
        Ok(None)
    }
}

pub struct ConversationMetricsPlugin;

impl Plugin for ConversationMetricsPlugin {
    fn descriptor(&self) -> &'static crate::PluginDescriptor {
        static D: crate::PluginDescriptor = crate::PluginDescriptor {
            id: "conversation_metrics", label: "会话指标",
            description: "实时计算讲话比例、问题和会中提示",
            category: crate::PluginCategory::Infrastructure, phase: crate::PluginPhase::Observer,
            capabilities: &[], host_managed: &[], after: &[],
        };
        &D
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true }))
    }

    fn register(&self, _cfg: &PluginConfig, _ctx: &PluginContext, hooks: &mut HookRegistry) {
        hooks.add_observer(Arc::new(ConversationMetricsObserver::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::{DomainEvent, TranscriptSegment};

    fn seg(speaker_id: u32, label: &str, text: &str, ts_ms: u64) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id,
            speaker_label: label.into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms,
            duration_ms: 1000,
            rms: 0.2,
        }
    }

    #[test]
    fn emits_metrics_for_every_final_segment() {
        let p = ConversationMetricsObserver::default();
        let evs = p.skeleton(&seg(0, "我", "我们下周确认交期", 1000));
        assert!(
            evs.iter().any(|e| matches!(e, DomainEvent::Metrics { .. })),
            "每个 final 段都应产出 Metrics: {evs:?}"
        );
    }

    #[test]
    fn metrics_accumulate_across_segments() {
        let p = ConversationMetricsObserver::default();
        p.skeleton(&seg(0, "我", "第一句", 1000));
        let evs = p.skeleton(&seg(1, "客户", "第二句", 2000));
        let m = evs
            .iter()
            .find_map(|e| match e {
                DomainEvent::Metrics { metrics } => Some(metrics),
                _ => None,
            })
            .expect("应有 Metrics");
        assert_eq!(
            m.segment_count_me + m.segment_count_them,
            2,
            "指标必须跨段累计，而不是只看当前段: {m:?}"
        );
    }

    #[test]
    fn ignores_partial_segments() {
        let p = ConversationMetricsObserver::default();
        let mut partial = seg(0, "我", "半句", 1000);
        partial.is_partial = true;
        assert!(!p.should_trigger(&partial), "partial 不应触发指标累计");
    }

    #[test]
    fn nudge_is_optional_not_emitted_every_segment() {
        // NudgeEngine 有 2 分钟冷却；单段不应立刻触发提示。
        let p = ConversationMetricsObserver::default();
        let evs = p.skeleton(&seg(0, "我", "你好", 1000));
        assert!(
            !evs.iter().any(|e| matches!(e, DomainEvent::Nudge { .. })),
            "首段不应立刻产出 Nudge（冷却未到）: {evs:?}"
        );
    }

    #[test]
    fn plugin_registers_one_observer() {
        let p = ConversationMetricsPlugin;
        assert_eq!(p.id(), "conversation_metrics");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &PluginContext::new(), &mut hooks);
        assert_eq!(hooks.observers().len(), 1);
    }
}
