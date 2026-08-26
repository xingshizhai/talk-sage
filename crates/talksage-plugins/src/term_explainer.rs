//! 术语解释插件：从转写文本中识别专业术语并用 LLM 解释。
//!
//! 两种触发模式（可同时生效）：
//! 1. `llm_extract`（默认开）：LLM 自动识别段落中的专业术语/行业词汇
//! 2. `user_terms`：用户指定关注词（逗号/换行分隔），出现即优先解释
//!
//! 冷却计时器：`llm_extract` 模式受 `cooldown_seconds` 约束，避免每段都调 LLM。
//! `user_terms` 命中不受冷却影响，但同一词在本次会话里只解释一次（去重）。

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use talksage_core::{DomainEvent, ResultStatus, TranscriptSegment};
use talksage_llm::render_prompt;

use super::{prompts, PluginContext, SegmentObserver};

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// 从逗号/换行分隔的字符串解析词条列表（去空格、去重、过滤空串）。
fn parse_user_terms(raw: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    raw.split([',', '\n', '；', '，'])
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty() && seen.insert(t.clone()))
        .collect()
}

/// 在 text 中查找 user_terms 里出现的词，保持原大小写。
fn matched_user_terms(text: &str, terms: &[String]) -> Vec<String> {
    let lower = text.to_lowercase();
    terms
        .iter()
        .filter(|t| lower.contains(t.to_lowercase().as_str()))
        .cloned()
        .collect()
}

// ── Plugin 状态 ──────────────────────────────────────────────────────────────

pub struct TermExplainerPlugin {
    cooldown_seconds: f64,
    llm_extract: bool,
    min_chars: usize,
    user_terms: Vec<String>,
    /// 已解释过的用户词（会话级去重）。
    explained_terms: Mutex<HashSet<String>>,
    last_llm_trigger_at: Mutex<f64>,
    pending_result_id: Mutex<Option<String>>,
}

impl TermExplainerPlugin {
    pub fn new(cooldown_seconds: f64, llm_extract: bool, min_chars: usize, user_terms_raw: &str) -> Self {
        Self {
            cooldown_seconds,
            llm_extract,
            min_chars,
            user_terms: parse_user_terms(user_terms_raw),
            explained_terms: Mutex::new(HashSet::new()),
            last_llm_trigger_at: Mutex::new(0.0),
            pending_result_id: Mutex::new(None),
        }
    }

    fn cooldown_active(&self) -> bool {
        let last = *self.last_llm_trigger_at.lock().unwrap();
        self.cooldown_seconds > 0.0 && last > 0.0 && now_secs() - last < self.cooldown_seconds
    }

    fn new_user_terms_in(&self, text: &str) -> Vec<String> {
        let explained = self.explained_terms.lock().unwrap();
        matched_user_terms(text, &self.user_terms)
            .into_iter()
            .filter(|t| !explained.contains(t))
            .collect()
    }

    fn mark_explained(&self, terms: &[String]) {
        let mut explained = self.explained_terms.lock().unwrap();
        for t in terms {
            explained.insert(t.clone());
        }
    }
}

// ── SegmentObserver ──────────────────────────────────────────────────────────

impl SegmentObserver for TermExplainerPlugin {
    fn name(&self) -> &'static str {
        "term_explainer"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        if seg.is_partial || seg.text.trim().is_empty() {
            return false;
        }
        // 用户词命中：不受冷却
        if !self.new_user_terms_in(&seg.text).is_empty() {
            return true;
        }
        // LLM 主动提取：受冷却 + 最短字数
        self.llm_extract
            && !self.cooldown_active()
            && seg.text.trim().chars().count() >= self.min_chars
    }

    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let result_id = format!("term-{}", now_secs() as u64);
        *self.pending_result_id.lock().unwrap() = Some(result_id.clone());

        // 用户词命中时骨架里提示词名，增强反馈感
        let pinned = self.new_user_terms_in(&seg.text);
        let content = if !pinned.is_empty() {
            format!("{} = …", pinned.join("、"))
        } else {
            "术语识别中…".to_string()
        };
        vec![DomainEvent::Term {
            result_id,
            status: ResultStatus::Skeleton,
            content,
        }]
    }

    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> anyhow::Result<Option<DomainEvent>> {
        let Some(llm) = ctx.llm.as_ref() else {
            return Ok(None);
        };
        let pinned = self.new_user_terms_in(&seg.text);
        let pinned_section = if pinned.is_empty() {
            String::new()
        } else {
            format!("\n用户关注词（必须解释）：{}\n", pinned.join("、"))
        };
        let prompt = render_prompt(
            prompts::TERM_EXPLAINER_USER,
            &[("text", seg.text.trim()), ("pinned_section", &pinned_section)],
        );
        let content = llm.complete(&prompt, prompts::TERM_EXPLAINER_SYSTEM)?;
        let content = content.trim().to_string();
        if content.is_empty() {
            return Ok(None);
        }
        // 标记用户词已解释；更新冷却（非用户词触发时才更新）
        if !pinned.is_empty() {
            self.mark_explained(&pinned);
        } else {
            *self.last_llm_trigger_at.lock().unwrap() = now_secs();
        }
        let result_id = self
            .pending_result_id
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| format!("term-{}", now_secs() as u64));
        Ok(Some(DomainEvent::Term {
            result_id,
            status: ResultStatus::Final,
            content,
        }))
    }
}

// ── Plugin 注册 ───────────────────────────────────────────────────────────────

pub struct TermExplainerPluginDef;

impl crate::registry::Plugin for TermExplainerPluginDef {
    fn descriptor(&self) -> &'static crate::PluginDescriptor {
        static D: crate::PluginDescriptor = crate::PluginDescriptor {
            id: "term_explainer",
            label: "术语解释",
            description: "LLM 识别专业术语并解释；支持用户自定义关注词",
            category: crate::PluginCategory::Analysis,
            phase: crate::PluginPhase::Observer,
            capabilities: &[crate::PluginCapability::Llm],
            host_managed: &[],
            after: &[],
        };
        &D
    }

    fn default_config(&self) -> crate::registry::PluginConfig {
        crate::registry::PluginConfig::from_value(serde_json::json!({
            "enabled": true,
            "llm_extract": true,
            "cooldown_seconds": 20.0,
            "min_chars": 15,
            "user_terms": "",
        }))
    }

    fn register(&self, cfg: &crate::registry::PluginConfig, _ctx: &PluginContext, hooks: &mut crate::registry::HookRegistry) {
        let cooldown = cfg.get_f64("cooldown_seconds", 20.0);
        let llm_extract = cfg.get_bool("llm_extract", true);
        let min_chars = cfg.get_u64("min_chars", 15) as usize;
        let user_terms = cfg.get_str("user_terms", "");
        hooks.add_observer(std::sync::Arc::new(TermExplainerPlugin::new(
            cooldown,
            llm_extract,
            min_chars,
            &user_terms,
        )));
    }
}

// ── 旧版 find_acronyms（保留供外部兼容）──────────────────────────────────────

/// 提取 2+ 位连续大写字母的缩写。保留以兼容任何直接引用它的测试/代码。
pub fn find_acronyms(text: &str) -> Vec<String> {
    let uppercase = text.chars().filter(|c| c.is_ascii_uppercase()).count();
    let alphabetic = text.chars().filter(|c| c.is_alphabetic()).count();
    if alphabetic > 0 && uppercase as f64 / alphabetic as f64 > 0.7 {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut j = 0;
    while j < bytes.len() {
        if bytes[j].is_ascii_uppercase() {
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_uppercase() {
                j += 1;
            }
            let word = &text[start..j];
            let prev_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let next_ok = j >= bytes.len() || !bytes[j].is_ascii_alphanumeric();
            if prev_ok && next_ok && word.len() >= 2 && !out.iter().any(|w| w == word) {
                out.push(word.to_string());
            }
        } else {
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::TranscriptSegment;
    use talksage_llm::MockProvider;

    fn seg(text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: 1,
            speaker_label: "客户".into(),
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms: 0,
            duration_ms: 500,
            rms: 0.2,
        }
    }

    #[test]
    fn parse_user_terms_handles_various_separators() {
        let terms = parse_user_terms("API, REST\nGraphQL；gRPC，SDK");
        assert!(terms.contains(&"API".to_string()));
        assert!(terms.contains(&"REST".to_string()));
        assert!(terms.contains(&"GraphQL".to_string()));
        assert!(terms.contains(&"gRPC".to_string()));
        assert!(terms.contains(&"SDK".to_string()));
    }

    #[test]
    fn user_terms_trigger_without_cooldown() {
        let p = TermExplainerPlugin::new(999.0, false, 5, "NPI, MOQ");
        assert!(p.should_trigger(&seg("We need NPI samples")));
        assert!(!p.should_trigger(&seg("How are you today")));
    }

    #[test]
    fn user_terms_deduplicate_across_session() {
        let p = TermExplainerPlugin::new(0.0, false, 5, "NPI");
        assert!(p.should_trigger(&seg("We need NPI")));
        p.mark_explained(&["NPI".to_string()]);
        assert!(!p.should_trigger(&seg("NPI status?")));
    }

    #[test]
    fn llm_extract_respects_cooldown() {
        let p = TermExplainerPlugin::new(999.0, true, 5, "");
        assert!(p.should_trigger(&seg("请讨论系统架构设计方案")));
        *p.last_llm_trigger_at.lock().unwrap() = now_secs();
        assert!(!p.should_trigger(&seg("另一段关于架构设计的内容")));
    }

    #[test]
    fn llm_extract_respects_min_chars() {
        let p = TermExplainerPlugin::new(0.0, true, 20, "");
        assert!(!p.should_trigger(&seg("短段落"))); // < 20 chars
        assert!(p.should_trigger(&seg("这是一段较长的关于系统架构设计的讨论内容")));
    }

    #[test]
    fn skeleton_includes_pinned_terms_if_matched() {
        let p = TermExplainerPlugin::new(0.0, false, 5, "NPI");
        let events = p.skeleton(&seg("We need NPI samples"));
        match &events[0] {
            DomainEvent::Term { status: ResultStatus::Skeleton, content, .. } => {
                assert!(content.contains("NPI"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn run_calls_llm_and_returns_final_term() {
        let mock = MockProvider { response: "NPI：New Product Introduction，新产品导入流程".into() };
        let ctx = PluginContext { kb: None, llm: Some(std::sync::Arc::new(mock)), ..PluginContext::new() };
        let p = TermExplainerPlugin::new(0.0, true, 5, "");
        match p.run(&seg("We need NPI samples"), &ctx).unwrap() {
            Some(DomainEvent::Term { status: ResultStatus::Final, content, .. }) => {
                assert!(content.contains("NPI"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn find_acronyms_still_works() {
        let found = find_acronyms("We need NPI and MOQ by Friday.");
        assert!(found.contains(&"NPI".to_string()));
        assert!(found.contains(&"MOQ".to_string()));
    }
}
