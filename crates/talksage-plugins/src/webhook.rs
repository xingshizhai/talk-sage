//! webhook：会话结束时把结论推送到外部地址。
//!
//! 默认关闭 —— 它会把会话内容发到外部地址，
//! 不能因为「新增了插件」就默认开启。
//! 实际启用仍由 [webhooks] 配置决定（见 Task 4 的注入）。

use std::sync::Arc;

use serde_json::json;

use crate::registry::{FinalizeContext, HookRegistry, Plugin, PluginConfig, SessionFinalizer};

/// webhook 推送所需的宿主能力。由 pipeline 侧实现并注入。
pub trait WebhookDeps: Send + Sync {
    /// 推送会话结论到外部地址。
    fn push(&self, session_id: i64, quality: Option<&str>) -> anyhow::Result<()>;
}

/// 依赖在构造时注入 —— FinalizeContext 刻意不带 SessionStore，
/// 避免插件通过 context 改库，破坏「会后只读」。
pub struct WebhookFinalizer {
    pub deps: Option<Arc<dyn WebhookDeps>>,
}

impl SessionFinalizer for WebhookFinalizer {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn finalize(&self, ctx: &FinalizeContext) -> anyhow::Result<()> {
        let Some(deps) = &self.deps else {
            return Ok(()); // 无依赖（如单测）时静默跳过
        };
        deps.push(ctx.session_id, ctx.quality)?;
        log::info!("会话 #{} webhook 推送完成", ctx.session_id);
        Ok(())
    }
}

pub struct WebhookPlugin;

impl Plugin for WebhookPlugin {
    fn id(&self) -> &'static str {
        "webhook"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": false }))
    }

    fn register(&self, _cfg: &PluginConfig, hooks: &mut HookRegistry) {
        hooks.add_finalizer(Arc::new(WebhookFinalizer { deps: None }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_one_finalizer() {
        let p = WebhookPlugin;
        assert_eq!(p.id(), "webhook");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &mut hooks);
        assert_eq!(hooks.finalizer_count(), 1);
    }

    /// webhook 默认关闭 —— 它会把会话内容发到外部地址，
    /// 不能因为「新增了插件」就默认开启。
    #[test]
    fn webhook_is_disabled_by_default() {
        assert!(!WebhookPlugin.default_config().enabled());
    }
}
