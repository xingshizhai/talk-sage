//! TalkSage v2 会话持久化：SQLite（rusqlite bundled）。
//!
//! 表：sessions / segments / terms / translations。
//! SessionStore 线程安全（内部 Mutex<Connection>），可由 pipeline 事件线程写入。

use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use talksage_core::TranscriptSegment;

/// 会话概要（历史列表用）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionRecord {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub segment_count: u64,
    pub term_count: u64,
}

/// 搜索命中（跨会话文本检索）。
#[derive(Debug, Clone, Serialize)]
pub struct SegmentHit {
    pub session_id: i64,
    pub speaker_label: String,
    pub text: String,
    pub ts_ms: u64,
}

/// 会话详情（含全部内容）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub segments: Vec<TranscriptSegment>,
    pub terms: Vec<String>,
    pub translations: Vec<String>,
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
                ended_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                speaker_id INTEGER NOT NULL,
                speaker_label TEXT NOT NULL,
                text TEXT NOT NULL,
                ts_ms INTEGER NOT NULL,
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

    /// 追加转写段。
    pub fn add_segment(&self, session_id: i64, seg: &TranscriptSegment) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO segments (session_id, speaker_id, speaker_label, text, ts_ms) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![session_id, seg.speaker_id, seg.speaker_label, seg.text, seg.ts_ms],
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

    /// 会话列表（按时间倒序）。
    pub fn list_sessions(&self, limit: u32) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.started_at, s.ended_at,
                    (SELECT COUNT(*) FROM segments g WHERE g.session_id = s.id),
                    (SELECT COUNT(*) FROM terms t WHERE t.session_id = s.id)
             FROM sessions s ORDER BY s.id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok(SessionRecord {
                    id: r.get(0)?,
                    started_at: r.get(1)?,
                    ended_at: r.get(2)?,
                    segment_count: r.get(3)?,
                    term_count: r.get(4)?,
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
                "SELECT id, started_at, ended_at FROM sessions WHERE id = ?1",
                [session_id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?)),
            )
            .optional()?
            .ok_or_else(|| anyhow!("会话不存在: {session_id}"))?;
        let (id, started_at, ended_at) = row;

        let segments = {
            let mut stmt = conn.prepare(
                "SELECT speaker_id, speaker_label, text, ts_ms FROM segments WHERE session_id = ?1 ORDER BY id",
            )?;
            let rows = stmt
                .query_map([session_id], |r| {
                    Ok(TranscriptSegment {
                        speaker_id: r.get(0)?,
                        speaker_label: r.get(1)?,
                        text: r.get(2)?,
                        is_partial: false,
                        ts_ms: r.get(3)?,
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
        })
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
}
