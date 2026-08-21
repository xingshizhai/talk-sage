//! session_quality：会话结束时评估质量并写入 sessions.meta。
//!
//! 迁移自 service.rs 的 finish()。必须排在 finalizer 链首位——
//! 它写入 FinalizeContext.quality，webhook 载荷要带这个结论。

use std::sync::Arc;

use serde_json::json;

use crate::registry::{FinalizeContext, HookRegistry, Plugin, PluginConfig, SessionFinalizer};

/// 质量评估所需的宿主能力。由 pipeline 侧实现并注入。
pub trait QualityDeps: Send + Sync {
    /// 评估并写入 meta，返回质量标签（如 "clean" / "noise"）。
    fn evaluate_and_store(&self, session_id: i64) -> anyhow::Result<String>;
}

/// 依赖在构造时注入 —— FinalizeContext 刻意不带 SessionStore，
/// 避免插件通过 context 改库，破坏「会后只读」。
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
        let label = deps.evaluate_and_store(ctx.session_id)?;
        log::info!("会话 #{} 质量评估: {label}", ctx.session_id);
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

    fn register(&self, _cfg: &PluginConfig, hooks: &mut HookRegistry) {
        hooks.add_finalizer(Arc::new(SessionQualityFinalizer { deps: None }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_one_finalizer() {
        let p = SessionQualityPlugin;
        assert_eq!(p.id(), "session_quality");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &mut hooks);
        assert_eq!(hooks.finalizer_count(), 1);
        assert_eq!(hooks.observers().len(), 0, "本插件不挂 observer");
    }

    #[test]
    fn default_config_has_enabled() {
        assert!(SessionQualityPlugin.default_config().enabled());
    }
}
