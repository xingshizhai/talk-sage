//! TalkSage v2 会话持久化：SQLite（rusqlite bundled）。
//!
//! 表：sessions（含 meta JSON）/ segments（含 duration_ms/rms）/ terms / translations。
//! SessionStore 线程安全（内部 Mutex<Connection>），可由 pipeline 事件线程写入。
//!
//! `sessions.meta` 保存会话级统计与质量评估（SessionMeta），
//! 使"给定时间点 → 完整回溯会话质量/语音占比/能量/录音路径"成为可能。

use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use talksage_core::TranscriptSegment;

/// 会话概要（历史列表用）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionRecord {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub segment_count: u64,
    pub term_count: u64,
    /// 会话质量（"clean"/"noise"/"silent"/"low"，老数据为 None）。
    pub quality: Option<String>,
    /// 会话时长（ms）。
    pub duration_ms: Option<u64>,
    /// 语音占比（0..1）。
    pub speech_ratio: Option<f32>,
}

/// 搜索命中（跨会话文本检索）。
#[derive(Debug, Clone, Serialize)]
pub struct SegmentHit {
    pub session_id: i64,
    pub speaker_label: String,
    pub text: String,
    pub ts_ms: u64,
}

/// 会话详情（含全部内容与元数据）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub segments: Vec<TranscriptSegment>,
    pub terms: Vec<String>,
    pub translations: Vec<String>,
    pub notes: Option<String>,
    /// 会话元数据（统计/质量），老数据为 None。
    pub meta: Option<SessionMeta>,
}

/// 单条流的统计（写入 meta 用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMeta {
    pub speaker_label: String,
    pub total_ms: u64,
    pub speech_ms: u64,
    pub final_segments: usize,
    pub avg_rms: f32,
    pub max_rms: f32,
    pub recording: Option<String>,
    pub vad_preset: String,
    pub vad_threshold: f32,
}

/// 会话元数据：聚合统计 + 质量评估（存于 sessions.meta JSON）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// 质量结论：clean / noise / silent / low。
    pub quality: String,
    /// 是否跳过下游分析（要点聚合/简报等）。
    pub skipped_analysis: bool,
    /// 会话总时长（ms，主用户流）。
    pub duration_ms: u64,
    /// 语音时长（ms，主用户流）。
    pub speech_ms: u64,
    /// 语音占比（主用户流）。
    pub speech_ratio: f32,
    /// 平均 RMS（主用户流）。
    pub avg_rms: f32,
    /// 峰值 RMS（主用户流）。
    pub max_rms: f32,
    /// 文本噪音评分（各段平均，0..1）。
    pub text_noise: f32,
    /// 各流明细。
    pub streams: Vec<StreamMeta>,
    /// 采样时刻（Unix 秒）。
    pub evaluated_at: i64,
}

impl SessionMeta {
    /// 质量中文标签（前端展示）。
    pub fn quality_label(&self) -> &'static str {
        match self.quality.as_str() {
            "clean" => "正常",
            "noise" => "噪音",
            "silent" => "静音",
            "low" => "待复核",
            _ => "未知",
        }
    }

    /// 根据各流统计 + 段文本评估会话质量。
    ///
    /// 规则（主用户流"我"为准）：
    /// - 时长 < 2s → low
    /// - 无语音（speech_ms==0）：能量 < 0.01 → silent；有能量（环境噪音但 VAD 不认）→ noise
    /// - 语音占比 < 0.15 → silent（有语音但极少）
    /// - 语音占比 < 0.4 或平均 RMS > 0.5 → noise（语音少 / 环境能量大）
    /// - 语音占比 > 0.85 → noise（几乎无停顿，持续有声：噪音/音乐/旁人说话，VAD 误判为语音）
    /// - 文本噪音评分 > 0.45 → noise（VAD 认为是语音，但内容是重复/语气词噪音）
    /// - 语音占比 < 0.6 → low（待复核）
    /// - 否则 → clean
    pub fn evaluate(stats: Vec<StreamMeta>, segment_texts: &[String], now: i64) -> Self {
        let main = stats
            .iter()
            .max_by_key(|s| s.total_ms)
            .cloned()
            .unwrap_or_else(|| StreamMeta {
                speaker_label: "我".into(),
                total_ms: 0,
                speech_ms: 0,
                final_segments: 0,
                avg_rms: 0.0,
                max_rms: 0.0,
                recording: None,
                vad_preset: String::new(),
                vad_threshold: 0.0,
            });
        let ratio = if main.total_ms > 0 {
            main.speech_ms as f32 / main.total_ms as f32
        } else {
            0.0
        };
        let text_noise = if segment_texts.is_empty() {
            0.0
        } else {
            segment_texts
                .iter()
                .map(|t| talksage_core::text_noise_score(t))
                .sum::<f32>()
                / segment_texts.len() as f32
        };

        let quality = if main.total_ms < 2000 {
            talksage_core::SessionQuality::Low
        } else if main.speech_ms == 0 {
            // 无语音：能量低 = 静音；有能量 = 环境噪音（VAD 不认为是语音）
            if main.avg_rms < 0.01 {
                talksage_core::SessionQuality::Silent
            } else {
                talksage_core::SessionQuality::Noise
            }
        } else if ratio < 0.15 {
            talksage_core::SessionQuality::Silent
        } else if ratio < 0.4 || main.avg_rms > 0.5 || ratio > 0.85 || text_noise > 0.45 {
            talksage_core::SessionQuality::Noise
        } else if ratio < 0.6 {
            talksage_core::SessionQuality::Low
        } else {
            talksage_core::SessionQuality::Clean
        };

        let quality_str = match quality {
            talksage_core::SessionQuality::Clean => "clean",
            talksage_core::SessionQuality::Noise => "noise",
            talksage_core::SessionQuality::Silent => "silent",
            talksage_core::SessionQuality::Low => "low",
        }
        .to_string();

        SessionMeta {
            quality: quality_str,
            skipped_analysis: quality.skip_analysis(),
            duration_ms: main.total_ms,
            speech_ms: main.speech_ms,
            speech_ratio: ratio,
            avg_rms: main.avg_rms,
            max_rms: main.max_rms,
            text_noise,
            streams: stats,
            evaluated_at: now,
        }
    }
}

/// 会话存储（线程安全）。
pub struct SessionStore {
    conn: Mutex<Connection>,
}

impl SessionStore {
    /// 打开/创建数据库（`path` 为 `:memory:` 时用内存库，测试用）。
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        if path != ":memory:" {
            if let Some(parent) = Path::new(path).parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                notes TEXT,
                meta TEXT
            );
            CREATE TABLE IF NOT EXISTS segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                speaker_id INTEGER NOT NULL,
                speaker_label TEXT NOT NULL,
                text TEXT NOT NULL,
                ts_ms INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                rms REAL NOT NULL DEFAULT 0,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS terms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS translations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                direction TEXT NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE INDEX IF NOT EXISTS idx_segments_text ON segments(text);
            CREATE INDEX IF NOT EXISTS idx_segments_session ON segments(session_id);
            CREATE INDEX IF NOT EXISTS idx_terms_session ON terms(session_id);
            CREATE INDEX IF NOT EXISTS idx_translations_session ON translations(session_id);
            ",
        )?;
        // 迁移（旧库）：sessions.meta / segments.duration_ms / segments.rms
        let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN meta TEXT;");
        let _ = conn.execute_batch("ALTER TABLE segments ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;");
        let _ = conn.execute_batch("ALTER TABLE segments ADD COLUMN rms REAL NOT NULL DEFAULT 0;");
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 开启新会话，返回 session id。
    pub fn start_session(&self, started_at: i64) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO sessions (started_at) VALUES (?1)", [started_at])?;
        Ok(conn.last_insert_rowid())
    }

    /// 结束会话。
    pub fn end_session(&self, session_id: i64, ended_at: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
            rusqlite::params![session_id, ended_at],
        )?;
        Ok(())
    }

    /// 追加转写段（含时长/能量统计）。
    pub fn add_segment(&self, session_id: i64, seg: &TranscriptSegment) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO segments (session_id, speaker_id, speaker_label, text, ts_ms, duration_ms, rms)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                session_id,
                seg.speaker_id,
                seg.speaker_label,
                seg.text,
                seg.ts_ms,
                seg.duration_ms,
                seg.rms
            ],
        )?;
        Ok(())
    }

    /// 保存会话元数据（统计 + 质量评估 JSON）。
    pub fn set_session_meta(&self, session_id: i64, meta: &SessionMeta) -> Result<()> {
        let json = serde_json::to_string(meta)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET meta = ?2 WHERE id = ?1",
            rusqlite::params![session_id, json],
        )?;
        Ok(())
    }

    /// 追加术语。
    pub fn add_term(&self, session_id: i64, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO terms (session_id, content) VALUES (?1,?2)",
            rusqlite::params![session_id, content],
        )?;
        Ok(())
    }

    /// 追加翻译。
    pub fn add_translation(&self, session_id: i64, direction: &str, content: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO translations (session_id, direction, content) VALUES (?1,?2,?3)",
            rusqlite::params![session_id, direction, content],
        )?;
        Ok(())
    }

    /// 保存纪要。
    pub fn set_notes(&self, session_id: i64, notes: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sessions SET notes = ?2 WHERE id = ?1",
            rusqlite::params![session_id, notes],
        )?;
        Ok(())
    }

    /// 会话列表（按时间倒序）。
    pub fn list_sessions(&self, limit: u32) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.started_at, s.ended_at,
                    (SELECT COUNT(*) FROM segments g WHERE g.session_id = s.id),
                    (SELECT COUNT(*) FROM terms t WHERE t.session_id = s.id),
                    s.meta
             FROM sessions s ORDER BY s.id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                let meta_raw: Option<String> = r.get(5)?;
                let meta = meta_raw.as_deref().and_then(SessionMeta::from_json);
                Ok(SessionRecord {
                    id: r.get(0)?,
                    started_at: r.get(1)?,
                    ended_at: r.get(2)?,
                    segment_count: r.get(3)?,
                    term_count: r.get(4)?,
                    quality: meta.as_ref().map(|m| m.quality.clone()),
                    duration_ms: meta.as_ref().map(|m| m.duration_ms),
                    speech_ratio: meta.as_ref().map(|m| m.speech_ratio),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 全文检索转写段。
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SegmentHit>> {
        let conn = self.conn.lock().unwrap();
        let q = format!("%{}%", query.trim());
        let mut stmt = conn.prepare(
            "SELECT session_id, speaker_label, text, ts_ms FROM segments
             WHERE text LIKE ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![q, limit], |r| {
                Ok(SegmentHit {
                    session_id: r.get(0)?,
                    speaker_label: r.get(1)?,
                    text: r.get(2)?,
                    ts_ms: r.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 会话详情。
    pub fn get_session(&self, session_id: i64) -> Result<SessionDetail> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, started_at, ended_at, notes, meta FROM sessions WHERE id = ?1",
                [session_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("会话不存在: {session_id}"))?;
        let (id, started_at, ended_at, notes, meta_raw) = row;

        let segments = {
            let mut stmt = conn.prepare(
                "SELECT speaker_id, speaker_label, text, ts_ms, duration_ms, rms FROM segments WHERE session_id = ?1 ORDER BY id",
            )?;
            let rows = stmt
                .query_map([session_id], |r| {
                    Ok(TranscriptSegment {
                        speaker_id: r.get(0)?,
                        speaker_label: r.get(1)?,
                        text: r.get(2)?,
                        is_partial: false,
                        ts_ms: r.get(3)?,
                        duration_ms: r.get(4)?,
                        rms: r.get(5)?,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let terms: Vec<String> = {
            let mut stmt = conn.prepare("SELECT content FROM terms WHERE session_id = ?1 ORDER BY id")?;
            let rows = stmt
                .query_map([session_id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        let translations: Vec<String> = {
            let mut stmt = conn.prepare("SELECT content FROM translations WHERE session_id = ?1 ORDER BY id")?;
            let rows = stmt
                .query_map([session_id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        Ok(SessionDetail {
            id,
            started_at,
            ended_at,
            segments,
            terms,
            translations,
            notes,
            meta: meta_raw.as_deref().and_then(SessionMeta::from_json),
        })
    }
}

impl SessionMeta {
    /// 从 JSON 解析（容错：损坏返回 None）。
    pub fn from_json(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SessionStore {
        SessionStore::open(":memory:").unwrap()
    }

    fn seg(speaker: u32, label: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            speaker_id: speaker,
            speaker_label: label.into(),
            text: text.into(),
            is_partial: false,
            ts_ms: 1000,
            duration_ms: 800,
            rms: 0.2,
        }
    }

    #[test]
    fn crud_roundtrip() {
        let s = store();
        let id = s.start_session(111).unwrap();
        s.add_segment(id, &seg(1, "客户", "We need NPI samples")).unwrap();
        s.add_segment(id, &seg(0, "我", "好的")).unwrap();
        s.add_term(id, "NPI = 新产品导入").unwrap();
        s.add_translation(id, "en_zh", "我们需要 NPI 样品").unwrap();
        s.end_session(id, 222).unwrap();

        let detail = s.get_session(id).unwrap();
        assert_eq!(detail.segments.len(), 2);
        assert_eq!(detail.segments[0].duration_ms, 800);
        assert!((detail.segments[0].rms - 0.2).abs() < 1e-6);
        assert_eq!(detail.terms, vec!["NPI = 新产品导入"]);
        assert_eq!(detail.translations.len(), 1);
        assert_eq!(detail.started_at, 111);
        assert_eq!(detail.ended_at, Some(222));
    }

    #[test]
    fn list_and_search() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(1, "客户", "The MOQ is 1000 units")).unwrap();
        s.add_segment(id, &seg(0, "我", "我们确认")).unwrap();

        let list = s.list_sessions(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].segment_count, 2);

        let hits = s.search("MOQ", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("MOQ"));

        let none = s.search("不存在词", 10).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn get_missing_session_errors() {
        let s = store();
        assert!(s.get_session(999).is_err());
    }

    #[test]
    fn notes_save_and_retrieve() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(1, "客户", "hello")).unwrap();
        s.set_notes(id, "# 会议纪要\n\n## 摘要\ntest").unwrap();
        let detail = s.get_session(id).unwrap();
        assert_eq!(detail.notes.as_deref(), Some("# 会议纪要\n\n## 摘要\ntest"));
    }

    #[test]
    fn session_meta_persists_and_lists_quality() {
        let s = store();
        let id = s.start_session(1).unwrap();
        s.add_segment(id, &seg(0, "我", "我们需要在周五之前拿到 NPI 样品")).unwrap();
        s.add_segment(id, &seg(0, "我", "另外请确认交期")).unwrap();
        let meta = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 60000,
                speech_ms: 45000,
                final_segments: 2,
                avg_rms: 0.12,
                max_rms: 0.6,
                recording: Some("2026-08-19_05-57-20_我.wav".into()),
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
            }],
            &["我们需要在周五之前拿到 NPI 样品".into(), "另外请确认交期".into()],
            12345,
        );
        assert_eq!(meta.quality, "clean");
        assert!(!meta.skipped_analysis);
        s.set_session_meta(id, &meta).unwrap();
        s.end_session(id, 2).unwrap();

        let list = s.list_sessions(10).unwrap();
        assert_eq!(list[0].quality.as_deref(), Some("clean"));
        assert_eq!(list[0].duration_ms, Some(60000));
        assert!((list[0].speech_ratio.unwrap() - 0.75).abs() < 1e-6);

        let detail = s.get_session(id).unwrap();
        let m = detail.meta.unwrap();
        assert_eq!(m.streams.len(), 1);
        assert_eq!(m.streams[0].recording.as_deref(), Some("2026-08-19_05-57-20_我.wav"));
    }

    #[test]
    fn quality_detects_noise_session() {
        // 模拟用户案例（13:57 会话）：VAD 把环境声音当语音，几乎无停顿（ratio 0.88）
        let meta = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 148000,
                speech_ms: 130000,
                final_segments: 16,
                avg_rms: 0.2,
                max_rms: 0.8,
                recording: None,
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
            }],
            &[
                "我说看嗯行嗯嗯哦不要行嗯".into(),
                "现然做为他规定嗯我还是没有思间别是奖们啊".into(),
                "你三三 d 是吗".into(),
                "嗯你看一下就会会会会有一个案".into(),
            ],
            12345,
        );
        assert_eq!(meta.quality, "noise", "持续有声（ratio 高）应判噪音: {meta:?}");
        assert!(meta.skipped_analysis);

        // 语气词/重复密集 → 文本噪音判 noise（即使 ratio 中等）
        let noisy_text = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 60000,
                speech_ms: 30000,
                final_segments: 5,
                avg_rms: 0.1,
                max_rms: 0.4,
                recording: None,
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
            }],
            &["嗯嗯嗯嗯嗯嗯嗯嗯嗯嗯".into(), "嗯嗯嗯对技术嗯嗯".into()],
            12345,
        );
        assert_eq!(noisy_text.quality, "noise");
        assert!(noisy_text.text_noise > 0.45);

        // 静音会话：几乎无语音
        let silent = SessionMeta::evaluate(
            vec![StreamMeta {
                speaker_label: "我".into(),
                total_ms: 60000,
                speech_ms: 2000,
                final_segments: 0,
                avg_rms: 0.002,
                max_rms: 0.01,
                recording: None,
                vad_preset: "standard".into(),
                vad_threshold: 0.5,
            }],
            &[],
            12345,
        );
        assert_eq!(silent.quality, "silent");
        assert!(silent.skipped_analysis);
    }
}
