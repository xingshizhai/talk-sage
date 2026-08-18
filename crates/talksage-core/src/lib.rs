//! TalkSage v2 领域模型与事件类型。
//!
//! 设计原则：领域事件是与传输无关的纯数据结构（serde 序列化），
//! Tauri IPC 与 headless 模式的 WebSocket 传输同一结构。

use serde::{Deserialize, Serialize};

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
}
