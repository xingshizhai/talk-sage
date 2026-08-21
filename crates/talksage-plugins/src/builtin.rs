//! 内置插件中心表。列表顺序即钩子执行顺序。

use std::collections::HashMap;

use serde_json::Value;

use crate::brief_retriever::BriefRetrieverPluginDef;
use crate::conversation_metrics::ConversationMetricsPlugin;
use crate::cross_stream_dedup::CrossStreamDedupPlugin;
use crate::registry::{HookRegistry, Plugin};
use crate::term_explainer::TermExplainerPluginDef;
use crate::translator::TranslatorPluginDef;
use crate::session_quality::SessionQualityPlugin;
use crate::short_segment::ShortSegmentPlugin;
use crate::webhook::WebhookPlugin;
use crate::PluginContext;

/// 受场景 allowlist 约束的插件 id —— 「会议辅助功能」那一类。
///
/// 只有这些插件的 `enabled` 会被 `SceneParams::plugin_allowlist` 裁决。
/// short_segment / cross_stream_dedup / session_quality / webhook /
/// conversation_metrics **不在**这里：它们是基础设施，不是产品功能。生活模式
/// 关掉术语解释是产品意图；关掉短段抑制不是。
///
/// 新增分析类插件时这里要同步加一行（另一行加在 `builtin_plugins()`）。
pub const ANALYSIS_PLUGIN_IDS: &[&str] = &["term_explainer", "translator", "brief_retriever"];

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
        // 分析类 observer：阶段 5 之前由 service.rs 手工装配
        Box::new(TermExplainerPluginDef),
        Box::new(TranslatorPluginDef),
        Box::new(BriefRetrieverPluginDef),
        // finalizer：session_quality 必须在 webhook 之前 —— 它把质量 meta 写进
        // 会话行，webhook 要重新读这一行来拼载荷
        Box::new(SessionQualityPlugin),
        Box::new(WebhookPlugin),
    ]
}

/// 按配置装配钩子。overrides 的键是插件 id。
///
/// `ctx` 携带宿主能力（知识库 / LLM / 会后依赖），原样透给每个 `register`。
/// 不需要宿主能力的插件忽略它即可。
pub fn build_registry(
    plugins: &[Box<dyn Plugin>],
    overrides: &HashMap<String, Value>,
    ctx: &PluginContext,
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
        p.register(&cfg, ctx, &mut hooks);
    }
    hooks
}

/// 每个内置插件的**生效配置**：`default_config()` + 用户覆盖。
///
/// 宿主拿去展示（doctor / 配置 API / 设置页）。用户表里没写的插件也会出现在
/// 结果里，值是插件默认 —— 前端因此不必知道有哪些插件、默认值是多少。
///
/// 刻意**不**施加场景 allowlist：那是「本次会话会不会装上」的运行期裁决，
/// 不是用户配置。混进来的话，切一次场景就像被人改了配置。
pub fn effective_plugin_configs(
    user: &std::collections::BTreeMap<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for p in builtin_plugins() {
        let mut cfg = p.default_config();
        if let Some(u) = user.get(p.id()) {
            cfg.merge(u);
        }
        out.insert(p.id().to_string(), cfg.as_value().clone());
    }
    out
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

    /// 设计 §3.4 S2：session_quality 必须在 webhook 之前 —— 它把质量 meta 写进
    /// 会话行，webhook 重新读这一行来拼载荷；反过来就会推一条 meta 还没写好的会话。
    /// 耦合走的是**数据库**，不是 FinalizeContext（谁也写不进那个只读引用）。
    #[test]
    fn session_quality_is_ordered_before_webhook() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        let q = ids.iter().position(|id| *id == "session_quality").expect("缺少 session_quality");
        let w = ids.iter().position(|id| *id == "webhook").expect("缺少 webhook");
        assert!(q < w, "session_quality 必须排在 webhook 之前，实际: {ids:?}");
    }

    /// webhook 默认关闭：不给 override 时它不该被装进注册表。
    /// 「插件开关」与「[webhooks] 配置」是两道闸，这里锁住第一道。
    #[test]
    fn webhook_is_not_registered_without_an_explicit_override() {
        let hooks = build_registry(&builtin_plugins(), &HashMap::new(), &PluginContext::new());
        assert_eq!(hooks.finalizer_count(), 1, "默认只应装上 session_quality");
        let mut on = HashMap::new();
        on.insert("webhook".to_string(), serde_json::json!({"enabled": true}));
        assert_eq!(build_registry(&builtin_plugins(), &on, &PluginContext::new()).finalizer_count(), 2);
    }

    #[test]
    fn build_registry_skips_disabled_plugins() {
        let mut overrides = HashMap::new();
        overrides.insert("cross_stream_dedup".to_string(), serde_json::json!({"enabled": false}));
        let ctx = PluginContext::new();
        let hooks = build_registry(&builtin_plugins(), &overrides, &ctx);
        let all = build_registry(&builtin_plugins(), &HashMap::new(), &ctx);
        assert_eq!(hooks.filter_count() + 1, all.filter_count(), "关掉一个插件应少一个 filter");
    }

    /// 阶段 5：三个分析插件必须进注册表，service.rs 不再手工装配。
    #[test]
    fn analysis_plugins_are_in_the_registry() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        for want in ["term_explainer", "translator", "brief_retriever"] {
            assert!(ids.contains(&want), "缺少插件 {want}，实际: {ids:?}");
        }
    }

    /// brief_retriever 依赖知识库：ctx.kb 为 None 时不应注册 observer。
    /// 这是它相对其他插件多出来的一道门（原 service.rs 的 `&& kb.is_some()`）。
    #[test]
    fn brief_retriever_needs_a_knowledge_base() {
        use crate::registry::Plugin as _;
        let p = crate::brief_retriever::BriefRetrieverPluginDef;
        let mut without = HookRegistry::default();
        p.register(&p.default_config(), &PluginContext::new(), &mut without);
        assert_eq!(without.observers().len(), 0, "无知识库时不应注册");

        let mut kb = talksage_knowledge::KnowledgeBase::new();
        kb.index_folder(std::path::Path::new("."));
        let ctx = PluginContext { kb: Some(std::sync::Arc::new(kb)), ..PluginContext::new() };
        let mut with = HookRegistry::default();
        p.register(&p.default_config(), &ctx, &mut with);
        assert_eq!(with.observers().len(), 1, "有知识库时应注册");
    }

    /// 生效配置要覆盖全部插件（用户没写的用默认），且用户值要压过默认值。
    #[test]
    fn effective_configs_cover_every_plugin_and_apply_user_values() {
        let mut user = std::collections::BTreeMap::new();
        user.insert("short_segment".to_string(), serde_json::json!({ "min_ms": 777 }));
        let eff = effective_plugin_configs(&user);
        assert_eq!(eff.len(), builtin_plugins().len(), "每个插件都该有一项");
        assert_eq!(eff["short_segment"]["min_ms"], serde_json::json!(777));
        assert_eq!(eff["short_segment"]["enabled"], serde_json::json!(true), "未覆盖的键取默认");
        assert_eq!(eff["webhook"]["enabled"], serde_json::json!(false), "webhook 默认关闭");
    }

    /// ANALYSIS_PLUGIN_IDS 里的 id 必须真的存在，否则场景 allowlist 会静默失效。
    #[test]
    fn analysis_plugin_ids_all_exist() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        for id in ANALYSIS_PLUGIN_IDS {
            assert!(ids.contains(id), "ANALYSIS_PLUGIN_IDS 里的 {id} 不在注册表: {ids:?}");
        }
    }

    /// 基础设施类插件不受场景 allowlist 约束 —— 这是 allowlist 只列分析类的前提。
    #[test]
    fn infrastructure_plugins_are_not_subject_to_the_allowlist() {
        for id in [
            "short_segment",
            "cross_stream_dedup",
            "conversation_metrics",
            "session_quality",
            "webhook",
        ] {
            assert!(!ANALYSIS_PLUGIN_IDS.contains(&id), "{id} 是基础设施，不该受场景裁决");
        }
    }

    #[test]
    fn build_registry_applies_user_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("short_segment".to_string(), serde_json::json!({"min_ms": 400}));
        let hooks = build_registry(&builtin_plugins(), &overrides, &PluginContext::new());
        let short = DomainEvent::Segment {
            speaker_id: 0, speaker_label: "我".into(), text: "喂".into(),
            is_partial: false, ts_ms: 0, duration_ms: 200, rms: 0.1,
            revision: 0, start_sample: 0, end_sample: 3200,
        };
        assert!(hooks.apply_filters(short).is_none(), "200ms < 覆盖后的 400ms 应被吞");
    }
}
