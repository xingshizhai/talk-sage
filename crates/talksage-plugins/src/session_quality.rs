//! session_quality：会话结束时评估质量并写入 sessions.meta。
//!
//! 迁移自 service.rs 的 finish()。必须排在 finalizer 链首位——
//! 它写入 FinalizeContext.quality，webhook 载荷要带这个结论。

use std::sync::Arc;

use serde_json::json;

use crate::registry::{FinalizeContext, HookRegistry, Plugin, PluginConfig, SessionFinalizer};
use crate::PluginContext;

/// 质量评估所需的宿主能力。由 pipeline 侧实现并注入。
///
/// 只暴露「评估并写入」这一个动作，不暴露 SessionStore ——
/// 插件能改的只有这一处 meta，改不了别的。
pub trait QualityDeps: Send + Sync {
    /// 评估并写入 meta，返回质量标签（如 "clean" / "noise"）。
    /// `Ok(None)` = 本次没有可评估的数据（如全程无统计），不算失败。
    fn evaluate_and_store(&self, session_id: i64) -> anyhow::Result<Option<String>>;
}

/// 依赖在 `register` 时从 `PluginContext` 取出并装进来 —— FinalizeContext
/// 刻意不带 SessionStore，避免插件通过 context 改库，破坏「会后只读」。
pub struct SessionQualityFinalizer {
    pub deps: Option<Arc<dyn QualityDeps>>,
}

impl SessionFinalizer for SessionQualityFinalizer {
    fn name(&self) -> &'static str {
        "session_quality"
    }

    fn finalize(&self, ctx: &FinalizeContext) -> anyhow::Result<()> {
        let Some(deps) = &self.deps else {
            return Ok(()); // 无依赖（如单测）时静默跳过
        };
        match deps.evaluate_and_store(ctx.session_id)? {
            Some(label) => log::info!("会话 #{} 质量评估: {label}", ctx.session_id),
            None => log::debug!("会话 #{} 无流统计，跳过质量评估", ctx.session_id),
        }
        Ok(())
    }
}

pub struct SessionQualityPlugin;

impl Plugin for SessionQualityPlugin {
    fn id(&self) -> &'static str {
        "session_quality"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true }))
    }

    fn register(&self, _cfg: &PluginConfig, ctx: &PluginContext, hooks: &mut HookRegistry) {
        hooks.add_finalizer(Arc::new(SessionQualityFinalizer {
            deps: ctx.quality.clone(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeQuality(Arc<AtomicUsize>);
    impl QualityDeps for FakeQuality {
        fn evaluate_and_store(&self, _session_id: i64) -> anyhow::Result<Option<String>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Some("clean".into()))
        }
    }

    #[test]
    fn plugin_registers_one_finalizer() {
        let p = SessionQualityPlugin;
        assert_eq!(p.id(), "session_quality");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &PluginContext::new(), &mut hooks);
        assert_eq!(hooks.finalizer_count(), 1);
        assert_eq!(hooks.observers().len(), 0, "本插件不挂 observer");
    }

    #[test]
    fn default_config_has_enabled() {
        assert!(SessionQualityPlugin.default_config().enabled());
    }

    /// 注入的依赖必须真的被调用 —— 这是 register 拿 ctx 的全部理由。
    #[test]
    fn injected_deps_are_called_on_finalize() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ctx = PluginContext {
            quality: Some(Arc::new(FakeQuality(calls.clone()))),
            ..PluginContext::new()
        };
        let mut hooks = HookRegistry::default();
        SessionQualityPlugin.register(&SessionQualityPlugin.default_config(), &ctx, &mut hooks);
        let report = hooks.run_finalizers(&FinalizeContext { session_id: 7, quality: None });
        assert!(report.failed.is_empty());
        assert_eq!(calls.load(Ordering::Relaxed), 1, "注入的 QualityDeps 应被调用");
    }

    /// 无宿主时静默跳过：单测 / offline 管道不该因为缺依赖而报失败。
    #[test]
    fn without_deps_it_is_a_no_op() {
        let mut hooks = HookRegistry::default();
        SessionQualityPlugin.register(
            &SessionQualityPlugin.default_config(),
            &PluginContext::new(),
            &mut hooks,
        );
        let report = hooks.run_finalizers(&FinalizeContext { session_id: 7, quality: None });
        assert!(report.failed.is_empty(), "没有依赖不等于失败");
    }
}
