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

/// 快路径钩子：在产生点变换或吞掉 final 段。
///
/// **实际到达这条链的只有 final 段。** 全仓只有一处 `apply_filters` 调用点 ——
/// `StreamWorker::finish_speech` 里 emit 与 on_final 之前 —— 而那里只处理
/// committed 段。有意不过链的两类事件：
/// - **插件自己 emit 的事件**（Metrics / Nudge / Term / Translation …）：设计
///   §3.4 S1，插件产物直接进 sink，不回灌 filter 链，否则会形成递归且难以推理。
/// - **partial（hypothesis）段**：还会被后续 partial 覆盖的猜测文本，不做过滤；
///   过滤/去重只对已 committed 的段有意义。
///
/// filter 的类型是 `DomainEvent -> Option<DomainEvent>`：既能吞（None），
/// 也能改写（返回改过的事件）。改写后的事件是唯一真相 —— sink、observer、
/// 统计计数器都取它。
///
/// 签名里既没有 Result 也没有 PluginContext —— 这是刻意的：filter 必须是
/// 纯函数、不可失败、不可阻塞。想做 IO 或会失败的活，去 SegmentObserver。
///
/// 实现应对非 final-Segment 的输入原样放行：链条本身不保证只喂 final 段，
/// 这条防御让「以后有人接上别的产生点」不至于变成静默的行为变更。
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
    /// 本地即时骨架（同步、无 HTTP）。返回多个事件：一段上可能同时产出
    /// 指标与提示。空向量 = 不发。
    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent>;
    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent>;
}

/// finalizer 的输入。会话已停、已落库，此处只读。
///
/// 刻意保持极简：finalizer 需要的持久数据都能用 `session_id` 从 SessionStore
/// 查到，把整个 store 塞进 context 会让插件能改库，破坏「会后只读」的约束。
///
/// **finalizer 之间不经由本结构传值。** `run_finalizers` 拿的是
/// `&FinalizeContext`，谁也写不进来。链上有顺序依赖时（如 `session_quality`
/// 必须排在 `webhook` 之前），耦合走的是数据库：前者把 meta 写进会话行，
/// 后者重新读这一行来拼载荷。
pub struct FinalizeContext {
    pub session_id: i64,
}

/// 会后钩子：`stop → flush → 落库` 之后执行，不占实时路径。
pub trait SessionFinalizer: Send + Sync {
    fn name(&self) -> &'static str;
    /// 返回 Err 只记录并继续下一个 —— 逐个独立，互不阻塞。
    fn finalize(&self, ctx: &FinalizeContext) -> anyhow::Result<()>;
}

/// `run_finalizers` 的结果汇总。
#[derive(Debug, Default)]
pub struct FinalizeReport {
    /// 执行失败的 finalizer 名字。
    pub failed: Vec<&'static str>,
}

/// 插件：拥有身份与默认配置，在 register() 里把自己挂进钩子。
/// 插件不拥有注册表，只能注册进去（对应 Cordis 的 seam 模型）。
///
/// `ctx` 是宿主能力的唯一入口。钩子一旦进了 `HookRegistry` 就是
/// `Arc<dyn ...>` —— 既不可变也不能 downcast，事后无法再注入。所以需要
/// 宿主依赖的插件（finalizer 尤其）必须在 register 时就把它装进自己。
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn default_config(&self) -> PluginConfig;
    fn register(&self, cfg: &PluginConfig, ctx: &PluginContext, hooks: &mut HookRegistry);

    /// 设置页显示用的人话名字。**归插件自己**：宿主既不认识插件的配置结构，
    /// 也就不该替它写标签表 —— 否则每加一个插件都要改一次前端常量。
    ///
    /// 默认返回 id：忘了写只是设置页里显示 `foo_bar` 而不是「某某」，不会崩。
    fn label(&self) -> &'static str {
        self.id()
    }
}

/// 钩子集合。顺序即执行顺序。
#[derive(Default, Clone)]
pub struct HookRegistry {
    filters: Vec<Arc<dyn EventFilter>>,
    observers: Vec<Arc<dyn SegmentObserver>>,
    finalizers: Vec<Arc<dyn SessionFinalizer>>,
}

impl HookRegistry {
    pub fn add_filter(&mut self, f: Arc<dyn EventFilter>) {
        self.filters.push(f);
    }

    pub fn add_observer(&mut self, o: Arc<dyn SegmentObserver>) {
        self.observers.push(o);
    }

    pub fn add_finalizer(&mut self, f: Arc<dyn SessionFinalizer>) {
        self.finalizers.push(f);
    }

    pub fn observers(&self) -> &[Arc<dyn SegmentObserver>] {
        &self.observers
    }

    pub fn filter_count(&self) -> usize {
        self.filters.len()
    }

    pub fn finalizer_count(&self) -> usize {
        self.finalizers.len()
    }

    /// 依次施加 filter；任一返回 None 即吞掉并中断链条。
    pub fn apply_filters(&self, ev: DomainEvent) -> Option<DomainEvent> {
        self.filters.iter().try_fold(ev, |e, f| f.filter(e))
    }

    /// 依次执行，逐个独立：任一失败只记录并继续，不中断链条。
    pub fn run_finalizers(&self, ctx: &FinalizeContext) -> FinalizeReport {
        let mut report = FinalizeReport::default();
        for f in &self.finalizers {
            if let Err(e) = f.finalize(ctx) {
                log::warn!("finalizer[{}] 失败: {e}", f.name());
                report.failed.push(f.name());
            }
        }
        report
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

#[cfg(test)]
mod finalizer_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct Recording(&'static str, Arc<Mutex<Vec<&'static str>>>);
    impl SessionFinalizer for Recording {
        fn name(&self) -> &'static str { self.0 }
        fn finalize(&self, _ctx: &FinalizeContext) -> anyhow::Result<()> {
            self.1.lock().unwrap().push(self.0);
            Ok(())
        }
    }

    struct Failing(Arc<AtomicUsize>);
    impl SessionFinalizer for Failing {
        fn name(&self) -> &'static str { "failing" }
        fn finalize(&self, _ctx: &FinalizeContext) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            anyhow::bail!("故意失败")
        }
    }

    fn ctx() -> FinalizeContext {
        FinalizeContext { session_id: 1 }
    }

    #[test]
    fn finalizers_run_in_registration_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut hooks = HookRegistry::default();
        hooks.add_finalizer(Arc::new(Recording("first", log.clone())));
        hooks.add_finalizer(Arc::new(Recording("second", log.clone())));
        hooks.run_finalizers(&ctx());
        assert_eq!(*log.lock().unwrap(), vec!["first", "second"]);
    }

    /// 关键契约：一个 finalizer 失败不得阻塞后续的。
    /// webhook 打不通，不能因此丢掉质量评估的写库。
    #[test]
    fn a_failing_finalizer_does_not_block_the_rest() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut hooks = HookRegistry::default();
        hooks.add_finalizer(Arc::new(Failing(calls.clone())));
        hooks.add_finalizer(Arc::new(Recording("after", log.clone())));
        let report = hooks.run_finalizers(&ctx());
        assert_eq!(calls.load(Ordering::Relaxed), 1, "失败的那个应被调用过");
        assert_eq!(*log.lock().unwrap(), vec!["after"], "后续 finalizer 必须照常执行");
        assert_eq!(report.failed, vec!["failing"], "失败项应汇总上报");
    }

    #[test]
    fn empty_registry_reports_no_failures() {
        let hooks = HookRegistry::default();
        assert!(hooks.run_finalizers(&ctx()).failed.is_empty());
    }
}

#[cfg(test)]
mod clone_sharing_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 计数被调用次数的 filter：若 clone 出的是独立实例，两份计数会各自独立。
    #[derive(Default)]
    struct Counting(AtomicUsize);
    impl EventFilter for Counting {
        fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Some(ev)
        }
    }

    /// Task 6 的跨流去重依赖：两条音频流各持一份 HookRegistry 克隆，
    /// 但必须共享同一个 filter 实例（共享去重历史窗口）。
    #[test]
    fn cloned_registry_shares_filter_instances() {
        let counter = Arc::new(Counting::default());
        let mut a = HookRegistry::default();
        a.add_filter(counter.clone());
        let b = a.clone();
        let ev = DomainEvent::Level { mic_rms: 0.0, loopback_rms: 0.0 };
        a.apply_filters(ev.clone());
        b.apply_filters(ev);
        assert_eq!(counter.0.load(Ordering::Relaxed), 2, "克隆必须共享实例，否则跨流去重会静默失效");
    }
}
