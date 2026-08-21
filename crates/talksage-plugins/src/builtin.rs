//! 内置插件中心表。列表顺序即钩子执行顺序。

use std::collections::HashMap;

use serde_json::Value;

use crate::conversation_metrics::ConversationMetricsPlugin;
use crate::cross_stream_dedup::CrossStreamDedupPlugin;
use crate::registry::{HookRegistry, Plugin};
use crate::short_segment::ShortSegmentPlugin;

/// 内置插件清单。
///
/// **顺序即执行顺序**（设计 §3.4 S2）。改动顺序前先看 builtin.rs 里的
/// 顺序不变量测试 —— 它锁住了有依赖关系的相对位置。
pub fn builtin_plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        // filter：便宜的先跑；dedup 需要看两条流的历史，必须在 short_segment 之后
        Box::new(ShortSegmentPlugin),
        Box::new(CrossStreamDedupPlugin),
        // observer：彼此无顺序依赖，排在 filter 之后仅为便于阅读
        Box::new(ConversationMetricsPlugin),
    ]
}

/// 按配置装配钩子。overrides 的键是插件 id。
pub fn build_registry(
    plugins: &[Box<dyn Plugin>],
    overrides: &HashMap<String, Value>,
) -> HookRegistry {
    let mut hooks = HookRegistry::default();
    for p in plugins {
        let mut cfg = p.default_config();
        if let Some(user) = overrides.get(p.id()) {
            cfg.merge(user);
        }
        if !cfg.enabled() {
            log::debug!("插件[{}] 已禁用，跳过注册", p.id());
            continue;
        }
        p.register(&cfg, &mut hooks);
    }
    hooks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use talksage_core::DomainEvent;

    #[test]
    fn plugin_ids_are_unique() {
        let plugins = builtin_plugins();
        let mut seen = std::collections::HashSet::new();
        for p in &plugins {
            assert!(seen.insert(p.id()), "重复的插件 id: {}", p.id());
        }
    }

    #[test]
    fn every_plugin_has_a_parsable_default_config_with_enabled() {
        for p in builtin_plugins() {
            let cfg = p.default_config();
            assert!(
                cfg.as_value().get("enabled").is_some(),
                "插件 {} 的默认配置缺少 enabled 键",
                p.id()
            );
        }
    }

    /// 设计 §3.4 S2：short_segment 必须排在 cross_stream_dedup 之前
    /// —— 便宜的先跑，且 dedup 需要看两条流的历史。
    #[test]
    fn short_segment_is_ordered_before_cross_stream_dedup() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        let short = ids.iter().position(|id| *id == "short_segment").expect("缺少 short_segment");
        let dedup = ids.iter().position(|id| *id == "cross_stream_dedup").expect("缺少 cross_stream_dedup");
        assert!(short < dedup, "short_segment 必须排在 cross_stream_dedup 之前，实际顺序: {ids:?}");
    }

    #[test]
    fn build_registry_skips_disabled_plugins() {
        let mut overrides = HashMap::new();
        overrides.insert("cross_stream_dedup".to_string(), serde_json::json!({"enabled": false}));
        let hooks = build_registry(&builtin_plugins(), &overrides);
        let all = build_registry(&builtin_plugins(), &HashMap::new());
        assert_eq!(hooks.filter_count() + 1, all.filter_count(), "关掉一个插件应少一个 filter");
    }

    #[test]
    fn build_registry_applies_user_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("short_segment".to_string(), serde_json::json!({"min_ms": 400}));
        let hooks = build_registry(&builtin_plugins(), &overrides);
        let short = DomainEvent::Segment {
            speaker_id: 0, speaker_label: "我".into(), text: "喂".into(),
            is_partial: false, ts_ms: 0, duration_ms: 200, rms: 0.1,
            revision: 0, start_sample: 0, end_sample: 3200,
        };
        assert!(hooks.apply_filters(short).is_none(), "200ms < 覆盖后的 400ms 应被吞");
    }
}
