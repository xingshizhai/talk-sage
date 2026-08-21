//! 实时翻译插件：客户英文 → 中文；用户中文 → 英文。

use std::time::{SystemTime, UNIX_EPOCH};

use talksage_core::{DomainEvent, ResultStatus, TranscriptSegment, TranslationDirection};

use super::{SegmentObserver, PluginContext};

const SYSTEM_PROMPT: &str = "你是商务会议同声翻译。把用户输入翻译成目标语言，只输出译文，不要解释。";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 实时翻译插件（无内部状态）。
pub struct TranslatorPlugin;

impl TranslatorPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TranslatorPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl SegmentObserver for TranslatorPlugin {
    fn name(&self) -> &'static str {
        "translator"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        !seg.text.trim().is_empty()
    }

    fn skeleton(&self, _seg: &TranscriptSegment) -> Vec<DomainEvent> {
        Vec::new()
    }

    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent> {
        let llm = ctx.llm.as_ref()?;
        let direction = if seg.speaker_id != 0 {
            TranslationDirection::EnZh
        } else {
            TranslationDirection::ZhEn
        };
        let target = match direction {
            TranslationDirection::EnZh => "简体中文",
            TranslationDirection::ZhEn => "English",
        };
        let prompt = format!("目标语言：{target}\n\n{}", seg.text);
        let content = llm.complete(&prompt, SYSTEM_PROMPT).ok()?;
        if content.trim().is_empty() {
            return None;
        }
        Some(DomainEvent::Translation {
            result_id: format!("trans-{}", now_ms()),
            status: ResultStatus::Final,
            direction,
            content: content.trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::TranscriptSegment;
    use talksage_llm::MockProvider;

    fn seg(speaker: u32, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: speaker,
            speaker_label: if speaker == 1 { "客户" } else { "我" }.into(),
            text: text.into(),
            is_partial: false,
            ts_ms: 0,
            duration_ms: 500,
            rms: 0.2,
        }
    }

    #[test]
    fn en_to_zh_direction() {
        let mock = MockProvider { response: "我们需要 NPI 样品".into() };
        let ctx = PluginContext { kb: None, llm: Some(std::sync::Arc::new(mock)), ..PluginContext::new() };
        let p = TranslatorPlugin::new();
        assert!(p.should_trigger(&seg(1, "We need NPI samples")));
        match p.run(&seg(1, "We need NPI samples"), &ctx) {
            Some(DomainEvent::Translation { direction: TranslationDirection::EnZh, content, .. }) => {
                assert!(!content.is_empty());
            }
            other => panic!("应有 EnZh 翻译事件: {other:?}"),
        }
    }

    #[test]
    fn zh_to_en_direction() {
        let mock = MockProvider { response: "We need NPI samples".into() };
        let ctx = PluginContext { kb: None, llm: Some(std::sync::Arc::new(mock)), ..PluginContext::new() };
        let p = TranslatorPlugin::new();
        match p.run(&seg(0, "我们需要 NPI 样品"), &ctx) {
            Some(DomainEvent::Translation { direction: TranslationDirection::ZhEn, .. }) => {}
            other => panic!("应有 ZhEn 翻译事件: {other:?}"),
        }
    }

    #[test]
    fn no_llm_means_no_translation() {
        let p = TranslatorPlugin::new();
        let ctx = PluginContext { kb: None, llm: None, ..PluginContext::new() };
        assert!(p.run(&seg(1, "We need NPI"), &ctx).is_none());
    }
}
