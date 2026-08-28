//! LLM 要点聚合插件：批量积累转写段 → 调用 LLM → 发射 KeyPoint 事件。
//!
//! 策略：
//! - `buffer`：待自动聚合的新段，积满 `batch_size` 或超过 `tail_timeout_ms` 后触发
//! - `recent`：最近 N 段的滑动窗口，**手动聚合专用**，不受自动聚合清空影响
//!   这样即使自动一直在跑，用户手动点击时仍能得到足够的上下文

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;
use talksage_core::{DomainEvent, KeyPointCategory, ResultStatus, TranscriptSegment};

use crate::registry::{HookRegistry, Plugin, PluginConfig, SegmentObserver};
use crate::PluginContext;

// ── 共享状态 ────────────────────────────────────────────────────────────────

/// 手动聚合的回看窗口（段数）。
///
/// 用户点「立即整理」多半是因为自动聚合没整出东西，所以这里要看得比自动批次
/// （默认 4 段一批）远得多 —— 40 段约等于最近十轮自动批次的全部内容。
const RECENT_WINDOW: usize = 40;

struct State {
    /// 尚未自动聚合的新段，积满 batch_size 后清空。
    buffer: Vec<String>,
    /// 最近 RECENT_WINDOW 段的滑动窗口，手动聚合时使用，自动聚合不清空它。
    recent: Vec<String>,
    /// LLM 已返回、等待下次 skeleton() 发射的事件。
    pending: Vec<DomainEvent>,
    /// 上次触发 LLM 的时间（用于 tail 超时触发）。
    last_flush: Instant,
    /// 已输出要点的归一化指纹（跨批去重）。
    seen: std::collections::HashSet<String>,
    /// 已输出要点的完整文本（传给 LLM 做语义去重）。
    emitted: Vec<String>,
}

impl State {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            recent: Vec::new(),
            pending: Vec::new(),
            last_flush: Instant::now(),
            seen: std::collections::HashSet::new(),
            emitted: Vec::new(),
        }
    }

    fn record_emitted(&mut self, content: &str) {
        self.remember(content);
        if self.emitted.len() >= 50 {
            self.emitted.remove(0);
        }
        self.emitted.push(content.to_string());
    }

    fn push_segment(&mut self, text: String) {
        self.buffer.push(text.clone());
        self.recent.push(text);
        if self.recent.len() > RECENT_WINDOW {
            self.recent.remove(0);
        }
    }

    /// 内容归一化：小写、去空白与常见标点，用于同义要点去重。
    fn fingerprint(content: &str) -> String {
        content
            .chars()
            .filter(|c| !c.is_whitespace() && !matches!(c, '，' | '。' | '、' | '；' | '：' | ',' | '.' | '?' | '？' | '!' | '！' | '"' | '“' | '”' | ' '))
            .flat_map(|c| c.to_lowercase())
            .collect()
    }

    fn is_duplicate(&self, content: &str) -> bool {
        self.seen.contains(&Self::fingerprint(content))
    }

    fn remember(&mut self, content: &str) {
        let fp = Self::fingerprint(content);
        if fp.chars().count() >= 6 {
            self.seen.insert(fp);
        }
    }
}

// ── Observer ────────────────────────────────────────────────────────────────

pub struct KeyPointLlmObserver {
    batch_size: usize,
    /// 超过此时长（ms）且 buffer 非空时，不足 batch_size 也触发。
    tail_timeout_ms: u64,
    state: Mutex<State>,
    /// 手动 flush 标志：设为 true 后在下次 run() 中强制触发 LLM。
    manual_flush: AtomicBool,
}

impl KeyPointLlmObserver {
    fn new(batch_size: usize, tail_timeout_ms: u64) -> Self {
        Self {
            batch_size,
            tail_timeout_ms,
            state: Mutex::new(State::new()),
            manual_flush: AtomicBool::new(false),
        }
    }

    /// 手动整理：用户点了「立即整理」，说明自动那轮太保守。放宽标准再问一次，
    /// 但仍然把已提取要点发过去做去重（否则会把上一轮的结论重说一遍）。
    fn call_llm_manual(
        texts: &[String],
        existing: &[String],
        llm: &Arc<dyn talksage_llm::LLMProvider>,
    ) -> (Vec<(KeyPointCategory, String)>, Vec<String>) {
        log::info!(
            "key_point_llm: 手动整理发送 {} 段给 LLM，内容: {}",
            texts.len(),
            texts.join(" | ").chars().take(200).collect::<String>()
        );
        let prompt = build_prompt_manual(texts, existing);
        match llm.complete(&prompt, SYSTEM_PROMPT) {
            Ok(resp) => {
                log::info!("key_point_llm: 手动整理原始响应: {}", resp.chars().take(500).collect::<String>());
                let result = parse_response(&resp);
                log::info!("key_point_llm: 手动整理解析得到 {} 个要点，{} 个关键词", result.0.len(), result.1.len());
                result
            }
            Err(e) => {
                log::warn!("key_point_llm: 手动整理 LLM 调用失败: {e}");
                (Vec::new(), Vec::new())
            }
        }
    }

    fn call_llm(
        texts: &[String],
        existing: &[String],
        llm: &Arc<dyn talksage_llm::LLMProvider>,
    ) -> (Vec<(KeyPointCategory, String)>, Vec<String>) {
        log::info!(
            "key_point_llm: 发送 {} 段给 LLM，内容: {}",
            texts.len(),
            texts.join(" | ").chars().take(200).collect::<String>()
        );
        let prompt = build_prompt(texts, existing);
        match llm.complete(&prompt, SYSTEM_PROMPT) {
            Ok(resp) => {
                log::info!(
                    "key_point_llm: LLM 原始响应: {}",
                    resp.chars().take(500).collect::<String>()
                );
                let result = parse_response(&resp);
                log::info!(
                    "key_point_llm: 解析得到 {} 个要点，{} 个关键词",
                    result.0.len(),
                    result.1.len()
                );
                result
            }
            Err(e) => {
                log::warn!("key_point_llm: LLM 调用失败: {e}");
                (Vec::new(), Vec::new())
            }
        }
    }
}

impl SegmentObserver for KeyPointLlmObserver {
    fn name(&self) -> &'static str {
        "key_point_llm"
    }

    fn request_flush(&self) {
        self.manual_flush.store(true, Ordering::Relaxed);
    }

    fn flush_now(&self, ctx: &PluginContext, emit: &dyn Fn(DomainEvent)) {
        let Some(llm) = ctx.llm.as_ref() else {
            log::warn!("key_point_llm: 手动 flush 无 LLM，跳过");
            return;
        };
        let (texts, existing) = {
            let mut g = self.state.lock().unwrap();
            if g.recent.is_empty() {
                log::info!("key_point_llm: 手动 flush 滑动窗口为空，跳过");
                emit(DomainEvent::KeyPointFlush {
                    added: 0,
                    message: "最近没有新的转写内容".into(),
                });
                return;
            }
            g.last_flush = Instant::now();
            g.buffer.clear();
            // 刻意不清空 recent：一次没整出东西不该把这批内容永久丢掉，
            // 用户再点一次仍应能重试（重复要点由 seen 指纹和 prompt 去重拦住）。
            (g.recent.clone(), g.emitted.clone())
        };
        log::info!("key_point_llm: 手动 flush 处理最近 {} 段（已有 {} 条要点作去重参考）", texts.len(), existing.len());
        let (points, keywords) = Self::call_llm_manual(&texts, &existing, llm);
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut added = 0usize;
        let mut skipped_dup = 0usize;
        {
            let mut g = self.state.lock().unwrap();
            for (i, (category, content)) in points.into_iter().enumerate() {
                if g.is_duplicate(&content) {
                    skipped_dup += 1;
                    continue;
                }
                g.record_emitted(&content);
                added += 1;
                emit(DomainEvent::KeyPoint {
                    result_id: format!("kp-manual-{ts_ms}-{i}"),
                    status: talksage_core::ResultStatus::Final,
                    category,
                    content,
                    ts_ms,
                    manual: true,
                });
            }
            for (i, kw) in keywords.into_iter().enumerate() {
                if kw.trim().is_empty() { continue; }
                emit(DomainEvent::Term {
                    result_id: format!("term-kp-manual-{ts_ms}-{i}"),
                    status: talksage_core::ResultStatus::Final,
                    content: kw,
                });
            }
        }
        // 回执：没有它，界面上"没整出要点"和"功能坏了"长得一模一样
        let message = flush_message(added, skipped_dup, texts.len(), existing.len());
        log::info!("key_point_llm: 手动 flush 完成 —— {message}");
        emit(DomainEvent::KeyPointFlush { added, message });
    }

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        !seg.is_partial && seg.text.trim().chars().count() >= 6
    }

    /// 把上一批 LLM 结果发射出去，同时将本段加入缓冲区。
    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let mut g = self.state.lock().unwrap();
        let label = if seg.speaker_label.is_empty() { "讲话者" } else { seg.speaker_label.as_str() };
        g.push_segment(format!("[{label}] {}", seg.text.trim()));
        std::mem::take(&mut g.pending)
    }

    /// 当积满 batch_size 或超过 tail_timeout 时调用 LLM。
    fn run(&self, _seg: &TranscriptSegment, ctx: &PluginContext) -> anyhow::Result<Option<DomainEvent>> {
        let Some(llm) = ctx.llm.as_ref() else { return Ok(None) };

        let manual = self.manual_flush.swap(false, Ordering::Relaxed);
        let should_flush = {
            let g = self.state.lock().unwrap();
            let timeout_elapsed = self.tail_timeout_ms > 0
                && g.last_flush.elapsed().as_millis() as u64 >= self.tail_timeout_ms;
            g.buffer.len() >= self.batch_size
                || (timeout_elapsed && !g.buffer.is_empty())
                || (manual && !g.buffer.is_empty())
        };

        if should_flush {
            if manual {
                log::info!("key_point_llm: 手动触发自动批量整理（via run）");
            }
            let (texts, existing) = {
                let mut g = self.state.lock().unwrap();
                g.last_flush = Instant::now();
                // 自动聚合只清空 buffer，不碰 recent（留给手动聚合）
                (std::mem::take(&mut g.buffer), g.emitted.clone())
            };
            let (points, keywords) = Self::call_llm(&texts, &existing, llm);
            let ts_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let mut g = self.state.lock().unwrap();
            for (i, (category, content)) in points.into_iter().enumerate() {
                if g.is_duplicate(&content) {
                    log::debug!("key_point_llm: 跳过重复要点: {content}");
                    continue;
                }
                g.record_emitted(&content);
                g.pending.push(DomainEvent::KeyPoint {
                    result_id: format!("kp-llm-{ts_ms}-{i}"),
                    status: ResultStatus::Final,
                    category,
                    content,
                    ts_ms,
                    manual: false,
                });
            }
            for (i, kw) in keywords.into_iter().enumerate() {
                if kw.trim().is_empty() { continue; }
                g.pending.push(DomainEvent::Term {
                    result_id: format!("term-kp-{ts_ms}-{i}"),
                    status: ResultStatus::Final,
                    content: kw,
                });
            }
            if !g.pending.is_empty() {
                log::info!("key_point_llm: 批量结果 {} 条已存入 pending，等待下段发射", g.pending.len());
            }
        }

        Ok(None)
    }
}

// ── Plugin ──────────────────────────────────────────────────────────────────

pub struct KeyPointLlmPlugin;

impl Plugin for KeyPointLlmPlugin {
    fn descriptor(&self) -> &'static crate::PluginDescriptor {
        static D: crate::PluginDescriptor = crate::PluginDescriptor {
            id: "key_point_llm",
            label: "要点聚合（LLM）",
            description: "用 LLM 从转写提取要点，支持 DeepSeek / OpenRouter 等 API",
            category: crate::PluginCategory::Analysis,
            phase: crate::PluginPhase::Observer,
            capabilities: &[crate::PluginCapability::Llm],
            host_managed: &[],
            after: &[],
        };
        &D
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({
            "enabled": true,
            "batch_size": 12,
            "tail_timeout_ms": 60000,
        }))
    }

    fn register(&self, cfg: &PluginConfig, _ctx: &PluginContext, hooks: &mut HookRegistry) {
        let batch_size = cfg.get_u64("batch_size", 12) as usize;
        let tail_timeout_ms = cfg.get_u64("tail_timeout_ms", 60000);
        hooks.add_observer(Arc::new(KeyPointLlmObserver::new(batch_size, tail_timeout_ms)));
    }
}

// ── Prompt & Parser ─────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
你是专业的内容摘要助理，专门处理**语音识别（ASR）转写结果**。\
输入文本来自自动语音识别，可能存在同音字错误、中英混杂、口语停顿词、句子不完整等问题。\
请根据上下文推断说话者的真实意图，而非照字面解读可能有误的文字。\
\n\n你的任务是从转写片段中提炼**核心要点**。内容类型可能是：\
\n- 会议讨论 → 提取决策、要求、行动项、待解决问题\
\n- 教学讲座 → 提取核心知识点、重要概念、关键结论\
\n- 故事叙述 → 提取关键事件、人物、时间节点\
\n- 其他场景 → 提取对听者有价值的实质性信息\
\n\n忽略：寒暄客套、确认性应答（嗯/对/好的）、无意义的口语碎片、不完整的半句话。\
\n只返回 JSON 数组，不加任何其他文字或 Markdown 格式。";

/// 手动整理的结论文案：让用户一眼看懂这次为什么是这个结果。
fn flush_message(added: usize, skipped_dup: usize, segments: usize, existing: usize) -> String {
    if added > 0 {
        return format!("新增 {added} 条要点");
    }
    if skipped_dup > 0 {
        return format!("没有新要点（{skipped_dup} 条与已有要点重复）");
    }
    if existing > 0 {
        return format!("未发现新要点（已有 {existing} 条已覆盖这 {segments} 段内容）");
    }
    format!("这 {segments} 段里没有提炼出实质要点")
}

/// 手动整理用的提示词：在通用要求之上放宽标准。
///
/// 用户点「立即整理」通常是因为自动聚合什么都没给出，所以这里明确要求"尽量给出"，
/// 而不是继续沿用自动批次那套"宁可少而精"。
fn build_prompt_manual(texts: &[String], existing: &[String]) -> String {
    format!(
        "{}

补充要求（用户主动点击「立即整理」）：
- 自动聚合这一轮没有产出，用户希望拿到结果：请放宽标准，尽量给出 1–3 条最有价值的信息；
- 即使是闲聊、故事或零散信息，也可以用 \"other\" 类别概括其中的事实、观点或结论；
- 仍然不要重复上面已提取的要点；
- 只有当这批内容确实没有任何可概括的信息时，才返回空数组。",
        build_prompt(texts, existing)
    )
}

fn build_prompt(texts: &[String], existing: &[String]) -> String {
    let numbered = texts
        .iter()
        .enumerate()
        .map(|(i, t)| format!("[{}] {}", i + 1, t))
        .collect::<Vec<_>>()
        .join("\n");

    let existing_section = if existing.is_empty() {
        String::new()
    } else {
        let list = existing.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n");
        format!(
            "\n**已提取的要点（请勿重复，语义相同或高度相似的内容也算重复）：**\n{list}\n"
        )
    };

    format!(
        "以下是语音识别转写片段（ASR 输出，可能含错字、谐音字、中英混杂、不完整句子）：\n\
{numbered}\n\
{existing_section}\n\
请先理解上下文推断真实含义，再提炼核心内容。返回 JSON 数组，每个元素包含：\n\
- category: \"requirement\"（要求/需求）| \"decision\"（决策）| \"action\"（行动项）| \"question\"（待解答问题）| \"technical\"（技术/知识要点）| \"other\"（其他重要信息）\n\
- content: 一句话概括要点，用规范书面语，主语明确，≤40字；如遇 ASR 错字请纠正后再概括\n\
- keywords: 数组，每项形如 {{\"term\":\"词\",\"gloss\":\"≤20字解释\"}}。只收听众可能不懂、需要解释的行业缩写/专业名词/技术概念；\n\
  常识词（人名、地名、日常物品、知名公司产品）一律不收；没有够格的就给空数组\n\n\
要求：\n\
1. 提取对听者有价值的实质性内容，无论是决策、知识点还是关键事件；\n\
2. 多条片段讲同一件事时合并成一条；\n\
3. 跳过寒暄、确认应答、语气词、不完整的半句话；\n\
4. 宁可少而精，不要多而碎；\n\
5. 内容有误字时（ASR 错误）先纠正再概括；\n\
6. 已提取的要点不要重复，语义相同或高度相似的内容直接跳过。\n\n\
示例：[{{\"category\":\"technical\",\"content\":\"瞒天过海：利用处于优势地位时麻痹对方，暗中行动\",\"keywords\":[{{\"term\":\"瞒天过海\",\"gloss\":\"三十六计之一，以常态掩护真实意图\"}}]}}]\n\
若无新的实质性内容返回空数组 []"
    )
}

/// 关键词必须带解释：「专业术语」面板里没有解释的词等于噪音。
#[derive(Deserialize)]
struct RawKeyword {
    term: String,
    #[serde(default)]
    gloss: String,
}

impl RawKeyword {
    /// 归一成「术语：解释」；缺任一半就丢弃（返回空串）。
    fn into_line(self) -> String {
        let (term, gloss) = (self.term.trim(), self.gloss.trim());
        if term.is_empty() || gloss.is_empty() {
            return String::new();
        }
        format!("{term}：{gloss}")
    }
}

#[derive(Deserialize)]
struct RawPoint {
    category: String,
    content: String,
    #[serde(default)]
    keywords: Vec<RawKeyword>,
}

fn parse_category(s: &str) -> KeyPointCategory {
    match s.trim().to_lowercase().as_str() {
        "requirement" | "要求" => KeyPointCategory::Requirement,
        "decision" | "决策" => KeyPointCategory::Decision,
        "action" | "行动" | "行动项" => KeyPointCategory::Action,
        "question" | "问句" | "问题" => KeyPointCategory::Question,
        "technical" | "技术" | "技术方案" | "技术知识要点" | "知识要点" => KeyPointCategory::Technical,
        _ => KeyPointCategory::Other,
    }
}

fn parse_response(resp: &str) -> (Vec<(KeyPointCategory, String)>, Vec<String>) {
    let json_str = extract_json_array(resp);
    let Ok(points) = serde_json::from_str::<Vec<RawPoint>>(&json_str) else {
        log::warn!("key_point_llm: 无法解析 LLM 响应: {}", &resp[..resp.len().min(200)]);
        return (Vec::new(), Vec::new());
    };
    let mut kps = Vec::new();
    let mut all_keywords: Vec<String> = Vec::new();
    for p in points {
        if p.content.trim().is_empty() { continue; }
        kps.push((parse_category(&p.category), p.content.trim().to_string()));
        for kw in p.keywords {
            let kw = kw.into_line();
            if !kw.is_empty() && !all_keywords.contains(&kw) {
                all_keywords.push(kw);
            }
        }
    }
    (kps, all_keywords)
}

fn extract_json_array(s: &str) -> String {
    if let (Some(start), Some(end)) = (s.find('['), s.rfind(']')) {
        if start <= end {
            return s[start..=end].to_string();
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::{build_prompt_manual, flush_message};

    /// 手动整理的结论文案要能区分四种情况，否则界面上又变成"点了没反应"。
    #[test]
    fn flush_message_explains_every_outcome() {
        assert_eq!(flush_message(2, 0, 12, 3), "新增 2 条要点");
        assert!(flush_message(0, 3, 12, 3).contains("重复"), "全被去重时要说清楚");
        assert!(flush_message(0, 0, 12, 5).contains("已有 5 条"), "模型认为已覆盖时要点明");
        assert!(flush_message(0, 0, 12, 0).contains("没有提炼出"), "确实没内容时直说");
    }

    /// 关键词要带解释：只给裸词的话，「专业术语」面板里全是没有解释的词
    /// （线上现象：面板里只剩「木乃伊」「欧洲」「祷告」这类词，一条解释都没有）。
    #[test]
    fn keywords_carry_their_gloss() {
        let resp = r#"[{"category":"technical","content":"讨论了灰度发布策略",
            "keywords":[{"term":"灰度发布","gloss":"新版本先放小比例用户"},
                        {"term":"SLA","gloss":"服务等级协议"}]}]"#;
        let (_, kws) = parse_response(resp);
        assert_eq!(kws, vec!["灰度发布：新版本先放小比例用户", "SLA：服务等级协议"]);
    }

    /// 没有解释的关键词直接丢弃：面板里只应出现「术语：解释」。
    #[test]
    fn keywords_without_gloss_are_dropped() {
        let resp = r#"[{"category":"other","content":"聊到了 MOQ","keywords":[{"term":"NPI","gloss":""},{"term":"MOQ","gloss":"最小起订量"}]}]"#;
        let (_, kws) = parse_response(resp);
        assert_eq!(kws, vec!["MOQ：最小起订量"]);
    }

    /// 手动整理不该消费滑动窗口：一次没整出东西，用户再点一次仍要能重试。
    /// （线上现象：点第二次直接"滑动窗口为空，跳过"，那批内容再也回不来。）
    #[test]
    fn manual_flush_keeps_the_window_and_reports_result() {
        use super::*;
        use talksage_core::TranscriptSegment;
        use talksage_llm::MockSeqProvider;

        let obs = KeyPointLlmObserver::new(99, 0); // batch_size 很大，确保不会自动触发
        for i in 0..3 {
            let seg = TranscriptSegment {
                speaker_id: 0,
                speaker_label: "讲话者".into(),
                speaker_attribution: None,
                text: format!("这是第 {i} 段有内容的转写文本"),
                is_partial: false,
                ts_ms: i as u64 * 1000,
                duration_ms: 800,
                rms: 0.2,
            };
            let _ = SegmentObserver::skeleton(&obs, &seg);
        }

        // 第一次：模型给一条要点
        let llm = std::sync::Arc::new(MockSeqProvider::new(vec![
            r#"[{"category":"other","content":"第一次整理出的要点","keywords":[]}]"#.into(),
            "[]".into(),
        ]));
        let ctx = PluginContext { kb: None, llm: Some(llm), ..PluginContext::new() };

        let first = std::sync::Mutex::new(Vec::new());
        SegmentObserver::flush_now(&obs, &ctx, &|ev| first.lock().unwrap().push(ev));
        let first = first.into_inner().unwrap();
        assert!(
            first.iter().any(|e| matches!(e, DomainEvent::KeyPoint { content, .. } if content == "第一次整理出的要点")),
            "应发出要点: {first:?}"
        );
        match first.last() {
            Some(DomainEvent::KeyPointFlush { added, message }) => {
                assert_eq!(*added, 1);
                assert!(message.contains("新增 1 条"), "回执应说明结果: {message}");
            }
            other => panic!("最后一条应是回执，实际: {other:?}"),
        }

        // 第二次：窗口还在（不是"窗口为空"），模型这次返回空 → 回执说明原因
        let second = std::sync::Mutex::new(Vec::new());
        SegmentObserver::flush_now(&obs, &ctx, &|ev| second.lock().unwrap().push(ev));
        let second = second.into_inner().unwrap();
        match second.last() {
            Some(DomainEvent::KeyPointFlush { added, message }) => {
                assert_eq!(*added, 0);
                assert!(!message.contains("最近没有新的转写内容"), "窗口不该被上一次清空: {message}");
            }
            other => panic!("最后一条应是回执，实际: {other:?}"),
        }
    }

    /// 手动整理放宽标准，但仍要带上已提取要点做去重。
    #[test]
    fn manual_prompt_relaxes_standard_and_keeps_dedup_list() {
        let texts = vec!["[讲话者] 木乃伊可以泡水喝。".to_string()];
        let existing = vec!["欧洲人将木乃伊入药泡水饮用".to_string()];
        let prompt = build_prompt_manual(&texts, &existing);
        assert!(prompt.contains("放宽标准"), "手动整理应放宽提取标准");
        assert!(prompt.contains("尽量给出"), "应明确要求给出结果");
        assert!(prompt.contains("欧洲人将木乃伊入药泡水饮用"), "已提取要点仍要发过去做去重");
        assert!(prompt.contains("请勿重复"), "去重指令不能丢");
    }

    use super::*;

    #[test]
    fn parse_response_normal() {
        let resp = r#"[{"category":"action","content":"下周完成 API 文档","keywords":[{"term":"API","gloss":"应用程序接口"}]},{"category":"decision","content":"预算不超过 20 万","keywords":[]}]"#;
        let (pts, kws) = parse_response(resp);
        assert_eq!(pts.len(), 2);
        assert!(matches!(pts[0].0, KeyPointCategory::Action));
        assert!(matches!(pts[1].0, KeyPointCategory::Decision));
        assert_eq!(kws, vec!["API：应用程序接口"]);
    }

    #[test]
    fn parse_response_no_keywords_field() {
        let resp = r#"[{"category":"action","content":"下周完成 API 文档"}]"#;
        let (pts, kws) = parse_response(resp);
        assert_eq!(pts.len(), 1);
        assert!(kws.is_empty());
    }

    #[test]
    fn parse_response_with_surrounding_text() {
        let resp = "好的，以下是要点：\n[{\"category\":\"question\",\"content\":\"预算如何分配？\",\"keywords\":[]}]";
        let (pts, _) = parse_response(resp);
        assert_eq!(pts.len(), 1);
        assert!(matches!(pts[0].0, KeyPointCategory::Question));
    }

    #[test]
    fn parse_response_empty_array() {
        let (pts, kws) = parse_response("[]");
        assert!(pts.is_empty());
        assert!(kws.is_empty());
    }

    #[test]
    fn parse_category_all_variants() {
        assert!(matches!(parse_category("requirement"), KeyPointCategory::Requirement));
        assert!(matches!(parse_category("ACTION"), KeyPointCategory::Action));
        assert!(matches!(parse_category("decision"), KeyPointCategory::Decision));
        assert!(matches!(parse_category("question"), KeyPointCategory::Question));
        assert!(matches!(parse_category("technical"), KeyPointCategory::Technical));
        assert!(matches!(parse_category("foo"), KeyPointCategory::Other));
    }

    #[test]
    fn state_dedupes_identical_points_across_batches() {
        let mut state = State::new();
        assert!(!state.is_duplicate("下周前完成 API 文档更新"));
        state.remember("下周前完成 API 文档更新");
        assert!(state.is_duplicate("下周前完成 api 文档更新。"));
        assert!(!state.is_duplicate("预算不超过 20 万"));
    }

    #[test]
    fn state_ignores_too_short_fingerprints() {
        let mut state = State::new();
        state.remember("好");
        assert!(!state.is_duplicate("好"));
    }

    #[test]
    fn recent_window_caps_at_limit() {
        let mut state = State::new();
        for i in 0..=RECENT_WINDOW + 5 {
            state.push_segment(format!("段 {i}"));
        }
        assert_eq!(state.recent.len(), RECENT_WINDOW);
        assert_eq!(state.buffer.len(), RECENT_WINDOW + 6);
    }

    #[test]
    fn auto_flush_does_not_affect_recent() {
        let mut state = State::new();
        for i in 0..8 {
            state.push_segment(format!("段 {i}"));
        }
        // 模拟自动聚合：清空 buffer，不碰 recent
        state.buffer.clear();
        assert_eq!(state.recent.len(), 8, "recent 不受自动聚合影响");
    }
}
