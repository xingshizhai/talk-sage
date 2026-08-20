//! TalkSage v2 领域模型与事件类型。
//!
//! 设计原则：领域事件是与传输无关的纯数据结构（serde 序列化），
//! Tauri IPC 与 headless 模式的 WebSocket 传输同一结构。

use serde::{Deserialize, Serialize};

pub mod metrics;
pub use metrics::{
    compute_conversation_metrics, ConversationMetrics, Nudge, NudgeAction, NudgeEngine, NudgeKind, NudgeSeverity,
};

pub mod webhook;
pub use webhook::{post_webhook, trigger_webhooks, validate_webhook_url, WebhookResult};

/// 应用版本（与 workspace 版本保持一致）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 领域事件：实时链路中宿主 → 客户端推送的所有事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    /// 转写片段（增量或最终）。speaker_id: 0=我, 1=客户, >1=其他说话人（diarization 预留）。
    Segment {
        speaker_id: u32,
        speaker_label: String,
        text: String,
        is_partial: bool,
        ts_ms: u64,
        /// 最终段语音时长（ms；partial 事件为 0）。
        #[serde(default)]
        duration_ms: u64,
        /// 最终段平均能量 RMS（0..1；partial 事件为 0）。
        #[serde(default)]
        rms: f32,
    },
    /// 术语解释结果（骨架 → 最终，按 result_id 原地更新）。
    Term {
        result_id: String,
        status: ResultStatus,
        content: String,
    },
    /// 实时翻译结果。
    Translation {
        result_id: String,
        status: ResultStatus,
        direction: TranslationDirection,
        content: String,
    },
    /// 关键要点（需求/技术方案/问句/决策）。
    KeyPoint {
        result_id: String,
        status: ResultStatus,
        category: KeyPointCategory,
        content: String,
    },
    /// 简报检索命中。
    Brief { source: String, text: String },
    /// 对话上下文状态。
    State {
        topic: String,
        open_questions: Vec<String>,
        recent_decisions: Vec<String>,
    },
    /// 状态变更（ASR 加载 / 就绪 / 监听中 / 导入中…）。
    Status {
        stage: StatusStage,
        message: String,
    },
    /// 音频电平（UI 电平表用）。
    Level { mic_rms: f32, loopback_rms: f32 },
    /// 会话级统计（监听结束 / 文件输入结束，每条流推送一次）。
    /// 用于质量评估与历史回溯：给定时间点即可判断会话是"有效语音 / 噪音 / 静音"。
    SessionStats {
        speaker_label: String,
        /// 该流总时长（ms）。
        total_ms: u64,
        /// VAD 检测到的语音时长（ms）。
        speech_ms: u64,
        /// 最终段数量。
        final_segments: usize,
        /// 该流总采样数（16k）。
        samples: u64,
        /// 平均 RMS（能量，0..1）。
        avg_rms: f32,
        /// 峰值 RMS。
        max_rms: f32,
        /// 非语音块平均 RMS（背景噪音水平，自动阈值用）。旧客户端缺省 0。
        #[serde(default)]
        non_speech_avg_rms: f32,
        /// 录音文件路径（若有录音）。
        recording: Option<String>,
        /// VAD 参数快照（回溯灵敏度配置）。
        vad_preset: String,
        /// VAD 阈值。
        vad_threshold: f32,
        /// 该流 final 段词数（借鉴 Call.md 会话指标）。旧客户端缺省 0。
        #[serde(default)]
        words: usize,
        /// 该流 final 段问句数。旧客户端缺省 0。
        #[serde(default)]
        questions: usize,
    },
    /// 会话指标（会中实时：每次 final 段后聚合推送；借鉴 Call.md conversation-metrics）。
    Metrics {
        metrics: ConversationMetrics,
    },
    /// 会中提示（规则 + 冷却限流；借鉴 Call.md nudge-engine）。
    Nudge {
        nudge: Nudge,
    },
}

/// 渐进结果状态（骨架先显示，最终填充）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Skeleton,
    Final,
}

/// 翻译方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslationDirection {
    ZhEn,
    EnZh,
}

/// 要点分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPointCategory {
    Requirement,
    Technical,
    Question,
    Decision,
    Other,
}

/// 运行阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusStage {
    Starting,
    AsrLoading,
    AsrReady,
    Recording,
    Importing,
    Idle,
}

/// 单条转写记录（会话域使用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub speaker_id: u32,
    pub speaker_label: String,
    pub text: String,
    pub is_partial: bool,
    pub ts_ms: u64,
    /// 该段语音时长（ms）。旧数据缺省为 0。
    #[serde(default)]
    pub duration_ms: u64,
    /// 该段平均能量 RMS（0..1）。旧数据缺省为 0。
    #[serde(default)]
    pub rms: f32,
}

/// 会话质量评估：`SessionStats` 聚合后的结论，写入 sessions.meta。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionQuality {
    /// 有效语音占比高，内容可分析。
    Clean,
    /// 语音占比过低或能量异常（噪音/他人声音/转写无意义）。
    Noise,
    /// 几乎无语音（静音/麦克风未采集到）。
    Silent,
    /// 语音占比偏低，需人工复核。
    Low,
}

impl SessionQuality {
    /// 是否应跳过下游分析（要点聚合/简报等）。
    pub fn skip_analysis(&self) -> bool {
        matches!(self, Self::Noise | Self::Silent)
    }
}

/// 编辑距离（Levenshtein）：`talksage bench` 的 CER/WER 基础。
/// 泛型：`&[char]`（字符级）或 `&[&str]`（词级）。
pub fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let (la, lb) = (a.len(), b.len());
    // 滚动两行 DP（O(min(la,lb)) 内存）
    let (short, long) = if la <= lb { (a, b) } else { (b, a) };
    let (_, ll) = (short.len(), long.len());
    let mut prev: Vec<usize> = (0..=ll).collect();
    let mut curr = vec![0usize; ll + 1];
    for (i, cs) in short.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cl) in long.iter().enumerate() {
            let cost = if cs == cl { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1) // 删除
                .min(curr[j] + 1) // 插入
                .min(prev[j] + cost); // 替换/匹配
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[ll]
}

/// 字符错误率 CER（中文按字符）：`ref` 为参考文本，`hyp` 为转写结果。
/// 返回 0..1（0 = 完全正确）。
pub fn cer(reference: &str, hypothesis: &str) -> f32 {
    let r: Vec<char> = reference.trim().chars().collect();
    let h: Vec<char> = hypothesis.trim().chars().collect();
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    edit_distance(&r, &h) as f32 / r.len() as f32
}

/// 词错误率 WER（英文按空白分词）。
pub fn wer(reference: &str, hypothesis: &str) -> f32 {
    let r: Vec<&str> = reference.trim().split_whitespace().collect();
    let h: Vec<&str> = hypothesis.trim().split_whitespace().collect();
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    edit_distance(&r, &h) as f32 / r.len() as f32
}

/// 文本噪音评分：0.0（正常语言）~ 1.0（纯噪音/乱码）。
///
/// 信号：语气词占比（嗯/啊/哦/唉/呢/呀/吧/啦/嘛/么…）、连续重复块（"嗯嗯嗯"/"那个个"）、
/// 非中英文内容占比。用于辅助判定"VAD 认为是语音但内容是噪音/他人声音"的会话。
/// 注意：对"随机汉字组合"型乱码不敏感，此类由会话级 speech_ratio/能量规则兜底。
pub fn text_noise_score(text: &str) -> f32 {
    if text.is_empty() {
        return 1.0;
    }
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len() as f32;
    if total < 2.0 {
        return 1.0;
    }
    // 语气词集合
    const FILLERS: [char; 14] = ['嗯', '啊', '哦', '唉', '哎', '呢', '呀', '吧', '啦', '嘛', '么', '呃', '嘿', '哈'];
    let mut filler = 0usize;
    let mut run_chars = 0usize; // 连续重复块的字符数（第 2 个起计）
    let mut run_len = 0usize;
    let mut meaningful = 0usize;
    let mut prev: Option<char> = None;
    for &c in &chars {
        if FILLERS.contains(&c) {
            filler += 1;
        }
        // 有意义字符：常用汉字范围（0x4E00-0x9FFF）或 ASCII 字母
        let is_cn = ('\u{4e00}'..='\u{9fff}').contains(&c);
        let is_en = c.is_ascii_alphabetic() || c.is_ascii_digit();
        if is_cn || is_en {
            meaningful += 1;
        }
        if prev == Some(c) {
            run_len += 1;
        } else {
            run_len = 1;
        }
        if run_len >= 2 {
            run_chars += 1;
        }
        prev = Some(c);
    }
    let filler_ratio = filler as f32 / total;
    let run_ratio = run_chars as f32 / total;
    let meaningful_ratio = meaningful as f32 / total;
    // bigram 多样性：重复短语（"那个个那个个"）的相邻二元组高度重复 → 噪音
    let mut bigram_set: std::collections::HashSet<[char; 2]> = std::collections::HashSet::new();
    for w in chars.windows(2) {
        bigram_set.insert([w[0], w[1]]);
    }
    let bigram_total = (chars.len() - 1) as f32;
    let bigram_unique = if bigram_total > 0.0 {
        bigram_set.len() as f32 / bigram_total
    } else {
        1.0
    };
    // 综合：重复短语（bigram）为主信号，连续重复块次之，语气词再次
    let score = (1.0 - bigram_unique) * 0.5 + run_ratio * 0.4 + filler_ratio * 0.2 + (1.0 - meaningful_ratio) * 0.1;
    score.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_roundtrips_via_json() {
        let ev = DomainEvent::Segment {
            speaker_id: 1,
            speaker_label: "客户".into(),
            text: "We need NPI samples by Friday.".into(),
            is_partial: false,
            ts_ms: 1234,
            duration_ms: 1500,
            rms: 0.3,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: DomainEvent = serde_json::from_str(&json).unwrap();
        match back {
            DomainEvent::Segment {
                speaker_id, text, ..
            } => {
                assert_eq!(speaker_id, 1);
                assert!(text.contains("NPI"));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn skeleton_and_final_are_distinct() {
        assert_ne!(ResultStatus::Skeleton, ResultStatus::Final);
    }

    #[test]
    fn edit_distance_and_transcription_metrics() {
        // 编辑距离
        assert_eq!(edit_distance(&['a', 'b', 'c'], &['a', 'b', 'c']), 0);
        assert_eq!(edit_distance(&['a', 'b'], &['a']), 1);
        assert_eq!(edit_distance(&['a'], &['b']), 1);
        assert_eq!(edit_distance(&[], &['x']), 1);
        // CER（中文按字符）
        assert!((cer("我们确认交期", "我们确认交期") - 0.0).abs() < 1e-6);
        let cer1 = cer("我们确认交期", "我们确认");
        assert!(cer1 > 0.0 && cer1 < 1.0, "CER 应介于 0-1: {cer1}");
        assert_eq!(cer("", ""), 0.0);
        assert_eq!(cer("", "abc"), 1.0);
        // WER（英文按词）
        assert!((wer("we need samples", "we need samples") - 0.0).abs() < 1e-6);
        let wer1 = wer("we need samples", "we need");
        assert!(wer1 > 0.0 && wer1 < 1.0, "WER 应介于 0-1: {wer1}");
        assert!((wer("hello world", "hello world") - 0.0).abs() < 1e-6);
    }
}
