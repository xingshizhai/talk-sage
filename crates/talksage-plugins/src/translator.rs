//! 实时翻译插件：客户英文 → 中文；用户中文 → 英文。

use std::time::{SystemTime, UNIX_EPOCH};

use talksage_core::{DomainEvent, ResultStatus, TranscriptSegment, TranslationDirection};

use super::{LiveTranslationMode, PluginContext, SegmentObserver};

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
        let policy = ctx.translation.as_ref()?;
        let is_client = seg.speaker_id != 0;
        if policy.mode == LiveTranslationMode::Off
            || (policy.mode == LiveTranslationMode::ClientToUser && !is_client)
        {
            return None;
        }
        let (source, target_language) = if is_client {
            (policy.client_language.as_str(), policy.user_language.as_str())
        } else {
            (policy.user_language.as_str(), policy.client_language.as_str())
        };
        let direction = match (source, target_language) {
            ("en", "zh") => TranslationDirection::EnZh,
            ("zh", "en") => TranslationDirection::ZhEn,
            _ => return None,
        };
        let target = match target_language {
            "zh" => "简体中文",
            "en" => "English",
            _ => return None,
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

/// 注册表条目。
pub struct TranslatorPluginDef;

impl crate::registry::Plugin for TranslatorPluginDef {
    fn id(&self) -> &'static str {
        "translator"
    }

    fn label(&self) -> &'static str {
        "实时翻译"
    }

    fn default_config(&self) -> crate::registry::PluginConfig {
        // `cooldown_seconds` 保留只为不让用户已有的 [plugins.translator] 配置
        // 突然消失 —— `TranslatorPlugin::new()` 不接受参数，这个值迁移前就
        // 没被读过。register() 刻意不读它：读了就是行为变更（翻译触发频率）。
        crate::registry::PluginConfig::from_value(serde_json::json!({
            "enabled": true,
            "cooldown_seconds": 3.0,
        }))
    }

    fn register(
        &self,
        _cfg: &crate::registry::PluginConfig,
        _ctx: &PluginContext,
        hooks: &mut crate::registry::HookRegistry,
    ) {
        hooks.add_observer(std::sync::Arc::new(TranslatorPlugin::new()));
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
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms: 0,
            duration_ms: 500,
            rms: 0.2,
        }
    }

    fn bilingual_ctx(response: &str) -> PluginContext {
        PluginContext {
            llm: Some(std::sync::Arc::new(MockProvider { response: response.into() })),
            translation: Some(crate::LiveTranslationPolicy {
                mode: LiveTranslationMode::Bidirectional,
                user_language: "zh".into(),
                client_language: "en".into(),
            }),
            ..PluginContext::new()
        }
    }

    #[test]
    fn en_to_zh_direction() {
        let ctx = bilingual_ctx("我们需要 NPI 样品");
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
        let ctx = bilingual_ctx("We need NPI samples");
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

    #[test]
    fn one_way_ignores_user_stream() {
        let mut ctx = bilingual_ctx("translated");
        ctx.translation.as_mut().unwrap().mode = LiveTranslationMode::ClientToUser;
        let p = TranslatorPlugin::new();
        assert!(p.run(&seg(0, "中文"), &ctx).is_none());
        assert!(p.run(&seg(1, "English"), &ctx).is_some());
    }
}
