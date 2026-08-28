//! 会话持久化写线程。
//!
//! Pipeline 的事件回调只负责把少量、需要持久化的事件送进有界队列；SQLite
//! 操作和会后统计收集都在本线程串行执行。关闭 writer 会 drain 队列，因此
//! finalizer 启动前可以保证已提交事件全部可见。

use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Result};
use talksage_core::{DomainEvent, KeyPointRecord, ResultStatus, TranscriptSegment};
use talksage_session::{SessionStore, StreamMeta};

const WRITER_QUEUE_CAPACITY: usize = 256;

enum WriteCommand {
    Segment(TranscriptSegment),
    Term(String),
    Translation(String),
    KeyPoint(KeyPointRecord),
    Stats(StreamMeta),
    Shutdown,
}

impl WriteCommand {
    fn from_event(ev: &DomainEvent) -> Option<Self> {
        match ev {
            DomainEvent::Segment {
                text,
                is_partial: false,
                speaker_id,
                speaker_label,
                speaker_attribution,
                ts_ms,
                duration_ms,
                rms,
                ..
            } => Some(Self::Segment(TranscriptSegment {
                speaker_id: *speaker_id,
                speaker_label: speaker_label.clone(),
                speaker_attribution: speaker_attribution.clone(),
                text: text.clone(),
                is_partial: false,
                ts_ms: *ts_ms,
                duration_ms: *duration_ms,
                rms: *rms,
            })),
            DomainEvent::Term {
                status: ResultStatus::Final,
                content,
                ..
            } => Some(Self::Term(content.clone())),
            DomainEvent::Translation { content, .. } => Some(Self::Translation(content.clone())),
            DomainEvent::KeyPoint {
                result_id,
                status: ResultStatus::Final,
                category,
                content,
                ts_ms,
                ..
            } => Some(Self::KeyPoint(KeyPointRecord {
                result_id: result_id.clone(),
                category: *category,
                content: content.clone(),
                ts_ms: *ts_ms,
            })),
            DomainEvent::SessionStats {
                speaker_label,
                total_ms,
                speech_ms,
                final_segments,
                avg_rms,
                max_rms,
                non_speech_avg_rms,
                recording,
                vad_preset,
                vad_threshold,
                words,
                questions,
                ..
            } => Some(Self::Stats(StreamMeta {
                speaker_label: speaker_label.clone(),
                total_ms: *total_ms,
                speech_ms: *speech_ms,
                final_segments: *final_segments,
                avg_rms: *avg_rms,
                max_rms: *max_rms,
                non_speech_avg_rms: *non_speech_avg_rms,
                recording: recording.clone(),
                vad_preset: vad_preset.clone(),
                vad_threshold: *vad_threshold,
                words: *words,
                questions: *questions,
            })),
            _ => None,
        }
    }
}

/// 单会话 writer。发送端可安全地被 EventSink 持有；`finish` 只调用一次。
pub(super) struct SessionWriter {
    tx: Option<mpsc::SyncSender<WriteCommand>>,
    join: Option<JoinHandle<()>>,
}

impl SessionWriter {
    pub(super) fn start(
        store: Arc<SessionStore>,
        session_id: i64,
        stats: Arc<Mutex<Vec<StreamMeta>>>,
        texts: Arc<Mutex<Vec<String>>>,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(WRITER_QUEUE_CAPACITY);
        let join = thread::Builder::new()
            .name(format!("session-writer-{session_id}"))
            .spawn(move || run_writer(store, session_id, stats, texts, rx))?;
        Ok(Self {
            tx: Some(tx),
            join: Some(join),
        })
    }

    pub(super) fn sender(&self) -> SessionWriterSender {
        SessionWriterSender {
            tx: self.tx.as_ref().expect("writer already finished").clone(),
        }
    }

    /// 关闭发送端并等待队列排空。返回时数据库可供 finalizer 完整读取。
    pub(super) fn finish(&mut self) -> Result<()> {
        if let Some(tx) = self.tx.take() {
            // Shutdown 与之前的写命令共享 FIFO；即使 Pipeline 超时退出、仍有
            // sender clone 存活，也不会靠“最后一个 sender drop”无限等待。
            let _ = tx.send(WriteCommand::Shutdown);
        }
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| anyhow!("会话持久化线程异常退出"))?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct SessionWriterSender {
    tx: mpsc::SyncSender<WriteCommand>,
}

impl SessionWriterSender {
    pub(super) fn enqueue(&self, ev: &DomainEvent) {
        let Some(cmd) = WriteCommand::from_event(ev) else {
            return;
        };
        // 有界队列提供明确背压。只有低频 committed 事件进入此处；正常路径
        // 不等待磁盘，极端持续写入时宁可减速也不静默丢失会话数据。
        if self.tx.send(cmd).is_err() {
            log::warn!("会话持久化线程已退出，事件未写入");
        }
    }
}

fn run_writer(
    store: Arc<SessionStore>,
    session_id: i64,
    stats: Arc<Mutex<Vec<StreamMeta>>>,
    texts: Arc<Mutex<Vec<String>>>,
    rx: mpsc::Receiver<WriteCommand>,
) {
    while let Ok(cmd) = rx.recv() {
        let result = match cmd {
            WriteCommand::Segment(segment) => {
                if let Ok(mut values) = texts.lock() {
                    values.push(segment.text.clone());
                }
                store.add_segment(session_id, &segment)
            }
            WriteCommand::Term(content) => store.add_term(session_id, &content),
            WriteCommand::Translation(content) => {
                store.add_translation(session_id, "translate", &content)
            }
            WriteCommand::KeyPoint(kp) => store.add_key_point(session_id, &kp),
            WriteCommand::Stats(meta) => {
                if let Ok(mut values) = stats.lock() {
                    values.push(meta);
                }
                Ok(())
            }
            WriteCommand::Shutdown => break,
        };
        if let Err(err) = result {
            log::warn!("会话 #{session_id} 异步持久化失败: {err}");
        }
    }
}
