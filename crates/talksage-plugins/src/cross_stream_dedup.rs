//! cross_stream_dedup：双流（麦克风 + 系统回环）把同一句话各识别一次，
//! 只保留先到的那份。
//!
//! 迁移自 pipeline/src/lib.rs:812 的 emit 包装。判定逻辑复用
//! talksage_core::is_echo_duplicate。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::json;
use talksage_core::DomainEvent;

use crate::registry::{EventFilter, HookRegistry, Plugin, PluginConfig};
use crate::PluginContext;

/// 回声比对的历史窗口容量（条）。
pub const HISTORY_CAP: usize = 32;

/// 跨流回声去重。内部有可变历史，因此用 Mutex —— filter 签名是 &self。
#[derive(Default)]
pub struct CrossStreamDedupFilter {
    /// (speaker_id, text, ts_ms)
    recent: Mutex<VecDeque<(u32, String, u64)>>,
}

impl CrossStreamDedupFilter {
    pub fn history_len(&self) -> usize {
        self.recent.lock().unwrap().len()
    }
}

impl EventFilter for CrossStreamDedupFilter {
    fn filter(&self, ev: DomainEvent) -> Option<DomainEvent> {
        let DomainEvent::Segment {
            speaker_id, speaker_label, text, is_partial: false, ts_ms, ..
        } = &ev
        else {
            return Some(ev);
        };
        let mut recent = self.recent.lock().unwrap();
        let is_echo = recent.iter().any(|(sp, t, ts)| {
            *sp != *speaker_id && talksage_core::is_echo_duplicate(t, text, ts_ms.saturating_sub(*ts))
        });
        if is_echo {
            log::info!(
                "跨流回显去重: 丢弃[{}] 文本={}（与另一条流重复）",
                speaker_label,
                text.chars().take(40).collect::<String>()
            );
            return None;
        }
        recent.push_back((*speaker_id, text.clone(), *ts_ms));
        if recent.len() > HISTORY_CAP {
            recent.pop_front();
        }
        drop(recent);
        Some(ev)
    }
}

pub struct CrossStreamDedupPlugin;

impl Plugin for CrossStreamDedupPlugin {
    fn id(&self) -> &'static str {
        "cross_stream_dedup"
    }

    fn label(&self) -> &'static str {
        "双流去重"
    }

    fn default_config(&self) -> PluginConfig {
        PluginConfig::from_value(json!({ "enabled": true }))
    }

    fn register(&self, _cfg: &PluginConfig, _ctx: &PluginContext, hooks: &mut HookRegistry) {
        hooks.add_filter(Arc::new(CrossStreamDedupFilter::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talksage_core::DomainEvent;

    fn seg(speaker_id: u32, text: &str, ts_ms: u64) -> DomainEvent {
        DomainEvent::Segment {
            speaker_id,
            speaker_label: if speaker_id == 0 { "我".into() } else { "客户".into() },
            speaker_attribution: None,
            text: text.into(),
            is_partial: false,
            ts_ms,
            duration_ms: 800,
            rms: 0.2,
            revision: 0,
            start_sample: 0,
            end_sample: 12800,
        }
    }

    #[test]
    fn keeps_the_first_copy_and_drops_the_cross_stream_echo() {
        let f = CrossStreamDedupFilter::default();
        assert!(f.filter(seg(0, "我们下周确认交期", 1000)).is_some(), "先到的应保留");
        assert!(
            f.filter(seg(1, "我们下周确认交期", 1200)).is_none(),
            "另一条流的同一句话应被吞掉"
        );
    }

    #[test]
    fn same_stream_repetition_is_not_an_echo() {
        // 同一说话人真的说了两遍，不是双录，必须保留
        let f = CrossStreamDedupFilter::default();
        assert!(f.filter(seg(0, "好的", 1000)).is_some());
        assert!(f.filter(seg(0, "好的", 1200)).is_some(), "同流重复不是回声");
    }

    #[test]
    fn different_text_from_the_other_stream_passes_through() {
        let f = CrossStreamDedupFilter::default();
        assert!(f.filter(seg(0, "我们下周确认交期", 1000)).is_some());
        assert!(f.filter(seg(1, "完全不同的一句话", 1200)).is_some());
    }

    /// 契约测试（防御性），不是现网覆盖：产生点的 filter 链**只喂 final 段**
    /// （见 `EventFilter` 文档），partial 根本到不了这里。锁住的是「万一以后
    /// 有人把 partial 也接进来，hypothesis 文本不会被当成回声吞掉」。
    #[test]
    fn partials_are_never_deduped() {
        let f = CrossStreamDedupFilter::default();
        let mut p = seg(0, "重复", 1000);
        if let DomainEvent::Segment { is_partial, .. } = &mut p {
            *is_partial = true;
        }
        assert!(f.filter(p.clone()).is_some());
        assert!(f.filter(p).is_some(), "partial 不参与去重");
    }

    #[test]
    fn history_is_bounded() {
        let f = CrossStreamDedupFilter::default();
        for i in 0..100 {
            let _ = f.filter(seg(0, &format!("句子{i}"), 1000 + i as u64 * 100));
        }
        assert!(f.history_len() <= HISTORY_CAP, "历史窗口必须有界");
    }

    #[test]
    fn plugin_registers_one_filter() {
        let p = CrossStreamDedupPlugin;
        assert_eq!(p.id(), "cross_stream_dedup");
        let mut hooks = HookRegistry::default();
        p.register(&p.default_config(), &PluginContext::new(), &mut hooks);
        assert_eq!(hooks.filter_count(), 1);
    }
}
