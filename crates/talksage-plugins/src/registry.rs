//! 插件注册表：Plugin trait、三类钩子、配置载体。

use serde_json::Value;

/// 插件配置载体。用 serde_json::Value 与 ConfigManager 已有的
/// apply_scene_params(p, u: &Value) 模式保持一致，不引入新的 schema 机制。
#[derive(Debug, Clone)]
pub struct PluginConfig(Value);

impl Default for PluginConfig {
    fn default() -> Self {
        Self(Value::Object(Default::default()))
    }
}

impl PluginConfig {
    pub fn from_value(v: Value) -> Self {
        Self(v)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// 用户值覆盖：只覆盖 user 里出现的键，其余保留默认。
    pub fn merge(&mut self, user: &Value) {
        let (Value::Object(base), Value::Object(over)) = (&mut self.0, user) else {
            return;
        };
        for (k, v) in over {
            base.insert(k.clone(), v.clone());
        }
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        self.0.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    pub fn get_u64(&self, key: &str, default: u64) -> u64 {
        self.0.get(key).and_then(Value::as_u64).unwrap_or(default)
    }

    pub fn get_str(&self, key: &str, default: &str) -> String {
        self.0.get(key).and_then(Value::as_str).unwrap_or(default).to_string()
    }

    /// 约定键：所有插件都有 enabled，缺省为 true。
    pub fn enabled(&self) -> bool {
        self.get_bool("enabled", true)
    }
}

use std::sync::Arc;
use talksage_core::{DomainEvent, TranscriptSegment};
use crate::PluginContext;

/// 快路径钩子：每个事件都过。
///
/// 签名里既没有 Result 也没有 PluginContext —— 这是刻意的：filter 必须是
/// 纯函数、不可失败、不可阻塞。想做 IO 或会失败的活，去 SegmentObserver。
pub trait EventFilter: Send + Sync {
    /// 返回 None 表示吞掉该事件：既不进 sink，也不触发 observer。
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent>;
}

/// 慢路径钩子：committed 段触发。
/// skeleton 同步、本地、无 HTTP；run 在独立线程，可含 LLM。
pub trait SegmentObserver: Send + Sync {
    fn name(&self) -> &'static str;
    fn should_trigger(&self, seg: &TranscriptSegment) -> bool;
    /// 是否消费 hypothesis（partial）。默认 false：只处理 committed。
    fn accepts_speculative(&self) -> bool {
        false
    }
    fn skeleton(&self, seg: &TranscriptSegment) -> Option<DomainEvent>;
    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent>;
}

/// 插件：拥有身份与默认配置，在 register() 里把自己挂进钩子。
/// 插件不拥有注册表，只能注册进去（对应 Cordis 的 seam 模型）。
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn default_config(&self) -> PluginConfig;
    fn register(&self, cfg: &PluginConfig, hooks: &mut HookRegistry);
}

/// 钩子集合。顺序即执行顺序。
#[derive(Default, Clone)]
pub struct HookRegistry {
    filters: Vec<Arc<dyn EventFilter>>,
    observers: Vec<Arc<dyn SegmentObserver>>,
}

impl HookRegistry {
    pub fn add_filter(&mut self, f: Arc<dyn EventFilter>) {
        self.filters.push(f);
    }

    pub fn add_observer(&mut self, o: Arc<dyn SegmentObserver>) {
        self.observers.push(o);
    }

    pub fn observers(&self) -> &[Arc<dyn SegmentObserver>] {
        &self.observers
    }

    pub fn filter_count(&self) -> usize {
        self.filters.len()
    }

    /// 依次施加 filter；任一返回 None 即吞掉并中断链条。
    pub fn apply_filters(&self, ev: DomainEvent) -> Option<DomainEvent> {
        self.filters.iter().try_fold(ev, |e, f| f.filter(e))
    }
}

#[cfg(test)]
mod hook_tests {
    use super::*;
    use std::sync::Arc;
    use talksage_core::DomainEvent;

    /// 测试替身：吞掉文本等于 drop_text 的 final 段。
    struct DropByText(&'static str);
    impl EventFilter for DropByText {
        fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
            match &ev {
                DomainEvent::Segment { text, .. } if text == self.0 => None,
                _ => Some(ev),
            }
        }
    }

    /// 测试替身：给文本加后缀，用来验证链式顺序。
    struct AppendSuffix(&'static str);
    impl EventFilter for AppendSuffix {
        fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
            match ev {
                DomainEvent::Segment { speaker_id, speaker_label, text, is_partial, ts_ms,
                                       duration_ms, rms, revision, start_sample, end_sample } => {
                    Some(DomainEvent::Segment {
                        speaker_id, speaker_label, text: format!("{text}{}", self.0),
                        is_partial, ts_ms, duration_ms, rms, revision, start_sample, end_sample,
                    })
                }
                other => Some(other),
            }
        }
    }

    fn seg(text: &str) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id: 0,
            speaker_label: "我".into(),
            text: text.into(),
            is_partial: false,
            ts_ms: 0,
            duration_ms: 500,
            rms: 0.1,
            revision: 0,
            start_sample: 0,
            end_sample: 8000,
        }
    }

    #[test]
    fn filters_apply_in_registration_order() {
        let mut hooks = HookRegistry::default();
        hooks.add_filter(Arc::new(AppendSuffix("-a")));
        hooks.add_filter(Arc::new(AppendSuffix("-b")));
        let out = hooks.apply_filters(seg("x")).expect("不应被吞掉");
        match out {
            DomainEvent::Segment { text, .. } => assert_eq!(text, "x-a-b", "应按注册顺序依次施加"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_filter_returning_none_swallows_the_event_and_stops_the_chain() {
        let mut hooks = HookRegistry::default();
        hooks.add_filter(Arc::new(DropByText("x")));
        hooks.add_filter(Arc::new(AppendSuffix("-never")));
        assert!(hooks.apply_filters(seg("x")).is_none(), "被吞掉的事件不应继续");
        // 不匹配的事件应原样穿过整条链
        let out = hooks.apply_filters(seg("y")).expect("不应被吞掉");
        match out {
            DomainEvent::Segment { text, .. } => assert_eq!(text, "y-never"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_registry_passes_everything_through() {
        let hooks = HookRegistry::default();
        assert!(hooks.apply_filters(seg("x")).is_some());
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_values_override_defaults_and_unknown_keys_are_kept() {
        let mut cfg = PluginConfig::from_value(json!({"enabled": true, "cooldown_seconds": 30.0}));
        cfg.merge(&json!({"cooldown_seconds": 5.0}));
        assert_eq!(cfg.get_f64("cooldown_seconds", 0.0), 5.0);
        assert!(cfg.enabled(), "未覆盖的 enabled 应保留默认值");
    }

    #[test]
    fn missing_keys_fall_back_to_the_supplied_default() {
        let cfg = PluginConfig::from_value(json!({}));
        assert_eq!(cfg.get_u64("min_ms", 300), 300);
        assert_eq!(cfg.get_f64("ratio", 0.5), 0.5);
        assert!(cfg.get_bool("whatever", true));
    }

    #[test]
    fn enabled_defaults_to_true_and_can_be_switched_off() {
        assert!(PluginConfig::from_value(json!({})).enabled());
        assert!(!PluginConfig::from_value(json!({"enabled": false})).enabled());
    }
}
