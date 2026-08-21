//! webhook：会话结束时把结论推送到外部地址。
//!
//! 默认关闭 —— 它会把会话内容发到外部地址，
//! 不能因为「新增了插件」就默认开启。
//! 实际启用仍由 [webhooks] 配置决定（见 Task 4 的注入）。

use std::sync::Arc;

use serde_json::json;

use crate::registry::{FinalizeContext, HookRegistry, Plugin, PluginConfig, SessionFinalizer};
use crate::PluginContext;

/// webhook 推送所需的宿主能力。由 pipeline 侧实现并注入。
///
/// **第二道闸在实现里**：本插件的 `enabled` 只决定 finalizer 装不装，
/// 真正发不发请求由宿主在 `push` 里按 `[webhooks]` 配置再判一次。
/// 两道闸互不代替 —— 配置是会后现取的，装载期的开关看不到它。
pub trait WebhookDeps: Send + Sync {
    /// 推送会话结论到外部地址。载荷由宿主用 `session_id` 现取 —— 其中的质量
    /// meta 已由链上游的 `session_quality` 写进会话行。
    ///
    /// 宿主可自行决定异步执行，所以 `Ok(())` 只表示**已派发**，不表示已送达。
    fn push(&self, session_id: i64) -> anyhow::Result<()>;
}

/// 依赖在 `register` 时从 `PluginContext` 取出并装进来 —— FinalizeContext
/// 刻意不带 SessionStore，避免插件通过 context 改库，破坏「会后只读」。
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
        deps.push(ctx.session_id)?;
        // 「已交付宿主」而非「已送达」：宿主通常异步发，失败进不了 FinalizeReport。
        log::debug!("会话 #{} webhook 推送已交付宿主", ctx.session_id);
        Ok(())
    }
}

pub struct WebhookPlugin;

impl Plugin for WebhookPlugin {
    fn descriptor(&self) -> &'static crate::PluginDescriptor {
        static D: crate::PluginDescriptor = crate::PluginDescriptor {
            id: "webhook", label: "会议结束推送",
            description: "会话完成后向已配置的外部端点推送结果",
            category: crate::PluginCategory::Infrastructure, phase: crate::PluginPhase::Finalizer,
            capabilities: &[crate::PluginCapability::Webhook], host_managed: &[], after: &["session_quality"],
        };
        &D
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": false }))
    }

    fn register(&self, _cfg: &PluginConfig, ctx: &PluginContext, hooks: &mut HookRegistry) {
        hooks.add_finalizer(Arc::new(WebhookFinalizer {
            deps: ctx.webhook.clone(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeWebhook(Mutex<Vec<i64>>);
    impl WebhookDeps for FakeWebhook {
        fn push(&self, session_id: i64) -> anyhow::Result<()> {
            self.0.lock().unwrap().push(session_id);
            Ok(())
        }
    }

    #[test]
    fn plugin_registers_one_finalizer() {
        let p = WebhookPlugin;
        assert_eq!(p.id(), "webhook");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &PluginContext::new(), &mut hooks);
        assert_eq!(hooks.finalizer_count(), 1);
    }

    /// 注入的依赖必须真的被调用，并拿到会话 id —— 载荷由宿主凭它现取。
    #[test]
    fn injected_deps_receive_the_session_id() {
        let fake = Arc::new(FakeWebhook::default());
        let ctx = PluginContext { webhook: Some(fake.clone()), ..PluginContext::new() };
        let mut hooks = HookRegistry::default();
        WebhookPlugin.register(&WebhookPlugin.default_config(), &ctx, &mut hooks);
        let report = hooks.run_finalizers(&FinalizeContext { session_id: 9 });
        assert!(report.failed.is_empty());
        assert_eq!(*fake.0.lock().unwrap(), vec![9]);
    }

    #[test]
    fn without_deps_it_is_a_no_op() {
        let mut hooks = HookRegistry::default();
        WebhookPlugin.register(&WebhookPlugin.default_config(), &PluginContext::new(), &mut hooks);
        assert!(hooks
            .run_finalizers(&FinalizeContext { session_id: 9 })
            .failed
            .is_empty());
    }

    /// webhook 默认关闭 —— 它会把会话内容发到外部地址，
    /// 不能因为「新增了插件」就默认开启。
    #[test]
    fn webhook_is_disabled_by_default() {
        assert!(!WebhookPlugin.default_config().enabled());
    }
}
