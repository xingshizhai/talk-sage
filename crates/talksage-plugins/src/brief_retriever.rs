//! 简报检索插件：客户发言命中知识库 → 相关简报片段。

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
    last_trigger_at: std::sync::Mutex<f64>,
}

impl BriefRetrieverPlugin {
    pub fn new(cooldown_seconds: f64, min_score: f32) -> Self {
        Self {
            cooldown_seconds,
            min_score,
            last_trigger_at: std::sync::Mutex::new(0.0),
        }
    }
}

impl SegmentObserver for BriefRetrieverPlugin {
    fn name(&self) -> &'static str {
        "brief_retriever"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        if seg.speaker_id == 0 {
            return false; // 主人发言不检索简报（简报针对客户/其他说话人）
        }
        let last = *self.last_trigger_at.lock().unwrap();
        !(self.cooldown_seconds > 0.0 && last > 0.0 && now() - last < self.cooldown_seconds)
    }

    fn skeleton(&self, _seg: &TranscriptSegment) -> Vec<DomainEvent> {
        Vec::new()
    }

    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent> {
        let kb = ctx.kb.as_ref()?;
        if kb.chunk_count() == 0 {
            return None;
        }
        let hits = kb.search(&seg.text, 2, self.min_score);
        if hits.is_empty() {
            return None;
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
        Some(DomainEvent::Brief {
            source: hits[0].source.clone(),
            text: content,
        })
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
    fn id(&self) -> &'static str {
        "brief_retriever"
    }

    fn label(&self) -> &'static str {
        "简报检索"
    }

    fn default_config(&self) -> crate::registry::PluginConfig {
        // `min_score` 迁移前硬编码在 service.rs 的调用点（0.05），不在配置里；
        // 这里给出同一个值，行为不变。
        crate::registry::PluginConfig::from_value(serde_json::json!({
            "enabled": true,
            "cooldown_seconds": 15.0,
            "min_score": 0.05,
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

    #[test]
    fn retrieves_brief_on_kb_hit() {
        let dir = std::env::temp_dir().join(format!("talksage-brief-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("client.md"), "# 客户\n\n客户关注 NPI 样品交期与 MOQ 价格。").unwrap();
        let mut kb = KnowledgeBase::new();
        kb.index_folder(Path::new(&dir.to_string_lossy().to_string()));
        let ctx = PluginContext { kb: Some(Arc::new(kb)), llm: None, ..PluginContext::new() };
        let p = BriefRetrieverPlugin::new(0.0, 0.05);
        assert!(p.should_trigger(&seg(1, "NPI samples MOQ")));
        let ev = p.run(&seg(1, "NPI samples MOQ"), &ctx).expect("应有简报");
        match ev {
            DomainEvent::Brief { text, .. } => assert!(text.contains("NPI")),
            other => panic!("unexpected: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_hit_returns_none() {
        let ctx = PluginContext { kb: None, llm: None, ..PluginContext::new() };
        let p = BriefRetrieverPlugin::new(0.0, 0.05);
        assert!(p.run(&seg(1, "hello world"), &ctx).is_none());
    }
}
