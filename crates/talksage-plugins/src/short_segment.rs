//! short_segment：final 段时长低于阈值时吞掉（噪音短段抑制）。
//!
//! 迁移自 pipeline/src/lib.rs:646 —— 原实现在 StreamWorker::finish_speech
//! 里提前 return，同时抑制事件与插件。作为产生点 filter，语义等价。

use std::sync::Arc;

use serde_json::json;
use talksage_core::DomainEvent;

use crate::registry::{EventFilter, HookRegistry, Plugin, PluginConfig};

/// 默认最短提交时长（ms）。0 = 关闭。
const DEFAULT_MIN_MS: u64 = 0;

pub struct ShortSegmentFilter {
    pub min_ms: u64,
}

impl EventFilter for ShortSegmentFilter {
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
        if self.min_ms == 0 {
            return Some(ev);
        }
        if let DomainEvent::Segment { is_partial: false, duration_ms, speaker_label, text, .. } = &ev {
            if *duration_ms < self.min_ms {
                log::info!(
                    "短段丢弃[{}]: 时长={duration_ms}ms < 最短提交={}ms 文本={}",
                    speaker_label,
                    self.min_ms,
                    text.chars().take(40).collect::<String>(),
                );
                return None;
            }
        }
        Some(ev)
    }
}

pub struct ShortSegmentPlugin;

impl Plugin for ShortSegmentPlugin {
    fn id(&self) -> &'static str {
        "short_segment"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true, "min_ms": DEFAULT_MIN_MS }))
    }

    fn register(&self, cfg: &PluginConfig, hooks: &mut HookRegistry) {
        hooks.add_filter(Arc::new(ShortSegmentFilter {
            min_ms: cfg.get_u64("min_ms", DEFAULT_MIN_MS),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use talksage_core::DomainEvent;

    fn seg(duration_ms: u64, is_partial: bool) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            text: "喂".into(),
            is_partial,
            ts_ms: 1000,
            duration_ms,
            rms: 0.2,
            revision: 0,
            start_sample: 0,
            end_sample: 16000,
        }
    }

    fn filter_with(min_ms: u64) -> ShortSegmentFilter {
        ShortSegmentFilter { min_ms }
    }

    #[test]
    fn drops_final_segments_shorter_than_threshold() {
        assert!(filter_with(300).filter(seg(120, false)).is_none());
    }

    #[test]
    fn keeps_final_segments_at_or_above_threshold() {
        assert!(filter_with(300).filter(seg(300, false)).is_some(), "等于阈值应保留");
        assert!(filter_with(300).filter(seg(800, false)).is_some());
    }

    #[test]
    fn zero_threshold_disables_the_filter() {
        assert!(filter_with(0).filter(seg(1, false)).is_some());
    }

    #[test]
    fn never_touches_partials_or_other_events() {
        // partial 段时长恒为 0，绝不能被当成短段吞掉
        assert!(filter_with(300).filter(seg(0, true)).is_some());
        let level = DomainEvent::Level { mic_rms: 0.1, loopback_rms: 0.0 };
        assert!(filter_with(300).filter(level).is_some());
    }

    #[test]
    fn plugin_registers_a_filter_and_reads_min_ms_from_config() {
        let p = ShortSegmentPlugin;
        assert_eq!(p.id(), "short_segment");
        let mut cfg = p.default_config();
        cfg.merge(&json!({"min_ms": 500}));
        let mut hooks = HookRegistry::default();
        p.register(&cfg, &mut hooks);
        assert_eq!(hooks.filter_count(), 1);
        assert!(hooks.apply_filters(seg(400, false)).is_none(), "400ms < 配置的 500ms 应被吞");
    }
}
