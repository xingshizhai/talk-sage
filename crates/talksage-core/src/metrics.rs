//! 会话指标与实时提示（借鉴 Call.md conversation-metrics / nudge-engine）。
//!
//! - `ConversationMetrics`：纯统计（无 LLM）——我/客户发言占比、语速 WPM、
//!   提问数、独白检测、打断计数、平均段长、会话健康分 0–100。
//! - `NudgeEngine`：规则驱动 + 冷却限流的会中提示（talk_ratio / questions /
//!   pace / next_steps），模板 + severity + 可行动作。

use serde::{Deserialize, Serialize};

use crate::TranscriptSegment;

/// 会话指标（会中实时 / 会后入 meta）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConversationMetrics {
    /// 我（speaker_id=0）发言时长占比（0..1）。
    pub talk_ratio_me: f32,
    /// 其他说话人发言时长占比（0..1）。
    pub talk_ratio_them: f32,
    /// 我语速（词/分钟，clamp 50–250；数据不足为 0）。
    pub pace_wpm: f32,
    /// 我提问数（文本含问句标记）。
    pub questions_me: usize,
    /// 独白检测（我连续发言 >45s）。
    pub monologue_detected: bool,
    /// 最长连续发言（ms）。
    pub longest_monologue_ms: u64,
    /// 打断计数（不同说话人段重叠）。
    pub interruption_count: usize,
    /// 词数（我 / 其他）。
    pub words_me: usize,
    pub words_them: usize,
    /// final 段数（我 / 其他）。
    pub segment_count_me: usize,
    pub segment_count_them: usize,
    /// 平均段长 ms（我 / 其他）。
    pub avg_segment_ms_me: u64,
    pub avg_segment_ms_them: u64,
    /// 会话健康分 0–100。
    pub health_score: u8,
    /// 通话时长（最后一个段结束时刻，ms）。
    pub call_duration_ms: u64,
}

impl ConversationMetrics {
    /// 是否有足够数据（至少一段 final）。
    pub fn has_data(&self) -> bool {
        self.segment_count_me + self.segment_count_them > 0
    }
}

/// 段级别数据（指标计算用：start_ms/end_ms 为相对会话起点的时间轴）。
#[derive(Debug, Clone, Copy)]
struct Seg {
    me: bool,
    start_ms: u64,
    end_ms: u64,
    words: usize,
    question: bool,
}

/// 问句启发式：ASCII '?'、全角 '？'、句尾 吗/呢，或疑问词（什么/怎么/为什么/如何/是否/能不能/要不要/多少/哪/几）。
pub fn is_question_text(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t.contains('?') || t.contains('？') {
        return true;
    }
    if t.ends_with('吗') || t.ends_with('呢') {
        return true;
    }
    ["什么", "怎么", "为什么", "如何", "是否", "能不能", "要不要", "多少", "哪个", "哪些", "几点", "几号"].iter().any(|w| t.contains(w))
}

/// 统计词数（中英文通用：按空白切分，中文连续串算一个词）。
pub fn count_words(text: &str) -> usize {
    text.split_whitespace().filter(|w| !w.is_empty()).count()
}

/// 从 final 段计算会话指标。
///
/// `segments` 的 `ts_ms` 为**段结束时刻**（pipeline 约定），`duration_ms` 为段时长；
/// 段开始 = `ts_ms - duration_ms`。说话人按 `speaker_id == 0`（我）分组。
pub fn compute_conversation_metrics(segments: &[TranscriptSegment]) -> ConversationMetrics {
    let mut segs: Vec<Seg> = segments
        .iter()
        .filter(|s| !s.is_partial && !s.text.trim().is_empty())
        .map(|s| Seg {
            me: s.speaker_id == 0,
            start_ms: s.ts_ms.saturating_sub(s.duration_ms),
            end_ms: s.ts_ms,
            words: count_words(&s.text),
            question: is_question_text(&s.text),
        })
        .collect();
    segs.sort_by_key(|s| s.start_ms);

    let mut me_dur = 0u64;
    let mut them_dur = 0u64;
    let mut me_words = 0usize;
    let mut them_words = 0usize;
    let mut me_questions = 0usize;
    let mut me_segs = 0usize;
    let mut them_segs = 0usize;
    let mut me_dur_sum = 0u64;
    let mut them_dur_sum = 0u64;
    let mut call_end = 0u64;

    for s in &segs {
        let dur = s.end_ms.saturating_sub(s.start_ms);
        if s.me {
            me_dur += dur;
            me_words += s.words;
            me_segs += 1;
            me_dur_sum += dur;
            if s.question {
                me_questions += 1;
            }
        } else {
            them_dur += dur;
            them_words += s.words;
            them_segs += 1;
            them_dur_sum += dur;
        }
        call_end = call_end.max(s.end_ms);
    }

    let total_dur = (me_dur + them_dur) as f32;
    let talk_ratio_me = if total_dur > 0.0 { me_dur as f32 / total_dur } else { 0.5 };
    let talk_ratio_them = if total_dur > 0.0 { them_dur as f32 / total_dur } else { 0.5 };

    // 语速 WPM：我首尾段时间跨度为分母（>5s 才可信），否则用通话时长
    let pace_wpm = {
        let me_first = segs.iter().find(|s| s.me);
        let me_last = segs.iter().rev().find(|s| s.me);
        let raw = match (me_first, me_last) {
            (Some(f), Some(l)) if l.end_ms.saturating_sub(f.start_ms) > 5_000 => {
                let span_min = l.end_ms.saturating_sub(f.start_ms) as f32 / 60_000.0;
                me_words as f32 / span_min
            }
            _ if call_end > 10_000 => {
                let call_min = call_end as f32 / 60_000.0;
                me_words as f32 / call_min
            }
            _ => 0.0,
        };
        if me_words == 0 {
            0.0
        } else {
            raw.clamp(50.0, 250.0)
        }
    };

    // 独白：我连续段（gap < 2s）时长 > 45s
    let (monologue_detected, longest_monologue_ms) = detect_monologue(&segs);

    // 打断：相邻不同说话人且时间重叠
    let mut interruptions = 0usize;
    for pair in segs.windows(2) {
        if pair[0].me != pair[1].me && pair[1].start_ms < pair[0].end_ms {
            interruptions += 1;
        }
    }

    let health_score = health_score(
        talk_ratio_me,
        monologue_detected,
        pace_wpm,
        me_questions,
    );

    ConversationMetrics {
        talk_ratio_me,
        talk_ratio_them,
        pace_wpm,
        questions_me: me_questions,
        monologue_detected,
        longest_monologue_ms,
        interruption_count: interruptions,
        words_me: me_words,
        words_them: them_words,
        segment_count_me: me_segs,
        segment_count_them: them_segs,
        avg_segment_ms_me: if me_segs > 0 { me_dur_sum / me_segs as u64 } else { 0 },
        avg_segment_ms_them: if them_segs > 0 { them_dur_sum / them_segs as u64 } else { 0 },
        health_score,
        call_duration_ms: call_end,
    }
}

/// 独白检测（我）：相邻段 gap<2s 视为连续，总时长 >45s 判定独白；返回 (是否独白, 最长连续 ms)。
fn detect_monologue(segs: &[Seg]) -> (bool, u64) {
    const MONOLOGUE_MS: u64 = 45_000;
    const GAP_MS: u64 = 2_000;
    let mut max_streak = 0u64;
    let mut streak_start: Option<u64> = None;
    let mut streak_end = 0u64;
    let finalize = |max_streak: &mut u64, start: u64, end: u64| {
        *max_streak = (*max_streak).max(end.saturating_sub(start));
    };
    for s in segs {
        if !s.me {
            // 他方发言打断：先结算当前连续段
            if let Some(start) = streak_start.take() {
                finalize(&mut max_streak, start, streak_end);
            }
            continue;
        }
        match streak_start {
            None => {
                streak_start = Some(s.start_ms);
                streak_end = s.end_ms;
            }
            Some(start) => {
                if s.start_ms.saturating_sub(streak_end) < GAP_MS {
                    streak_end = s.end_ms;
                } else {
                    finalize(&mut max_streak, start, streak_end);
                    streak_start = Some(s.start_ms);
                    streak_end = s.end_ms;
                }
            }
        }
    }
    if let Some(start) = streak_start {
        finalize(&mut max_streak, start, streak_end);
    }
    (max_streak > MONOLOGUE_MS, max_streak)
}

/// 会话健康分（0–100）：发言均衡 −偏差；独白 −15；语速过/慢 −分；提问 +分。
fn health_score(talk_ratio_me: f32, monologue: bool, pace_wpm: f32, questions: usize) -> u8 {
    let mut score = 100.0;
    score -= (talk_ratio_me - 0.5).abs() * 100.0; // 最多 −50
    if monologue {
        score -= 15.0;
    }
    if pace_wpm > 180.0 {
        score -= ((pace_wpm - 180.0) / 5.0).min(20.0);
    } else if pace_wpm > 0.0 && pace_wpm < 100.0 {
        score -= ((100.0 - pace_wpm) / 5.0).min(10.0);
    }
    if questions > 0 {
        score += (questions as f32 * 2.0).min(10.0);
    }
    score.clamp(0.0, 100.0) as u8
}

// ── 实时提示引擎 ──────────────────────────────────────────

/// 提示类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeKind {
    TalkRatio,
    Questions,
    Pace,
    NextSteps,
}

/// 提示严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeSeverity {
    Low,
    Medium,
    High,
}

/// 提示动作（前端可一键触发）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NudgeAction {
    AskQuestion,
    Confirm,
    Pause,
    Clarify,
}

/// 一条会中提示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nudge {
    pub id: String,
    pub kind: NudgeKind,
    pub severity: NudgeSeverity,
    pub message: String,
    pub action: Option<NudgeAction>,
    pub timestamp_ms: u64,
}

/// 规则提示引擎：优先序 talk_ratio → questions → pace → next_steps，
/// 全局冷却（默认 2 分钟）防打扰。
#[derive(Debug, Clone)]
pub struct NudgeEngine {
    cooldown_ms: u64,
    last_nudge_ms: u64,
}

impl Default for NudgeEngine {
    fn default() -> Self {
        Self::new(120_000)
    }
}

impl NudgeEngine {
    pub fn new(cooldown_ms: u64) -> Self {
        Self { cooldown_ms, last_nudge_ms: 0 }
    }

    /// 评估当前指标，可能产生一条提示（受冷却与规则门槛限制）。
    pub fn evaluate(&mut self, metrics: &ConversationMetrics, call_duration_ms: u64, now_ms: u64) -> Option<Nudge> {
        if now_ms.saturating_sub(self.last_nudge_ms) < self.cooldown_ms {
            return None;
        }
        let nudge = self.check_talk_ratio(metrics)
            .or_else(|| self.check_questions(metrics, call_duration_ms))
            .or_else(|| self.check_pace(metrics))
            .or_else(|| self.check_next_steps(call_duration_ms));
        if let Some(mut n) = nudge {
            n.timestamp_ms = now_ms;
            self.last_nudge_ms = now_ms;
            Some(n)
        } else {
            None
        }
    }

    fn mk(&self, kind: NudgeKind, severity: NudgeSeverity, message: &str, action: Option<NudgeAction>, now_ms: u64) -> Nudge {
        Nudge {
            id: format!("{now_ms}"),
            kind,
            severity,
            message: message.to_string(),
            action,
            timestamp_ms: now_ms,
        }
    }

    fn check_talk_ratio(&self, m: &ConversationMetrics) -> Option<Nudge> {
        // 总发言不足 60s 不提示
        let total_speech_ms = m.segment_count_me as u64 * m.avg_segment_ms_me + m.segment_count_them as u64 * m.avg_segment_ms_them;
        if total_speech_ms < 60_000 {
            return None;
        }
        if m.talk_ratio_me > 0.75 {
            Some(self.mk(NudgeKind::TalkRatio, NudgeSeverity::Medium, "你主导了大部分对话——多倾听客户，留出发言空间。", Some(NudgeAction::AskQuestion), 0))
        } else if m.talk_ratio_me > 0.65 {
            Some(self.mk(NudgeKind::TalkRatio, NudgeSeverity::Low, "发言占比偏高——适当把话题抛给客户。", Some(NudgeAction::AskQuestion), 0))
        } else {
            None
        }
    }

    fn check_questions(&self, m: &ConversationMetrics, call_duration_ms: u64) -> Option<Nudge> {
        if call_duration_ms < 180_000 {
            return None;
        }
        // 期望 1 问 / 2 分钟，低于期望一半时提示
        let expected = (call_duration_ms as f32 / 120_000.0) as usize;
        if m.questions_me < expected.saturating_mul(1) / 2 {
            Some(self.mk(NudgeKind::Questions, NudgeSeverity::Low, "提问偏少——开放式问题能更好挖掘客户需求。", Some(NudgeAction::AskQuestion), 0))
        } else {
            None
        }
    }

    fn check_pace(&self, m: &ConversationMetrics) -> Option<Nudge> {
        if m.pace_wpm > 180.0 {
            Some(self.mk(NudgeKind::Pace, NudgeSeverity::Low, "语速偏快——放慢节奏，客户更容易跟上。", None, 0))
        } else {
            None
        }
    }

    fn check_next_steps(&self, call_duration_ms: u64) -> Option<Nudge> {
        // 20min 与 30min 的 30s 窗口各触发一次
        if (1_200_000..1_230_000).contains(&call_duration_ms) {
            Some(self.mk(NudgeKind::NextSteps, NudgeSeverity::Medium, "临近尾声——和客户确认下一步与时间节点。", Some(NudgeAction::Confirm), 0))
        } else if (1_800_000..1_830_000).contains(&call_duration_ms) {
            Some(self.mk(NudgeKind::NextSteps, NudgeSeverity::High, "会议已 30 分钟——建议收束议题，明确行动项。", Some(NudgeAction::Confirm), 0))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(speaker_id: u32, text: &str, start_ms: u64, dur_ms: u64) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id,
            speaker_label: if speaker_id == 0 { "我".into() } else { "客户".into() },
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms: start_ms + dur_ms,
            duration_ms: dur_ms,
            rms: 0.2,
        }
    }

    #[test]
    fn metrics_basic_ratio_words_questions() {
        let segs = vec![
            seg(0, "我们需要确认方案", 0, 3000),
            seg(1, "ok go ahead", 3500, 2000),
            seg(0, "价格能再低一些吗？", 6000, 2000),
        ];
        let m = compute_conversation_metrics(&segs);
        // 我 5000ms，客户 2000ms
        assert!((m.talk_ratio_me - 5000.0 / 7000.0).abs() < 0.01);
        assert!(m.questions_me >= 1, "问句未识别: {m:?}");
        assert_eq!(m.segment_count_me, 2);
        assert_eq!(m.segment_count_them, 1);
        assert!(m.has_data());
    }

    #[test]
    fn metrics_monologue_and_interruption() {
        let segs = vec![
            seg(0, "a", 0, 30_000),
            seg(0, "b", 31_000, 20_000), // gap 1s < 2s → 连续，共 50s > 45s
            seg(1, "c", 48_000, 8_000),  // 与上一段重叠 → 打断
        ];
        let m = compute_conversation_metrics(&segs);
        assert!(m.monologue_detected, "独白未检出: {m:?}");
        assert!(m.longest_monologue_ms >= 50_000);
        assert!(m.interruption_count >= 1, "打断未检出: {m:?}");
    }

    #[test]
    fn metrics_health_score_bounds() {
        // 完全一边倒 + 独白 → 低分
        let mut one_sided: Vec<TranscriptSegment> = Vec::new();
        for i in 0..5u64 {
            one_sided.push(seg(0, "我们在讲", i * 20_000, 18_000));
        }
        let m = compute_conversation_metrics(&one_sided);
        assert!(m.health_score <= 50, "一边倒应低分: {m:?}");
        assert!(m.health_score <= 100);
    }

    #[test]
    fn nudge_engine_cooldown_and_rules() {
        let mut engine = NudgeEngine::new(120_000);
        // 一边倒 + 足够时长 → talk_ratio 提示
        let m = ConversationMetrics {
            talk_ratio_me: 0.9,
            talk_ratio_them: 0.1,
            pace_wpm: 130.0,
            questions_me: 0,
            monologue_detected: true,
            longest_monologue_ms: 60_000,
            interruption_count: 0,
            words_me: 100,
            words_them: 10,
            segment_count_me: 10,
            segment_count_them: 2,
            avg_segment_ms_me: 20_000,
            avg_segment_ms_them: 5_000,
            health_score: 20,
            call_duration_ms: 300_000,
        };
        // now_ms 为 epoch 毫秒（现实值）
        let n = engine.evaluate(&m, 300_000, 1_700_000_000_000);
        assert!(n.is_some(), "应触发 talk_ratio 提示");
        // 冷却期内不重复
        assert!(engine.evaluate(&m, 300_000, 1_700_000_001_000).is_none());
        // 冷却过后（2 分钟后），规则不再命中（ratio 正常）→ 无提示
        let m2 = ConversationMetrics { talk_ratio_me: 0.5, talk_ratio_them: 0.5, questions_me: 5, pace_wpm: 130.0, ..m.clone() };
        let n2 = engine.evaluate(&m2, 300_000, 1_700_000_121_000);
        assert!(n2.is_none(), "正常会话不应提示: {n2:?}");
    }

    #[test]
    fn question_detection_heuristics() {
        assert!(is_question_text("你们什么时候能交付？"));
        assert!(is_question_text("这个价格还能谈吗"));
        assert!(is_question_text("What about the timeline?"));
        assert!(!is_question_text("我们下周一交付。"));
        assert!(!is_question_text("确认一下方案 A"));
    }
}
