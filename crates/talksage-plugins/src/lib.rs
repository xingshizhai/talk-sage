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

pub use builtin::{build_registry, builtin_plugins};

pub use registry::{EventFilter, HookRegistry, Plugin, PluginConfig, SegmentObserver};

/// 过渡别名：老代码仍可用 AnalyzerPlugin 这个名字。
/// 阶段 3 迁移完 observer 后删除。
pub use registry::SegmentObserver as AnalyzerPlugin;

use std::sync::Arc;

use talksage_knowledge::KnowledgeBase;
use talksage_llm::LLMProvider;

/// 插件执行上下文（Arc 共享，可跨线程）。
#[derive(Clone)]
pub struct PluginContext {
    pub kb: Option<Arc<KnowledgeBase>>,
    pub llm: Option<Arc<dyn LLMProvider>>,
}

impl PluginContext {
    pub fn new() -> Self {
        Self { kb: None, llm: None }
    }
}

impl Default for PluginContext {
    fn default() -> Self {
        Self::new()
    }
}
