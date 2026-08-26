//! 内置插件中心表。列表顺序即钩子执行顺序。

use std::collections::HashMap;

use serde_json::Value;

use crate::brief_retriever::BriefRetrieverPluginDef;
use crate::conversation_metrics::ConversationMetricsPlugin;
use crate::cross_stream_dedup::CrossStreamDedupPlugin;
use crate::key_point_extractor::KeyPointExtractorPlugin;
use crate::key_point_llm::KeyPointLlmPlugin;
use crate::registry::{
    config_type_name, CapabilityAvailability, HookRegistry, Plugin, PluginCategory,
    PluginConfig, PluginConfigIssue, PluginRegistration, RegistrationStatus,
};
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
pub fn analysis_plugin_ids() -> Vec<&'static str> {
    builtin_plugins()
        .into_iter()
        .filter(|plugin| plugin.descriptor().category == PluginCategory::Analysis)
        .map(|plugin| plugin.id())
        .collect()
}

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
        Box::new(KeyPointExtractorPlugin),
        Box::new(KeyPointLlmPlugin),
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
    build_registry_with_report(plugins, overrides, ctx).hooks
}

pub struct RegistryBuild {
    pub hooks: HookRegistry,
    pub registrations: Vec<PluginRegistration>,
}

pub fn build_registry_with_report(
    plugins: &[Box<dyn Plugin>],
    overrides: &HashMap<String, Value>,
    ctx: &PluginContext,
) -> RegistryBuild {
    let mut hooks = HookRegistry::default();
    let mut registrations = Vec::with_capacity(plugins.len());
    for p in plugins {
        let (cfg, registration) = prepare_plugin(
            p.as_ref(), overrides, ctx.capability_availability()
        );
        match registration.status {
            RegistrationStatus::Active => p.register(&cfg, ctx, &mut hooks),
            RegistrationStatus::Disabled => log::debug!("插件[{}] 已禁用，跳过注册", p.id()),
            RegistrationStatus::Unavailable => log::warn!(
                "插件[{}] 不可用，缺少宿主能力: {}",
                p.id(), registration.missing_capabilities.join(", ")
            ),
            RegistrationStatus::InvalidConfig => {
                for issue in &registration.issues {
                    log::warn!("插件[{}] 配置无效，跳过注册: {issue}", p.id());
                }
            }
        }
        registrations.push(registration);
    }
    RegistryBuild { hooks, registrations }
}

pub fn evaluate_plugin_registrations(
    plugins: &[Box<dyn Plugin>],
    overrides: &HashMap<String, Value>,
    availability: CapabilityAvailability,
) -> Vec<PluginRegistration> {
    plugins
        .iter()
        .map(|plugin| prepare_plugin(plugin.as_ref(), overrides, availability).1)
        .collect()
}

fn prepare_plugin(
    plugin: &dyn Plugin,
    overrides: &HashMap<String, Value>,
    availability: CapabilityAvailability,
) -> (PluginConfig, PluginRegistration) {
    let mut cfg = plugin.default_config();
    if let Some(user) = overrides.get(plugin.id()) {
        cfg.merge(user);
    }
    let issues = plugin.validate_config(&cfg);
    let missing_capabilities: Vec<&'static str> = plugin
        .descriptor()
        .capabilities
        .iter()
        .copied()
        .filter(|capability| !availability.has(*capability))
        .map(|capability| capability.as_str())
        .collect();
    let status = if !issues.is_empty() {
        RegistrationStatus::InvalidConfig
    } else if !cfg.enabled() {
        RegistrationStatus::Disabled
    } else if !missing_capabilities.is_empty() {
        RegistrationStatus::Unavailable
    } else {
        RegistrationStatus::Active
    };
    let registration = PluginRegistration {
        id: plugin.id(), label: plugin.label(), status, missing_capabilities, issues,
    };
    (cfg, registration)
}

/// 校验设置/API 提交的 `plugins` patch。
///
/// 与启动时的 `Plugin::validate_config` 形成双层防护：这里让错误即时反馈给
/// 用户；装配时仍会防御手工编辑或旧文件中的非法值。`notes` 属于宿主配置，
/// 不走插件 schema。未知插件 id 在提交入口拒绝，但手工 TOML 仍由 config
/// crate 原样保留，以便未来版本安装对应插件后恢复。
pub fn validate_plugin_updates(updates: &Value) -> Vec<PluginConfigIssue> {
    let Some(entries) = updates.as_object() else {
        return vec![PluginConfigIssue {
            path: "plugins".to_string(),
            message: "必须是对象".to_string(),
        }];
    };
    let plugins = builtin_plugins();
    let mut issues = Vec::new();
    for (id, patch) in entries {
        if id == "notes" {
            continue;
        }
        let Some(plugin) = plugins.iter().find(|plugin| plugin.id() == id) else {
            issues.push(PluginConfigIssue {
                path: format!("plugins.{id}"),
                message: "未知插件".to_string(),
            });
            continue;
        };
        let mut config = plugin.default_config();
        config.merge(patch);
        if !patch.is_object() {
            issues.push(PluginConfigIssue {
                path: format!("plugins.{id}"),
                message: "配置必须是对象".to_string(),
            });
            continue;
        }
        issues.extend(plugin.validate_config(&config));
    }
    issues
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

/// 从 descriptor 派生的宿主裁决配置键 —— `(插件 id, 配置键)`。
///
/// 装配时 `plugin_overrides_for`（talksage-pipeline）无条件覆盖它们：
/// `short_segment.min_ms` 取场景的 `min_segment_ms`；`cross_stream_dedup.enabled`
/// 恒为真；`brief_retriever.include_user` 在无客户流时为真（演讲检索主讲人）。
///
/// 元数据带上它，设置页才能把这些控件置灰并说明原因。否则页面上会出现一个
/// 能改、保存也成功、运行时却永远不生效的输入框 —— 那比没有这个输入框更糟。
///
/// talksage-pipeline 侧有测试锁住「这里声明的键确实被覆盖」，防止两处漂移。
pub fn host_managed_keys() -> Vec<(&'static str, &'static str)> {
    builtin_plugins()
        .into_iter()
        .flat_map(|plugin| {
            let id = plugin.id();
            plugin.descriptor().host_managed.iter().map(move |key| (id, *key))
        })
        .collect()
}

/// 插件元数据：设置 UI 用它**生成**表单，而不是硬编码控件。
///
/// 每项：
/// - `id`       —— 提交时 `plugins.<id>` 的键
/// - `label`    —— 显示名，插件自己给（见 `Plugin::label`）
/// - `analysis` —— 是否受场景 allowlist 约束（「会议辅助功能」那一类）
/// - `schema`   —— 默认配置整体，保留给现有设置页使用；
/// - `config_schema` —— 可供 API 客户端和校验工具消费的结构化 schema，当前
///                      包含字段类型、默认值和 `additionalProperties=false`；
/// - `host_managed` —— 该插件 descriptor 声明的宿主裁决键，
///                 设置页据此置灰。
///
/// 顺序与 `builtin_plugins()` 一致 —— 设置页按这个顺序排。
pub fn plugin_metadata() -> Vec<Value> {
    builtin_plugins()
        .iter()
        .map(|p| {
            let descriptor = p.descriptor();
            let managed = descriptor.host_managed;
            serde_json::json!({
                "id": p.id(),
                "label": p.label(),
                "description": descriptor.description,
                "category": descriptor.category.as_str(),
                "phase": descriptor.phase.as_str(),
                "capabilities": descriptor.capabilities.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
                "after": descriptor.after,
                "analysis": descriptor.category == PluginCategory::Analysis,
                "schema": p.default_config().as_value(),
                "config_schema": config_schema(p.default_config().as_value()),
                "host_managed": managed,
            })
        })
        .collect()
}

fn config_schema(defaults: &Value) -> Value {
    let properties = defaults
        .as_object()
        .map(|values| {
            values
                .iter()
                .map(|(key, default)| {
                    (key.clone(), serde_json::json!({
                        "type": config_type_name(default),
                        "default": default,
                    }))
                })
                .collect::<serde_json::Map<String, Value>>()
        })
        .unwrap_or_default();
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
    })
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

    #[test]
    fn plugin_updates_reject_unknown_plugins_fields_and_wrong_types() {
        let issues = validate_plugin_updates(&serde_json::json!({
            "missing_plugin": { "enabled": true },
            "term_explainer": { "enabled": "yes", "mystery": 1 },
        }));
        let paths: Vec<&str> = issues.iter().map(|issue| issue.path.as_str()).collect();
        assert!(paths.contains(&"plugins.missing_plugin"));
        assert!(paths.contains(&"term_explainer.enabled"));
        assert!(paths.contains(&"term_explainer.mystery"));
    }

    #[test]
    fn plugin_updates_accept_partial_known_config_and_notes() {
        assert!(validate_plugin_updates(&serde_json::json!({
            "term_explainer": { "cooldown_seconds": 5 },
            "notes": { "template": "standard" },
        }))
        .is_empty());
    }

    #[test]
    fn registry_skips_invalid_hand_written_config() {
        let plugins: Vec<Box<dyn Plugin>> = vec![Box::new(ShortSegmentPlugin)];
        let overrides = HashMap::from([(
            "short_segment".to_string(),
            serde_json::json!({ "enabled": true, "min_ms": "three hundred" }),
        )]);
        let hooks = build_registry(&plugins, &overrides, &PluginContext::new());
        assert_eq!(hooks.filter_count(), 0, "非法手写配置不能进入实时 filter 链");
    }

    #[test]
    fn metadata_exposes_machine_readable_config_schema() {
        for meta in plugin_metadata() {
            assert_eq!(meta["config_schema"]["type"], "object");
            assert_eq!(meta["config_schema"]["additionalProperties"], false);
            assert_eq!(
                meta["config_schema"]["properties"]["enabled"]["type"],
                "boolean"
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
        let build = build_registry_with_report(&builtin_plugins(), &HashMap::new(), &PluginContext::new());
        assert_eq!(build.hooks.finalizer_count(), 0, "无宿主能力时不应装空转 finalizer");
        let webhook = build.registrations.iter().find(|item| item.id == "webhook").unwrap();
        assert_eq!(webhook.status, RegistrationStatus::Disabled);
        let quality = build.registrations.iter().find(|item| item.id == "session_quality").unwrap();
        assert_eq!(quality.status, RegistrationStatus::Unavailable);
        let mut on = HashMap::new();
        on.insert("webhook".to_string(), serde_json::json!({"enabled": true}));
        let build = build_registry_with_report(&builtin_plugins(), &on, &PluginContext::new());
        let webhook = build.registrations.iter().find(|item| item.id == "webhook").unwrap();
        assert_eq!(webhook.status, RegistrationStatus::Unavailable);
        assert_eq!(webhook.missing_capabilities, vec!["webhook"]);
    }

    #[test]
    fn registration_report_covers_active_disabled_unavailable_and_invalid() {
        let plugins: Vec<Box<dyn Plugin>> = vec![
            Box::new(ShortSegmentPlugin),
            Box::new(TermExplainerPluginDef),
            Box::new(WebhookPlugin),
        ];
        let overrides = HashMap::from([
            ("short_segment".to_string(), serde_json::json!({"min_ms": "bad"})),
            ("webhook".to_string(), serde_json::json!({"enabled": false})),
        ]);
        let build = build_registry_with_report(&plugins, &overrides, &PluginContext::new());
        assert_eq!(build.registrations[0].status, RegistrationStatus::InvalidConfig);
        assert_eq!(build.registrations[1].status, RegistrationStatus::Unavailable);
        assert_eq!(build.registrations[1].missing_capabilities, vec!["llm"]);
        assert_eq!(build.registrations[2].status, RegistrationStatus::Disabled);

        let ctx = PluginContext {
            llm: Some(std::sync::Arc::new(talksage_llm::MockProvider { response: "ok".into() })),
            ..PluginContext::new()
        };
        let active = build_registry_with_report(
            &[Box::new(TermExplainerPluginDef)],
            &HashMap::new(),
            &ctx,
        );
        assert_eq!(active.registrations[0].status, RegistrationStatus::Active);
        assert_eq!(active.hooks.observers().len(), 1);
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

    /// 阶段 5：分析类插件必须进注册表，service.rs 不再手工装配。
    #[test]
    fn analysis_plugins_are_in_the_registry() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        for want in ["term_explainer", "translator", "brief_retriever", "key_point_extractor"] {
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

    /// 分析类 id 从 descriptor 派生，且必须真的存在。
    #[test]
    fn analysis_plugin_ids_all_exist() {
        let ids: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        for id in analysis_plugin_ids() {
            assert!(ids.contains(&id), "分析类插件 {id} 不在注册表: {ids:?}");
        }
    }

    /// 基础设施类插件不受场景 allowlist 约束 —— 这是 allowlist 只列分析类的前提。
    #[test]
    fn infrastructure_plugins_are_not_subject_to_the_allowlist() {
        let analysis = analysis_plugin_ids();
        for id in [
            "short_segment",
            "cross_stream_dedup",
            "conversation_metrics",
            "session_quality",
            "webhook",
        ] {
            assert!(!analysis.contains(&id), "{id} 是基础设施，不该受场景裁决");
        }
    }

    /// 元数据必须覆盖每个插件，且每项都带设置页依赖的四个键。
    /// 少一个 `schema.enabled`，设置页就会渲染出一个没有开关的插件卡片。
    #[test]
    fn metadata_covers_every_plugin_with_the_keys_the_ui_needs() {
        let meta = plugin_metadata();
        assert_eq!(meta.len(), builtin_plugins().len(), "每个插件都该有一项元数据");
        for m in &meta {
            let id = m["id"].as_str().expect("id 必须是字符串");
            assert!(!id.is_empty(), "id 不能为空");
            let label = m["label"].as_str().unwrap_or_else(|| panic!("插件 {id} 缺少 label"));
            assert!(!label.is_empty(), "插件 {id} 的 label 不能为空");
            assert!(m["description"].as_str().is_some_and(|value| !value.is_empty()));
            assert!(m["category"].is_string());
            assert!(m["phase"].is_string());
            assert!(m["capabilities"].is_array());
            assert!(m["after"].is_array());
            assert!(m["analysis"].is_boolean(), "插件 {id} 的 analysis 必须是布尔");
            assert!(
                m["schema"]["enabled"].is_boolean(),
                "插件 {id} 的 schema 缺少布尔 enabled —— 设置页会渲染不出开关"
            );
        }
    }

    /// 元数据的 `analysis` 是前端场景 allowlist 勾选框的唯一来源，
    /// 必须与 descriptor 派生列表完全一致。
    #[test]
    fn metadata_analysis_flag_matches_the_allowlist_constant() {
        let flagged: Vec<String> = plugin_metadata()
            .iter()
            .filter(|m| m["analysis"] == serde_json::json!(true))
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(flagged, analysis_plugin_ids(), "analysis 标记与 descriptor 不一致");
    }

    /// 元数据顺序即 `builtin_plugins()` 顺序 —— 设置页照单渲染。
    #[test]
    fn metadata_preserves_registry_order() {
        let want: Vec<&str> = builtin_plugins().iter().map(|p| p.id()).collect();
        let meta = plugin_metadata();
        let got: Vec<&str> = meta.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(got, want);
    }

    /// 宿主裁决的键必须真的在对应插件的 schema 里 —— 键名写错的话，
    /// 设置页会照常渲染出一个能改但不生效的输入框，而这正是它要防的事。
    #[test]
    fn host_managed_keys_exist_in_their_plugins_schema() {
        let meta = plugin_metadata();
        for (id, key) in host_managed_keys() {
            let m = meta
                .iter()
                .find(|m| m["id"] == serde_json::json!(id))
                .unwrap_or_else(|| panic!("descriptor host_managed 的插件 {id} 不存在"));
            assert!(!m["schema"][key].is_null(), "插件 {id} 的 schema 里没有 {key}");
            assert!(
                m["host_managed"].as_array().unwrap().contains(&serde_json::json!(key)),
                "元数据没把 {id}.{key} 标成宿主裁决"
            );
        }
    }

    #[test]
    fn descriptor_order_dependencies_are_satisfied() {
        let plugins = builtin_plugins();
        let ids: Vec<&str> = plugins.iter().map(|plugin| plugin.id()).collect();
        for (index, plugin) in plugins.iter().enumerate() {
            for dependency in plugin.descriptor().after {
                let dependency_index = ids
                    .iter()
                    .position(|id| id == dependency)
                    .unwrap_or_else(|| panic!("{} 依赖不存在的插件 {dependency}", plugin.id()));
                assert!(
                    dependency_index < index,
                    "{} 必须排在 {} 之后，实际顺序: {ids:?}",
                    plugin.id(), dependency
                );
            }
        }
    }

    /// 没被声明的插件，`host_managed` 是空数组而不是缺键 ——
    /// 前端可以无条件 `.includes()`，不必先判空。
    #[test]
    fn metadata_always_carries_a_host_managed_array() {
        for m in plugin_metadata() {
            assert!(m["host_managed"].is_array(), "{} 的 host_managed 不是数组", m["id"]);
        }
        let meta = plugin_metadata();
        let webhook = meta.iter().find(|m| m["id"] == serde_json::json!("webhook")).unwrap();
        assert_eq!(webhook["host_managed"], serde_json::json!([]));
    }

    /// 显示名必须唯一：两个插件同名，设置页上用户分不清在关哪个。
    #[test]
    fn plugin_labels_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in builtin_plugins() {
            assert!(seen.insert(p.label()), "重复的插件显示名: {}", p.label());
        }
    }

    #[test]
    fn build_registry_applies_user_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("short_segment".to_string(), serde_json::json!({"min_ms": 400}));
        let hooks = build_registry(&builtin_plugins(), &overrides, &PluginContext::new());
        let short = DomainEvent::Segment {
            speaker_id: 0, speaker_label: "我".into(), speaker_attribution: None, text: "喂".into(),
            is_partial: false, ts_ms: 0, duration_ms: 200, rms: 0.1,
            revision: 0, start_sample: 0, end_sample: 3200,
        };
        assert!(hooks.apply_filters(short).is_none(), "200ms < 覆盖后的 400ms 应被吞");
    }
}
