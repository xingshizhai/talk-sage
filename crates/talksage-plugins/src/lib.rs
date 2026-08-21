//! TalkSage v2 会议辅助插件。
//!
//! `SegmentObserver` 抽象：对一条最终转写段做触发判断 + 生成领域事件。
//! - `skeleton`：本地即时（骨架，同步发）；`run`：可含 LLM 调用（pipeline 在独立线程执行）。
//! - 依赖（知识库/LLM）经 `PluginContext` 注入（Arc 共享），插件不自行持有。

pub mod brief_retriever;
pub mod term_explainer;
pub mod translator;
pub mod registry;
pub mod short_segment;
pub mod cross_stream_dedup;
pub mod builtin;
pub mod conversation_metrics;
pub mod session_quality;
pub mod webhook;

pub use builtin::{build_registry, builtin_plugins};

pub use registry::{
    EventFilter, FinalizeContext, FinalizeReport, HookRegistry, Plugin, PluginConfig,
    SegmentObserver, SessionFinalizer,
};
pub use session_quality::QualityDeps;
pub use webhook::WebhookDeps;


use std::sync::Arc;

use talksage_knowledge::KnowledgeBase;
use talksage_llm::LLMProvider;

/// 插件执行上下文（Arc 共享，可跨线程）。
///
/// 承载**宿主能力**：插件自己造不出、也不该自己持有的东西。observer 在
/// `run` 里用 `kb`/`llm`，finalizer 在 `register` 时从这里取 `quality`/`webhook`。
///
/// `quality` / `webhook` 是 trait 对象而非具体类型：talksage-plugins 不依赖
/// talksage-session / talksage-config，依赖方向必须保持「pipeline 实现、
/// plugins 定义」，否则插件层会被拖进宿主的持久化与配置细节。
#[derive(Clone)]
pub struct PluginContext {
    pub kb: Option<Arc<KnowledgeBase>>,
    pub llm: Option<Arc<dyn LLMProvider>>,
    /// 会话质量评估（写 sessions.meta）。None = 无宿主，finalizer 静默跳过。
    pub quality: Option<Arc<dyn QualityDeps>>,
    /// 会后 webhook 推送。None = 无宿主，finalizer 静默跳过。
    pub webhook: Option<Arc<dyn WebhookDeps>>,
}

impl PluginContext {
    pub fn new() -> Self {
        Self { kb: None, llm: None, quality: None, webhook: None }
    }
}

impl Default for PluginContext {
    fn default() -> Self {
        Self::new()
    }
}
