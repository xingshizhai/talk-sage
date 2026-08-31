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

/// 专业度门槛：低于这个分数的词对白领/工程师听众没有解释价值。
const MIN_LEVEL: u8 = 4;
/// 单段最多保留几条，避免一次刷屏。
const MAX_TERMS: usize = 2;

/// 模型经常一边解释一边自己承认"这不是专业术语"。这些词出现在解释里，
/// 说明它自己也知道不该收 —— 直接丢掉，比再问一次便宜。
const CASUAL_MARKERS: [&str; 8] = [
    "网络热梗", "网络流行", "流行语", "非专业", "俗语", "俚语", "口语表达", "常识",
];

/// 一条通过筛选的专业术语。
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedTerm {
    pub term: String,
    /// 模型自评的专业度 1–5。
    pub level: u8,
    pub explanation: String,
}

/// 解析并筛选 LLM 输出。每行 `术语 | 专业度 | 解释`。
///
/// prompt 已经写明了收录标准，但模型并不总听话（线上见过「心率」「购物车」
/// 「天猫超市」），所以这里再卡一道硬门槛：专业度不够的、解释里自述不专业的，
/// 一律丢弃。用户关注词是用户明确点名要的，不受专业度门槛限制。
pub fn parse_terms(raw: &str, pinned: &[String]) -> Vec<ExtractedTerm> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim().trim_start_matches(['-', '*', '•']).trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(['|', '｜']).map(str::trim).collect();
        // 三段式是唯一被接受的格式：拿不到专业度就没法判断该不该收，宁可不收
        if parts.len() < 3 {
            continue;
        }
        let term = parts[0].trim_matches(['「', '」', '"', '“', '”']).trim().to_string();
        let explanation = parts[2..].join(" | ").trim().to_string();
        if term.is_empty() || explanation.is_empty() {
            continue;
        }
        let level: u8 = parts[1]
            .chars()
            .find(|c| c.is_ascii_digit())
            .and_then(|c| c.to_digit(10))
            .unwrap_or(0) as u8;
        let is_pinned = pinned
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&term) || term.to_lowercase().contains(&p.to_lowercase()));
        if !is_pinned {
            if level < MIN_LEVEL {
                continue;
            }
            if CASUAL_MARKERS.iter().any(|m| explanation.contains(m)) {
                continue;
            }
        }
        out.push(ExtractedTerm { term, level, explanation });
        if out.len() >= MAX_TERMS {
            break;
        }
    }
    out
}

/// 手动查询单个术语：用户在界面上点名要问的词，不做专业度筛选。
///
/// 与自动提取相反 —— 那边宁缺毋滥，这边用户既然问了就必须给答案。
pub fn lookup_term(llm: &dyn talksage_llm::LLMProvider, term: &str, context: &str) -> anyhow::Result<String> {
    let term = term.trim();
    if term.is_empty() {
        anyhow::bail!("术语不能为空");
    }
    let context_section = if context.trim().is_empty() {
        String::new()
    } else {
        format!("会议上下文（供判断语境用）：{}", context.trim())
    };
    let prompt = render_prompt(
        prompts::TERM_LOOKUP_USER,
        &[("term", term), ("context_section", &context_section)],
    );
    let answer = llm.complete(&prompt, prompts::TERM_LOOKUP_SYSTEM)?;
    let answer = answer.trim();
    if answer.is_empty() {
        anyhow::bail!("LLM 无输出");
    }
    // 模型有时只回解释、不带词头；补上词头，界面才能拆成「术语 + 解释」两栏
    let first_line = answer.lines().find(|l| !l.trim().is_empty()).unwrap_or(answer).trim();
    if first_line.contains('：') || first_line.contains(':') {
        Ok(first_line.to_string())
    } else {
        Ok(format!("{term}：{first_line}"))
    }
}

/// 判断 LLM 返回的是"无术语"兜底短语而非真实术语列表。
/// 真实术语行含 `|` 分隔符；兜底短语通常是一句话，不带分隔符。
fn is_no_term_response(text: &str) -> bool {
    let lower = text.to_lowercase();
    // 明确的"无"类短语
    if lower.contains("无专业术语")
        || lower.contains("没有专业术语")
        || lower.contains("无需解释")
        || lower.contains("未发现")
        || lower.contains("no term")
        || lower.starts_with("none")
        || lower.starts_with("无")
        || lower.starts_with("没有")
        || lower.starts_with("该")
    {
        return true;
    }
    // 整个响应不含分隔符：不符合"术语 | 专业度 | 解释"格式，视为无效
    !text.contains('|') && !text.contains('｜')
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

    /// 发送空 Final 事件撤销骨架卡片；无骨架时返回 None。
    fn dismiss_skeleton(&self) -> Option<DomainEvent> {
        let result_id = self.pending_result_id.lock().unwrap().take()?;
        Some(DomainEvent::Term {
            result_id,
            status: ResultStatus::Final,
            content: String::new(),
        })
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
            "专业术语识别中…".to_string()
        };
        vec![DomainEvent::Term {
            result_id,
            status: ResultStatus::Skeleton,
            content,
        }]
    }

    fn run(&self, seg: &TranscriptSegment, ctx: &PluginContext) -> anyhow::Result<Option<DomainEvent>> {
        let Some(llm) = ctx.llm.as_ref() else {
            return Ok(self.dismiss_skeleton());
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
        let raw = llm.complete(&prompt, prompts::TERM_EXPLAINER_SYSTEM)?;
        let raw = raw.trim().to_string();
        if raw.is_empty() || is_no_term_response(&raw) {
            log::debug!("term_explainer: LLM 无有效术语，撤销骨架 raw={raw:?}");
            return Ok(self.dismiss_skeleton());
        }
        // 硬门槛：专业度不够、或解释里自己承认不专业的，一律不出卡片
        let terms = parse_terms(&raw, &pinned);
        if terms.is_empty() {
            log::debug!("term_explainer: 无够格的专业术语（已过滤），raw={raw:?}");
            return Ok(self.dismiss_skeleton());
        }
        // 展示仍用「术语：解释」，一行一条（入库时按行拆成独立记录）
        let content = terms
            .iter()
            .map(|t| format!("{}：{}", t.term, t.explanation))
            .collect::<Vec<_>>()
            .join("\n");
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
            label: "专业术语",
            description: "只挑听众可能不懂的行业术语/缩写并解释；常识词不收。支持用户自定义关注词",
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
            "min_chars": 25,
            "user_terms": "",
        }))
    }

    fn register(&self, cfg: &crate::registry::PluginConfig, _ctx: &PluginContext, hooks: &mut crate::registry::HookRegistry) {
        let cooldown = cfg.get_f64("cooldown_seconds", 20.0);
        let llm_extract = cfg.get_bool("llm_extract", true);
        let min_chars = cfg.get_u64("min_chars", 25) as usize;
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
        TranscriptSegment { id: None,
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

    /// 线上真实踩到的那批：常识词、以及模型自己承认"非专业"的条目，都不该出卡片。
    #[test]
    fn parse_terms_drops_everyday_words_and_self_declared_casual() {
        let raw = "心率 | 2 | 单位时间内心跳次数。
天猫超市 | 1 | 阿里旗下线上超市。
扛风雨 | 3 | 比喻承受压力，非专业术语。
鹅腿阿姨 | 4 | 网络热梗，指代特定事件中的争议人物。
MOQ | 5 | 最小起订量，供应商单次接单的最低数量门槛。";
        let terms = parse_terms(raw, &[]);
        assert_eq!(terms.len(), 1, "只应留下 MOQ，实际: {terms:?}");
        assert_eq!(terms[0].term, "MOQ");
        assert_eq!(terms[0].level, 5);
    }

    /// 最多两条，且拿不到专业度（模型没按格式输出）时宁可不收。
    #[test]
    fn parse_terms_caps_count_and_requires_the_level_field() {
        let three = "灰度发布 | 4 | 新版本先放小比例用户。
对赌协议 | 5 | 业绩未达标时的补偿条款。
幂等 | 5 | 重复执行结果不变。";
        assert_eq!(parse_terms(three, &[]).len(), MAX_TERMS);

        // 老格式「术语：解释」没有专业度可判，全部丢弃
        assert!(parse_terms("购物车：电商网站中暂存待购商品的功能。", &[]).is_empty());
    }

    /// 用户点名要的词不受专业度门槛限制 —— 那是他自己加的关注词。
    #[test]
    fn parse_terms_keeps_pinned_terms_regardless_of_level() {
        let raw = "验收标准 | 2 | 双方约定的交付合格线。";
        assert!(parse_terms(raw, &[]).is_empty());
        let kept = parse_terms(raw, &["验收标准".to_string()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].term, "验收标准");
    }

    /// 一段里筛不出够格的术语时，应撤销骨架而不是硬塞一条。
    #[test]
    fn run_dismisses_skeleton_when_nothing_qualifies() {
        let mock = MockProvider { response: "购物车 | 1 | 电商网站中暂存待购商品的功能。".into() };
        let ctx = PluginContext { kb: None, llm: Some(std::sync::Arc::new(mock)), ..PluginContext::new() };
        let p = TermExplainerPlugin::new(0.0, true, 5, "");
        p.skeleton(&seg("这段话里只有购物车这种常识词"));
        match p.run(&seg("这段话里只有购物车这种常识词"), &ctx).unwrap() {
            Some(DomainEvent::Term { status: ResultStatus::Final, content, .. }) => {
                assert!(content.is_empty(), "应是撤销骨架的空事件，实际: {content:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn run_calls_llm_and_returns_final_term() {
        let mock = MockProvider { response: "NPI | 5 | New Product Introduction，新产品导入流程".into() };
        let ctx = PluginContext { kb: None, llm: Some(std::sync::Arc::new(mock)), ..PluginContext::new() };
        let p = TermExplainerPlugin::new(0.0, true, 5, "");
        match p.run(&seg("We need NPI samples"), &ctx).unwrap() {
            Some(DomainEvent::Term { status: ResultStatus::Final, content, .. }) => {
                assert!(content.contains("NPI"));
            }
            other => panic!("{other:?}"),
        }
    }

    /// 手动查词：用户点名要的，必须给答案，且不受专业度门槛限制。
    #[test]
    fn lookup_term_returns_answer_for_any_word() {
        let mock = MockProvider { response: "SLA：服务等级协议，服务商承诺的可用性与响应时限。".into() };
        let out = lookup_term(&mock, "SLA", "").unwrap();
        assert_eq!(out, "SLA：服务等级协议，服务商承诺的可用性与响应时限。");
    }

    /// 模型常常只回解释、不带词头；界面要靠「术语：解释」拆两栏，所以得补上。
    #[test]
    fn lookup_term_prepends_the_word_when_model_omits_it() {
        let mock = MockProvider { response: "服务等级协议，服务商承诺的可用性与响应时限。".into() };
        let out = lookup_term(&mock, "  SLA  ", "").unwrap();
        assert!(out.starts_with("SLA："), "应补上词头: {out}");
    }

    #[test]
    fn lookup_term_rejects_empty_input() {
        let mock = MockProvider { response: "whatever".into() };
        assert!(lookup_term(&mock, "   ", "").is_err());
    }

    #[test]
    fn find_acronyms_still_works() {
        let found = find_acronyms("We need NPI and MOQ by Friday.");
        assert!(found.contains(&"NPI".to_string()));
        assert!(found.contains(&"MOQ".to_string()));
    }
}
