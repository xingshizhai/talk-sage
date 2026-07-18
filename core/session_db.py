from __future__ import annotations

import sqlite3
import time
from pathlib import Path
from core.models import TranscriptSegment


class SessionDatabase:
    """SQLite store for searchable meeting sessions (complements Markdown export)."""

    def __init__(self, db_path: Path | str):
        self._path = Path(db_path)
        self._path.parent.mkdir(parents=True, exist_ok=True)
        self._conn = sqlite3.connect(str(self._path), check_same_thread=False)
        self._conn.row_factory = sqlite3.Row
        self._init_schema()

    def close(self) -> None:
        self._conn.close()

    def _init_schema(self) -> None:
        self._conn.executescript(
            """
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                stamp TEXT NOT NULL,
                markdown_path TEXT,
                notes TEXT,
                started_at REAL NOT NULL,
                ended_at REAL
            );
            CREATE TABLE IF NOT EXISTS segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                speaker TEXT NOT NULL,
                language TEXT,
                text TEXT NOT NULL,
                timestamp REAL NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE TABLE IF NOT EXISTS terms (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            CREATE INDEX IF NOT EXISTS idx_segments_text ON segments(text);
            CREATE INDEX IF NOT EXISTS idx_sessions_stamp ON sessions(stamp);
            """
        )
        self._conn.commit()

    def start_session(self, stamp: str, markdown_path: str = "") -> int:
        cur = self._conn.execute(
            "INSERT INTO sessions (stamp, markdown_path, started_at) VALUES (?, ?, ?)",
            (stamp, markdown_path, time.time()),
        )
        self._conn.commit()
        return int(cur.lastrowid)

    def add_segment(self, session_id: int, segment: TranscriptSegment) -> None:
        self._conn.execute(
            "INSERT INTO segments (session_id, speaker, language, text, timestamp) VALUES (?, ?, ?, ?, ?)",
            (session_id, segment.speaker, segment.language, segment.text, segment.timestamp),
        )
        self._conn.commit()

    def add_term(self, session_id: int, content: str) -> None:
        self._conn.execute(
            "INSERT INTO terms (session_id, content) VALUES (?, ?)",
            (session_id, content),
        )
        self._conn.commit()

    def set_notes(self, session_id: int, notes: str) -> None:
        self._conn.execute(
            "UPDATE sessions SET notes = ? WHERE id = ?",
            (notes, session_id),
        )
        self._conn.commit()

    def end_session(self, session_id: int) -> None:
        self._conn.execute(
            "UPDATE sessions SET ended_at = ? WHERE id = ?",
            (time.time(), session_id),
        )
        self._conn.commit()

    def list_sessions(self, limit: int = 50) -> list[dict]:
        rows = self._conn.execute(
            """
            SELECT s.id, s.stamp, s.markdown_path, s.started_at, s.ended_at,
                   (SELECT COUNT(*) FROM segments g WHERE g.session_id = s.id) AS segment_count
            FROM sessions s
            ORDER BY s.id DESC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
        return [dict(r) for r in rows]

    def get_session(self, session_id: int) -> dict:
        row = self._conn.execute(
            "SELECT * FROM sessions WHERE id = ?", (session_id,)
        ).fetchone()
        if row is None:
            raise KeyError(f"session {session_id} not found")
        segments = self._conn.execute(
            "SELECT speaker, language, text, timestamp FROM segments WHERE session_id = ? ORDER BY id",
            (session_id,),
        ).fetchall()
        terms = self._conn.execute(
            "SELECT content FROM terms WHERE session_id = ? ORDER BY id",
            (session_id,),
        ).fetchall()
        data = dict(row)
        data["segments"] = [dict(s) for s in segments]
        data["terms"] = [t["content"] for t in terms]
        return data

    def search(self, query: str, limit: int = 20) -> list[dict]:
        q = f"%{query.strip()}%"
        rows = self._conn.execute(
            """
            SELECT g.session_id, g.speaker, g.text, g.timestamp, s.stamp
            FROM segments g
            JOIN sessions s ON s.id = g.session_id
            WHERE g.text LIKE ?
            ORDER BY g.id DESC
            LIMIT ?
            """,
            (q, limit),
        ).fetchall()
        return [dict(r) for r in rows]
