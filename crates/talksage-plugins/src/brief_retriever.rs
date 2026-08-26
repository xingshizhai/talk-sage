//! 简报检索插件：发言命中知识库 → 相关简报片段。
//!
//! 默认跳过主人（`SpeakerRole::Owner`，无归属时回退 `speaker_id == 0`）。
//! 无客户流场景由宿主把 `include_user` 设为 true，检索主讲人。

use std::time::{SystemTime, UNIX_EPOCH};

use talksage_core::{DomainEvent, TranscriptSegment};

use super::{SegmentObserver, PluginContext};

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub struct BriefRetrieverPlugin {
    cooldown_seconds: f64,
    min_score: f32,
    include_user: bool,
    last_trigger_at: std::sync::Mutex<f64>,
}

impl BriefRetrieverPlugin {
    pub fn new(cooldown_seconds: f64, min_score: f32, include_user: bool) -> Self {
        Self {
            cooldown_seconds,
            min_score,
            include_user,
            last_trigger_at: std::sync::Mutex::new(0.0),
        }
    }
}

/// 是否为主人发言。有归属时看 role；没有或 Unknown 时回退 speaker_id == 0。
fn is_owner_speech(seg: &TranscriptSegment) -> bool {
    match seg.speaker_attribution.as_ref().map(|a| a.role) {
        Some(talksage_core::SpeakerRole::Owner) => true,
        Some(talksage_core::SpeakerRole::Client | talksage_core::SpeakerRole::Other) => false,
        Some(talksage_core::SpeakerRole::Unknown) | None => seg.speaker_id == 0,
    }
}

impl SegmentObserver for BriefRetrieverPlugin {
    fn name(&self) -> &'static str {
        "brief_retriever"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        if is_owner_speech(seg) && !self.include_user {
            return false; // 主人发言默认不检索；无客户流场景由 include_user 打开
        }
        let last = *self.last_trigger_at.lock().unwrap();
        !(self.cooldown_seconds > 0.0 && last > 0.0 && now() - last < self.cooldown_seconds)
    }

    fn skeleton(&self, _seg: &TranscriptSegment) -> Vec<DomainEvent> {
        Vec::new()
    }

    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> anyhow::Result<Option<DomainEvent>> {
        let Some(kb) = ctx.kb.as_ref() else { return Ok(None) };
        if kb.chunk_count() == 0 {
            return Ok(None);
        }
        let hits = kb.search(&seg.text, 2, self.min_score);
        if hits.is_empty() {
            return Ok(None);
        }
        *self.last_trigger_at.lock().unwrap() = now();
        let content = hits
            .iter()
            .map(|h| {
                let label = if h.heading.is_empty() { &h.source } else { &h.heading };
                format!("[{label}] {}", truncate(&h.text, 280))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(Some(DomainEvent::Brief {
            source: hits[0].source.clone(),
            text: content,
        }))
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

/// 注册表条目。
pub struct BriefRetrieverPluginDef;

impl crate::registry::Plugin for BriefRetrieverPluginDef {
    fn descriptor(&self) -> &'static crate::PluginDescriptor {
        static D: crate::PluginDescriptor = crate::PluginDescriptor {
            id: "brief_retriever", label: "简报检索",
            description: "从本地知识库检索与发言相关的简报；有客户流时跳过主人",
            category: crate::PluginCategory::Analysis, phase: crate::PluginPhase::Observer,
            capabilities: &[crate::PluginCapability::KnowledgeBase], host_managed: &["include_user"], after: &[],
        };
        &D
    }

    fn default_config(&self) -> crate::registry::PluginConfig {
        // `min_score` 迁移前硬编码在 service.rs 的调用点（0.05），不在配置里；
        // 这里给出同一个值，行为不变。
        crate::registry::PluginConfig::from_value(serde_json::json!({
            "enabled": true,
            "cooldown_seconds": 15.0,
            "min_score": 0.05,
            "include_user": false,
        }))
    }

    fn register(
        &self,
        cfg: &crate::registry::PluginConfig,
        ctx: &PluginContext,
        hooks: &mut crate::registry::HookRegistry,
    ) {
        // 原 service.rs 的 `&& kb.is_some()`：知识库没索引到内容时不装配，
        // 否则每段都会白跑一次检索。
        if ctx.kb.is_none() {
            return;
        }
        hooks.add_observer(std::sync::Arc::new(BriefRetrieverPlugin::new(
            cfg.get_f64("cooldown_seconds", 15.0),
            cfg.get_f64("min_score", 0.05) as f32,
            cfg.get_bool("include_user", false),
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use talksage_core::TranscriptSegment;
    use talksage_knowledge::KnowledgeBase;
    use std::sync::Arc;

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

    fn with_role(mut s: TranscriptSegment, role: talksage_core::SpeakerRole) -> TranscriptSegment {
        s.speaker_attribution = Some(talksage_core::SpeakerAttribution {
            source: talksage_core::AudioSource::Unknown,
            role,
            voice: None,
        });
        s
    }

    #[test]
    fn owner_role_does_not_trigger_even_on_client_stream_id() {
        let p = BriefRetrieverPlugin::new(0.0, 0.05, false);
        assert!(!p.should_trigger(&with_role(seg(1, "我在客户流上说话"), talksage_core::SpeakerRole::Owner)));
    }

    #[test]
    fn client_role_triggers_even_when_speaker_id_is_zero() {
        let p = BriefRetrieverPlugin::new(0.0, 0.05, false);
        assert!(p.should_trigger(&with_role(seg(0, "NPI samples"), talksage_core::SpeakerRole::Client)));
    }

    #[test]
    fn owner_triggers_when_include_user_is_enabled() {
        let p = BriefRetrieverPlugin::new(0.0, 0.05, true);
        assert!(p.should_trigger(&with_role(seg(0, "样品交期"), talksage_core::SpeakerRole::Owner)));
        assert!(p.should_trigger(&seg(0, "样品交期")), "无归属时 speaker_id=0 也应检索");
    }

    #[test]
    fn retrieves_brief_on_kb_hit() {
        let dir = std::env::temp_dir().join(format!("talksage-brief-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("client.md"), "# 客户\n\n客户关注 NPI 样品交期与 MOQ 价格。").unwrap();
        let mut kb = KnowledgeBase::new();
        kb.index_folder(Path::new(&dir.to_string_lossy().to_string()));
        let ctx = PluginContext { kb: Some(Arc::new(kb)), llm: None, ..PluginContext::new() };
        let p = BriefRetrieverPlugin::new(0.0, 0.05, false);
        assert!(p.should_trigger(&seg(1, "NPI samples MOQ")));
        let ev = p.run(&seg(1, "NPI samples MOQ"), &ctx).unwrap().expect("应有简报");
        match ev {
            DomainEvent::Brief { text, .. } => assert!(text.contains("NPI")),
            other => panic!("unexpected: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_hit_returns_none() {
        let ctx = PluginContext { kb: None, llm: None, ..PluginContext::new() };
        let p = BriefRetrieverPlugin::new(0.0, 0.05, false);
        assert!(p.run(&seg(1, "hello world"), &ctx).unwrap().is_none());
    }
}
