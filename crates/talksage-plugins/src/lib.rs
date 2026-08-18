//! TalkSage v2 会议辅助插件。
//!
//! `AnalyzerPlugin` 抽象：对一条最终转写段做触发判断 + 生成领域事件。
//! - `skeleton`：本地即时（骨架，同步发）；`run`：可含 LLM 调用（pipeline 在独立线程执行）。
//! - 依赖（知识库/LLM）经 `PluginContext` 注入（Arc 共享），插件不自行持有。

pub mod brief_retriever;
pub mod term_explainer;
pub mod translator;

use std::sync::Arc;

use talksage_core::{DomainEvent, TranscriptSegment};
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

/// 插件抽象。
pub trait AnalyzerPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn should_trigger(&self, seg: &TranscriptSegment) -> bool;
    /// 本地即时骨架（无 LLM），同步发出。
    fn skeleton(&self, seg: &TranscriptSegment) -> Option<DomainEvent>;
    /// 完整执行（可含 LLM），pipeline 在独立线程调用。
    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent>;
}
