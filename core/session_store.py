from __future__ import annotations

from datetime import datetime
from pathlib import Path
from core.models import TranscriptSegment, PluginResult

_SPEAKER_LABEL = {"client": "客户", "user": "我"}


class SessionStore:
    """Auto-save transcript + final plugin results as Markdown under sessions_dir."""

    def __init__(self, sessions_dir: Path | None = None):
        self._dir = sessions_dir or (Path.home() / ".talksage" / "sessions")
        self._path: Path | None = None
        self._last_path: Path | None = None
        self._segments: list[TranscriptSegment] = []
        self._terms: dict[str, str] = {}  # result_id or content key -> final content
        self._term_order: list[str] = []
        self._active = False

    @property
    def active(self) -> bool:
        return self._active

    def start(self) -> Path:
        self._dir.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
        self._path = self._dir / f"{stamp}.md"
        self._segments = []
        self._terms = {}
        self._term_order = []
        self._last_path = None
        self._active = True
        self._flush()
        return self._path

    def add_segment(self, segment: TranscriptSegment) -> None:
        if not self._active:
            return
        self._segments.append(segment)
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
        self._terms[key] = result.content
        self._flush()

    def stop(self) -> Path | None:
        if not self._active:
            return None
        self._flush()
        path = self._path
        self._active = False
        # Keep path for post-session notes append
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
            # Replace previous notes section
            head = existing.split("## 会议纪要")[0].rstrip() + "\n\n"
        else:
            head = existing.rstrip() + "\n\n"
        target.write_text(head + "## 会议纪要\n\n" + notes.strip() + "\n", encoding="utf-8")
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
