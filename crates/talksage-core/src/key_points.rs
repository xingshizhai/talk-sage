//! 会中要点抽取：本地规则从 committed 转写段提取问句/要求/决策/行动/技术。
//!
//! 这是唯一抽取层。历史页只回放已落库的结果；会后 LLM 做整理，不再重新抽取。

use serde::{Deserialize, Serialize};

use crate::{text_noise_score, KeyPointCategory};

/// 噪音段不参与抽取（阈值与质量评估一致，略保守）。
const NOISE_THRESHOLD: f32 = 0.5;
const MAX_PER_SEGMENT: usize = 3;
const DEDUP_WINDOW: usize = 5;
const MAX_ITEMS: usize = 80;
const MAX_TEXT_CHARS: usize = 120;

/// 单段抽取结果（尚未分配 result_id）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedKeyPoint {
    pub category: KeyPointCategory,
    pub text: String,
}

/// 持久化/事件用的要点记录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyPointRecord {
    pub result_id: String,
    pub category: KeyPointCategory,
    pub content: String,
    pub ts_ms: u64,
}

/// 会话内增量聚合：近窗口去重，有界列表。
#[derive(Debug, Default)]
pub struct KeyPointAggregator {
    items: Vec<KeyPointRecord>,
}

impl KeyPointAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[KeyPointRecord] {
        &self.items
    }

    /// 对一条 final 段抽取并并入列表。返回本轮新增的记录。
    pub fn push(&mut self, text: &str, ts_ms: u64) -> Vec<KeyPointRecord> {
        let extracted = extract_key_points(text);
        if extracted.is_empty() {
            return Vec::new();
        }
        let mut added = Vec::new();
        for kp in extracted {
            let recent = if self.items.len() > DEDUP_WINDOW {
                &self.items[self.items.len() - DEDUP_WINDOW..]
            } else {
                self.items.as_slice()
            };
            if recent.iter().any(|r| r.content == kp.text) {
                continue;
            }
            let record = KeyPointRecord {
                result_id: format!("kp-{ts_ms}-{}", self.items.len()),
                category: kp.category,
                content: kp.text,
                ts_ms,
            };
            self.items.push(record.clone());
            added.push(record);
        }
        if self.items.len() > MAX_ITEMS {
            let drop = self.items.len() - MAX_ITEMS;
            self.items.drain(0..drop);
        }
        added
    }
}

/// 从一条文本提取要点（按句判定，最多 3 条/段）。
pub fn extract_key_points(text: &str) -> Vec<ExtractedKeyPoint> {
    let t = text.trim();
    if t.chars().count() < 4 {
        return Vec::new();
    }
    if text_noise_score(t) > NOISE_THRESHOLD {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for sentence in split_for_kp(t) {
        let Some(category) = classify_sentence(&sentence) else {
            continue;
        };
        let min_len = if category == KeyPointCategory::Action { 6 } else { 8 };
        if sentence.chars().count() < min_len {
            continue;
        }
        if !seen.insert(sentence.clone()) {
            continue;
        }
        let clipped = if sentence.chars().count() > MAX_TEXT_CHARS {
            let cut: String = sentence.chars().take(MAX_TEXT_CHARS).collect();
            format!("{cut}…")
        } else {
            sentence
        };
        out.push(ExtractedKeyPoint {
            category,
            text: clipped,
        });
        if out.len() >= MAX_PER_SEGMENT {
            break;
        }
    }
    out
}

fn split_for_kp(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for coarse in text.split(|c| matches!(c, '。' | '！' | '？' | '；' | '…' | '\n')) {
        for fine in coarse.split(|c| matches!(c, '，' | '、' | ',' | ';')) {
            let s = fine.trim();
            if s.chars().count() >= 2 {
                parts.push(s.to_string());
            }
        }
    }
    parts
}

fn classify_sentence(sentence: &str) -> Option<KeyPointCategory> {
    if is_question(sentence) {
        return Some(KeyPointCategory::Question);
    }
    if is_decision(sentence) {
        return Some(KeyPointCategory::Decision);
    }
    if is_action(sentence) || is_numeric(sentence) {
        return Some(KeyPointCategory::Action);
    }
    if is_requirement(sentence) {
        return Some(KeyPointCategory::Requirement);
    }
    if is_technical(sentence) {
        return Some(KeyPointCategory::Technical);
    }
    if has_zh_actor(sentence) && contains_any(sentence, &["交付", "提交", "发送", "安排", "跟进", "确认", "做", "完成", "给"])
    {
        return Some(KeyPointCategory::Action);
    }
    None
}

fn is_question(s: &str) -> bool {
    s.contains('?')
        || s.contains('？')
        || contains_any(s, &["吗", "呢", "怎么", "什么", "多少", "能不能", "要不要", "是否"])
        || has_any_en_word(
            s,
            &[
                "what", "how", "why", "when", "where", "who", "which", "should", "could", "would", "can",
                "do", "does", "is", "are",
            ],
        )
}

fn is_decision(s: &str) -> bool {
    contains_any(s, &["确认", "决定", "就定", "采用", "拍板", "定了", "达成", "结论"])
        || has_any_en_word(s, &["agreed", "decided", "proceed", "settled", "confirmed"])
        || has_en_phrase(s, "go with")
}

fn is_requirement(s: &str) -> bool {
    contains_any(
        s,
        &[
            "要求", "需要", "必须", "希望", "期望", "交期", "价格", "报价", "样品", "MOQ", "NPI", "预算",
            "指标",
        ],
    ) || has_any_en_word(s, &["need", "require", "must", "should", "want", "wants"])
}

fn is_technical(s: &str) -> bool {
    contains_any(
        s,
        &[
            "方案", "架构", "接口", "协议", "版本", "兼容", "性能", "延迟", "并发", "部署", "迁移", "API",
            "SDK", "规范", "数据库", "服务器", "前端", "后端",
        ],
    )
}

fn is_action(s: &str) -> bool {
    contains_any(
        s,
        &[
            "提交", "发送", "发给", "安排", "跟进", "汇总", "整理", "确认", "通知", "联系", "更新", "上线",
            "交付", "截止", "之前", "之后", "明天", "下周", "本周", "月底", "月初", "下午", "上午",
        ],
    ) || s.contains('约')
        || has_any_en_word(
            s,
            &[
                "send", "submit", "deliver", "schedule", "arrange", "call", "email", "write", "prepare",
                "review",
            ],
        )
        || has_en_phrase(s, "follow up")
        || has_en_phrase(s, "followup")
}

fn is_numeric(s: &str) -> bool {
    let units = ['台', '套', '个', '件', '万', '亿', '元', '块', '%', '批', '日', '号', '月', '周', '点', '人'];
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j].is_whitespace()) {
                j += 1;
            }
            if j < chars.len() && units.contains(&chars[j]) {
                return true;
            }
        }
    }
    if s.contains('Q') || s.contains('q') {
        for (a, b) in s.chars().zip(s.chars().skip(1)) {
            if matches!(a, 'Q' | 'q') && matches!(b, '1' | '2' | '3' | '4') {
                return true;
            }
        }
    }
    let digit_run = chars.iter().filter(|c| c.is_ascii_digit()).count();
    digit_run >= 2
}

fn has_zh_actor(s: &str) -> bool {
    contains_any(s, &["我", "你", "他", "她"])
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

fn has_any_en_word(hay: &str, words: &[&str]) -> bool {
    words.iter().any(|w| has_en_word(hay, w))
}

fn has_en_word(hay: &str, word: &str) -> bool {
    let lower: Vec<char> = hay.to_lowercase().chars().collect();
    let needle: Vec<char> = word.to_lowercase().chars().collect();
    if needle.is_empty() || lower.len() < needle.len() {
        return false;
    }
    for i in 0..=lower.len() - needle.len() {
        if lower[i..i + needle.len()] == needle[..] {
            let prev_ok = i == 0 || !lower[i - 1].is_ascii_alphanumeric();
            let next = i + needle.len();
            let next_ok = next >= lower.len() || !lower[next].is_ascii_alphanumeric();
            if prev_ok && next_ok {
                return true;
            }
        }
    }
    false
}

fn has_en_phrase(hay: &str, phrase: &str) -> bool {
    hay.to_lowercase().contains(&phrase.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_questions() {
        let kps = extract_key_points("What is the MOQ for this product?");
        assert_eq!(kps[0].category, KeyPointCategory::Question);
    }

    #[test]
    fn classifies_decisions() {
        let kps = extract_key_points("我们决定采用方案 B");
        assert_eq!(kps[0].category, KeyPointCategory::Decision);
    }

    #[test]
    fn classifies_requirements() {
        let kps = extract_key_points("We need NPI samples by Friday.");
        assert_eq!(kps[0].category, KeyPointCategory::Requirement);
    }

    #[test]
    fn classifies_technical() {
        let kps = extract_key_points("讨论一下接口的兼容性和并发性能");
        assert_eq!(kps[0].category, KeyPointCategory::Technical);
    }

    #[test]
    fn classifies_action_items_with_numbers_and_time() {
        let kps = extract_key_points("我们下周一交付300台，客户周五前提交报价");
        assert!(!kps.is_empty());
        assert!(kps.iter().any(|k| k.category == KeyPointCategory::Action));
    }

    #[test]
    fn extracts_multiple_kinds_from_one_turn() {
        let kps = extract_key_points("价格能再低一些吗？我们决定采用方案A。客户下周一交付样品");
        let kinds: Vec<_> = kps.iter().map(|k| k.category).collect();
        assert!(kinds.contains(&KeyPointCategory::Question));
        assert!(kinds.contains(&KeyPointCategory::Decision));
        assert!(kinds.contains(&KeyPointCategory::Action));
    }

    #[test]
    fn ignores_too_short() {
        assert!(extract_key_points("嗯").is_empty());
        assert!(extract_key_points("好的").is_empty());
    }

    #[test]
    fn rejects_noisy_segments_even_if_they_match_keywords() {
        assert!(extract_key_points("嗯嗯嗯对那个技术嗯嗯嗯").is_empty());
        let mut agg = KeyPointAggregator::new();
        assert!(agg.push("嗯嗯嗯要求嗯嗯嗯嗯", 1).is_empty());
    }

    #[test]
    fn truncates_long_text() {
        let long = "我们需要确认交期并讨论技术方案的兼容性性能延迟并发部署迁移规范。".repeat(8);
        let kps = extract_key_points(&long);
        assert!(!kps.is_empty());
        assert!(kps.iter().all(|k| k.text.chars().count() <= 121));
    }

    #[test]
    fn aggregator_dedupes_identical_points() {
        let mut agg = KeyPointAggregator::new();
        agg.push("We need NPI samples", 1);
        agg.push("We need NPI samples", 2);
        assert_eq!(agg.items().len(), 1);
    }

    #[test]
    fn aggregator_keeps_distinct_points() {
        let mut agg = KeyPointAggregator::new();
        agg.push("We need NPI samples by Friday.", 1);
        agg.push("我们决定采用方案 B", 2);
        assert_eq!(agg.items().len(), 2);
    }
}
