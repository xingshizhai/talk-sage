//! LLM 要点聚合插件：批量积累转写段 → 调用 LLM → 发射 KeyPoint 事件。
//!
//! 策略：每积累 `batch_size` 段（默认 4），在 `run()` 里调一次 LLM；
//! 结果先存入 `pending` 队列，下次 `skeleton()` 调用时发射出去。
//!
//! 不依赖 SessionFinalizer（其 FinalizeContext 无 LLM 句柄），会话末尾
//! 剩余的不足 batch_size 的段会在最后一次 run() 里以 `flush_tail` 模式处理：
//! 当超过 `tail_timeout_ms` 没有新段时，把剩余段也发给 LLM。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Deserialize;
use serde_json::json;
use talksage_core::{DomainEvent, KeyPointCategory, ResultStatus, TranscriptSegment};

use crate::registry::{HookRegistry, Plugin, PluginConfig, SegmentObserver};
use crate::PluginContext;

// ── 共享状态 ────────────────────────────────────────────────────────────────

struct State {
    /// 尚未发给 LLM 的段文本（带说话人标签）。
    buffer: Vec<String>,
    /// LLM 已返回、等待下次 skeleton() 发射的事件。
    pending: Vec<DomainEvent>,
    /// 上次触发 LLM 的时间（用于 tail 超时触发）。
    last_flush: Instant,
    /// 已输出要点的归一化指纹（跨批去重：同一要点在多个批次重复出现时只发一次）。
    seen: std::collections::HashSet<String>,
}

impl State {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            pending: Vec::new(),
            last_flush: Instant::now(),
            seen: std::collections::HashSet::new(),
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

    fn call_llm(
        texts: &[String],
        llm: &Arc<dyn talksage_llm::LLMProvider>,
    ) -> (Vec<(KeyPointCategory, String)>, Vec<String>) {
        let prompt = build_prompt(texts);
        match llm.complete(&prompt, SYSTEM_PROMPT) {
            Ok(resp) => parse_response(&resp),
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
        let texts = {
            let mut g = self.state.lock().unwrap();
            if g.buffer.is_empty() {
                log::info!("key_point_llm: 手动 flush buffer 为空，跳过");
                return;
            }
            g.last_flush = Instant::now();
            std::mem::take(&mut g.buffer)
        };
        log::info!("key_point_llm: 手动 flush 立即处理 {} 段", texts.len());
        let (points, keywords) = Self::call_llm(&texts, llm);
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut g = self.state.lock().unwrap();
        for (i, (category, content)) in points.into_iter().enumerate() {
            if g.is_duplicate(&content) { continue; }
            g.remember(&content);
            emit(DomainEvent::KeyPoint {
                result_id: format!("kp-manual-{ts_ms}-{i}"),
                status: talksage_core::ResultStatus::Final,
                category,
                content,
                ts_ms,
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

    fn should_trigger(&self, seg: &TranscriptSegment) -> bool {
        !seg.is_partial && seg.text.trim().chars().count() >= 6
    }

    /// 把上一批 LLM 结果发射出去，同时将本段加入缓冲区。
    fn skeleton(&self, seg: &TranscriptSegment) -> Vec<DomainEvent> {
        let mut g = self.state.lock().unwrap();
        let label = if seg.speaker_label.is_empty() { "讲话者" } else { seg.speaker_label.as_str() };
        g.buffer.push(format!("[{label}] {}", seg.text.trim()));
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
        if manual && should_flush {
            log::info!("key_point_llm: 手动触发立即整理");
        }

        if should_flush {
            let texts = {
                let mut g = self.state.lock().unwrap();
                g.last_flush = Instant::now();
                std::mem::take(&mut g.buffer)
            };
            let (points, keywords) = Self::call_llm(&texts, llm);
            let ts_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let mut g = self.state.lock().unwrap();
            let mut events: Vec<DomainEvent> = Vec::new();
            for (i, (category, content)) in points.into_iter().enumerate() {
                if g.is_duplicate(&content) {
                    log::debug!("key_point_llm: 跳过重复要点: {content}");
                    continue;
                }
                g.remember(&content);
                events.push(DomainEvent::KeyPoint {
                    result_id: format!("kp-llm-{ts_ms}-{i}"),
                    status: ResultStatus::Final,
                    category,
                    content,
                    ts_ms,
                });
            }
            for (i, kw) in keywords.into_iter().enumerate() {
                if kw.trim().is_empty() { continue; }
                events.push(DomainEvent::Term {
                    result_id: format!("term-kp-{ts_ms}-{i}"),
                    status: ResultStatus::Final,
                    content: kw,
                });
            }
            g.pending.extend(events);
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
            description: "用 LLM 从转写提取会议要点，支持 DeepSeek / OpenRouter 等 API",
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
            "batch_size": 4,
            "tail_timeout_ms": 60000,
        }))
    }

    fn register(&self, cfg: &PluginConfig, _ctx: &PluginContext, hooks: &mut HookRegistry) {
        let batch_size = cfg.get_u64("batch_size", 4) as usize;
        let tail_timeout_ms = cfg.get_u64("tail_timeout_ms", 60000);
        hooks.add_observer(Arc::new(KeyPointLlmObserver::new(batch_size, tail_timeout_ms)));
    }
}

// ── Prompt & Parser ─────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = "\
你是专业的会议记录助理，专门处理**语音识别（ASR）转写结果**。\
输入文本来自自动语音识别，可能存在以下问题：同音字/谐音字错误、方言词汇、\
中英文混杂（英文专有名词可能被转写成中文谐音）、口语停顿词、句子不完整等。\
请根据上下文推断说话者的真实意图，而非照字面解读可能有误的文字。\
\n\n任务：从转写片段中提炼会议核心要点，并提取对话中的关键术语/专有名词。\
\n要点必须是实质性内容：决策、要求、行动项、待解决问题、技术方案等。\
\n忽略废话、客套、寒暄、确认性应答（嗯/对/好的）以及无意义的口语碎片。\
\n只返回 JSON 数组，不加任何其他文字或 Markdown 格式。";

fn build_prompt(texts: &[String]) -> String {
    let numbered = texts
        .iter()
        .enumerate()
        .map(|(i, t)| format!("[{}] {}", i + 1, t))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "以下是语音识别转写片段（ASR 输出，可能含错字、谐音字、中英混杂、不完整句子）：\n\
{numbered}\n\n\
请先理解上下文推断真实含义，再提炼核心内容。返回 JSON 数组，每个元素包含：\n\
- category: \"requirement\"（要求/需求）| \"decision\"（决策）| \"action\"（行动项）| \"question\"（待解答问题）| \"technical\"（技术方案）| \"other\"（其他重要事项）\n\
- content: 一句话概括要点，用规范书面语，主语明确，≤40字；如遇 ASR 错字请纠正后再概括\n\
- keywords: 字符串数组，提取对话中的专业术语、产品名、技术名词、人名、组织名等关键词，每项≤10字，若无则为空数组\n\n\
要求：\n\
1. 只保留**会议核心**：决策、明确的要求、具体的行动项、未决问题、关键技术结论；\n\
2. 多条片段讲同一件事时合并成一条；\n\
3. 跳过寒暄、确认应答、语气词、不完整的半句话；\n\
4. 宁可少而精，不要多而碎。\n\n\
示例：[{{\"category\":\"action\",\"content\":\"下周前完成 API 文档更新\",\"keywords\":[\"API\",\"文档\"]}}]\n\
若无实质性要点返回空数组 []"
    )
}

#[derive(Deserialize)]
struct RawPoint {
    category: String,
    content: String,
    #[serde(default)]
    keywords: Vec<String>,
}

fn parse_category(s: &str) -> KeyPointCategory {
    match s.trim().to_lowercase().as_str() {
        "requirement" | "要求" => KeyPointCategory::Requirement,
        "decision" | "决策" => KeyPointCategory::Decision,
        "action" | "行动" | "行动项" => KeyPointCategory::Action,
        "question" | "问句" | "问题" => KeyPointCategory::Question,
        "technical" | "技术" | "技术方案" => KeyPointCategory::Technical,
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
            let kw = kw.trim().to_string();
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
    use super::*;

    #[test]
    fn parse_response_normal() {
        let resp = r#"[{"category":"action","content":"下周完成 API 文档","keywords":["API","文档"]},{"category":"decision","content":"预算不超过 20 万","keywords":[]}]"#;
        let (pts, kws) = parse_response(resp);
        assert_eq!(pts.len(), 2);
        assert!(matches!(pts[0].0, KeyPointCategory::Action));
        assert!(matches!(pts[1].0, KeyPointCategory::Decision));
        assert_eq!(kws, vec!["API", "文档"]);
    }

    #[test]
    fn parse_response_no_keywords_field() {
        // 旧格式无 keywords 字段时应正常解析
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
        // 同义但标点/大小写不同的表述应视为重复
        assert!(state.is_duplicate("下周前完成 api 文档更新。"));
        // 不同要点不误伤
        assert!(!state.is_duplicate("预算不超过 20 万"));
    }

    #[test]
    fn state_ignores_too_short_fingerprints() {
        let mut state = State::new();
        state.remember("好");
        // 极短指纹不进入去重集合，避免误伤常见短句
        assert!(!state.is_duplicate("好"));
    }
}
