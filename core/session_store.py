from __future__ import annotations

from datetime import datetime
from pathlib import Path
from core.models import TranscriptSegment, PluginResult
from core.session_db import SessionDatabase

_SPEAKER_LABEL = {"client": "客户", "user": "我"}


class SessionStore:
    """Auto-save transcript + terms as Markdown; optionally mirror into SQLite."""

    def __init__(
        self,
        sessions_dir: Path | None = None,
        db: SessionDatabase | None = None,
    ):
        self._dir = sessions_dir or (Path.home() / ".talksage" / "sessions")
        self._db = db
        self._path: Path | None = None
        self._last_path: Path | None = None
        self._session_id: int | None = None
        self._segments: list[TranscriptSegment] = []
        self._terms: dict[str, str] = {}
        self._term_order: list[str] = []
        self._active = False

    @property
    def active(self) -> bool:
        return self._active

    @property
    def db(self) -> SessionDatabase | None:
        return self._db

    def start(self) -> Path:
        self._dir.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
        self._path = self._dir / f"{stamp}.md"
        self._segments = []
        self._terms = {}
        self._term_order = []
        self._last_path = None
        self._session_id = None
        self._active = True
        if self._db is not None:
            self._session_id = self._db.start_session(
                stamp=stamp, markdown_path=str(self._path)
            )
        self._flush()
        return self._path

    def add_segment(self, segment: TranscriptSegment) -> None:
        if not self._active:
            return
        self._segments.append(segment)
        if self._db is not None and self._session_id is not None:
            self._db.add_segment(self._session_id, segment)
        self._flush()

    def add_result(self, result: PluginResult) -> None:
        if not self._active:
            return
        if result.ui_section != "terms":
            return
        if result.status == "skeleton":
            return
        if not result.content:
            return
        key = result.result_id or result.content
        if key not in self._terms:
            self._term_order.append(key)
            if self._db is not None and self._session_id is not None:
                self._db.add_term(self._session_id, result.content)
        self._terms[key] = result.content
        self._flush()

    def stop(self) -> Path | None:
        if not self._active:
            return None
        self._flush()
        path = self._path
        if self._db is not None and self._session_id is not None:
            self._db.end_session(self._session_id)
        self._active = False
        self._last_path = path
        self._path = None
        return path

    def terms(self) -> list[str]:
        return [self._terms[k] for k in self._term_order]

    def last_path(self) -> Path | None:
        return self._last_path

    def append_notes(self, notes: str, path: Path | None = None) -> Path | None:
        target = path or self._path or self._last_path
        if target is None:
            return None
        existing = target.read_text(encoding="utf-8") if target.exists() else ""
        if "## 会议纪要" in existing:
            head = existing.split("## 会议纪要")[0].rstrip() + "\n\n"
        else:
            head = existing.rstrip() + "\n\n"
        target.write_text(head + "## 会议纪要\n\n" + notes.strip() + "\n", encoding="utf-8")
        if self._db is not None and self._session_id is not None:
            self._db.set_notes(self._session_id, notes.strip())
        return target

    def _flush(self) -> None:
        if self._path is None:
            return
        lines = [
            "# TalkSage Session",
            "",
            f"- started: {self._path.stem}",
            "",
            "## 转写",
            "",
        ]
        for seg in self._segments:
            label = _SPEAKER_LABEL.get(seg.speaker, seg.speaker)
            lines.append(f"- **{label}**: {seg.text}")
        lines.extend(["", "## 术语", ""])
        if self._term_order:
            for key in self._term_order:
                lines.append(f"- {self._terms[key]}")
        else:
            lines.append("- （暂无）")
        lines.append("")
        self._path.write_text("\n".join(lines), encoding="utf-8")
