//! 术语解释插件：客户英文段中的缩写 → LLM 中文解释。
//! 触发条件：speaker=client（英文客户）且含未见缩写，且冷却期已过。
//! 流程：`skeleton` 先发 `NPI = …`（同步）；`run` 调 LLM 后发最终解释。

use std::time::{SystemTime, UNIX_EPOCH};

use talksage_core::{DomainEvent, ResultStatus, TranscriptSegment};

use super::{SegmentObserver, PluginContext};

const SYSTEM_PROMPT: &str = "你是硬件制造业与商务谈判领域的专家助手。用户在与英文客户洽谈。\
    请解释对话中出现的专业术语/缩写。简洁，中文，每行格式：缩写 = 中文全称（英文全称），一句话说明含义。";

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// 提取 2+ 位连续大写字母的缩写（前后非字母数字边界）。
/// 若文本几乎全大写（ASR 英文输出风格），视为正常文本而非缩写集合。
pub fn find_acronyms(text: &str) -> Vec<String> {
    let uppercase = text.chars().filter(|c| c.is_ascii_uppercase()).count();
    let alphabetic = text.chars().filter(|c| c.is_alphabetic()).count();
    if alphabetic > 0 && uppercase as f64 / alphabetic as f64 > 0.7 {
        return Vec::new(); // 全大写输出（如 zipformer-en），非缩写
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
            if prev_ok && next_ok && !out.iter().any(|w| w == word) {
                out.push(word.to_string());
            }
        } else {
            j += 1;
        }
    }
    out
}

pub struct TermExplainerPlugin {
    cooldown_seconds: f64,
    seen: std::sync::Mutex<std::collections::HashSet<String>>,
    last_trigger_at: std::sync::Mutex<f64>,
    /// 最近骨架的 result_id（final 复用，前端按 id 原地更新）。
    pending_result_id: std::sync::Mutex<Option<String>>,
}

impl TermExplainerPlugin {
    pub fn new(cooldown_seconds: f64) -> Self {
        Self {
            cooldown_seconds,
            seen: std::sync::Mutex::new(std::collections::HashSet::new()),
            last_trigger_at: std::sync::Mutex::new(0.0),
            pending_result_id: std::sync::Mutex::new(None),
        }
    }

    fn unseen_acronyms(&self, text: &str) -> Vec<String> {
        let seen = self.seen.lock().unwrap();
        find_acronyms(text)
            .into_iter()
            .filter(|a| !seen.contains(a))
            .collect()
    }

    fn cooldown_active(&self) -> bool {
        let last = *self.last_trigger_at.lock().unwrap();
        self.cooldown_seconds > 0.0 && last > 0.0 && now() - last < self.cooldown_seconds
    }

    fn reserve(&self, acronyms: &[String]) {
        let mut seen = self.seen.lock().unwrap();
        for a in acronyms {
            seen.insert(a.clone());
        }
        *self.last_trigger_at.lock().unwrap() = now();
    }
}

impl SegmentObserver for TermExplainerPlugin {
    fn name(&self) -> &'static str {
        "term_explainer"
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        seg.speaker_id != 0 && !self.unseen_acronyms(&seg.text).is_empty() && !self.cooldown_active()
    }

    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let acronyms = self.unseen_acronyms(&seg.text);
        if acronyms.is_empty() {
            return Vec::new();
        }
        let result_id = format!("term-{}", now() as u64);
        *self.pending_result_id.lock().unwrap() = Some(result_id.clone());
        let content = if acronyms.len() == 1 {
            format!("{} = …", acronyms[0])
        } else {
            format!("{} = …", acronyms.join("、"))
        };
        vec![DomainEvent::Term {
            result_id,
            status: ResultStatus::Skeleton,
            content,
        }]
    }

    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> Option<DomainEvent> {
        let acronyms = find_acronyms(&seg.text);
        if acronyms.is_empty() {
            return None;
        }
        let llm = ctx.llm.as_ref()?;
        let prompt = format!("客户说：\"{}\"\n\n请解释其中出现的术语/缩写：{}", seg.text, acronyms.join(", "));
        let content = llm.complete(&prompt, SYSTEM_PROMPT).ok()?;
        if content.trim().is_empty() {
            return None;
        }
        self.reserve(&acronyms);
        // 复用骨架 result_id，前端按 id 原地更新
        let result_id = self.pending_result_id.lock().unwrap().take().unwrap_or_else(|| format!("term-{}", now() as u64));
        Some(DomainEvent::Term {
            result_id,
            status: ResultStatus::Final,
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
    fn finds_acronyms_in_text() {
        let found = find_acronyms("We need NPI and MOQ by Friday. ETA is next week.");
        assert!(found.contains(&"NPI".to_string()));
        assert!(found.contains(&"MOQ".to_string()));
        assert!(found.contains(&"ETA".to_string()));
    }

    #[test]
    fn ignores_all_uppercase_output() {
        // zipformer-en 全大写输出：不应被视为缩写
        assert!(find_acronyms("AFTER EARLY NIGHTFALL THE YELLOW LAMPS").is_empty());
    }

    #[test]
    fn ignores_non_client_segments() {
        let p = TermExplainerPlugin::new(0.0);
        assert!(!p.should_trigger(&seg(0, "NPI 是什么")));
    }

    #[test]
    fn triggers_and_emits_skeleton() {
        let p = TermExplainerPlugin::new(0.0);
        assert!(p.should_trigger(&seg(1, "We need NPI samples")));
        let mut skels = p.skeleton(&seg(1, "We need NPI samples"));
        assert!(!skels.is_empty(), "应有骨架");
        match skels.remove(0) {
            DomainEvent::Term { status: ResultStatus::Skeleton, content, .. } => {
                assert!(content.contains("NPI"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn run_reserves_acronyms_for_dedup() {
        let mock = MockProvider { response: "NPI = New Product Introduction".into() };
        let ctx = PluginContext {
            kb: None,
            llm: Some(std::sync::Arc::new(mock)),
        };
        let p = TermExplainerPlugin::new(0.0);
        let _ = p.run(&seg(1, "We need NPI samples"), &ctx);
        // 会话去重：run 后同缩写不再触发
        assert!(!p.should_trigger(&seg(1, "NPI again")));
    }

    #[test]
    fn run_uses_ctx_llm_for_final() {
        let mock = MockProvider { response: "NPI = New Product Introduction（新产品导入）".into() };
        let ctx = PluginContext {
            kb: None,
            llm: Some(std::sync::Arc::new(mock)),
        };
        let p = TermExplainerPlugin::new(0.0);
        match p.run(&seg(1, "NPI status?"), &ctx) {
            Some(DomainEvent::Term { status: ResultStatus::Final, content, .. }) => {
                assert!(content.contains("NPI"));
            }
            other => panic!("应有 final: {other:?}"),
        }
    }
}
