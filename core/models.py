import time
from dataclasses import dataclass, field
from collections import deque
from typing import Literal


@dataclass
class TranscriptSegment:
    speaker: Literal["user", "client"]
    text: str
    language: str
    timestamp: float = field(default_factory=time.time)


@dataclass
class PluginResult:
    plugin_name: str
    ui_section: Literal["transcript", "terms", "translation", "suggestions"]
    content: str
    priority: int = 0  # higher = more prominent in UI


class ConversationContext:
    def __init__(self, max_segments: int = 50):
        self._segments: deque[TranscriptSegment] = deque(maxlen=max_segments)

    def add(self, segment: TranscriptSegment) -> None:
        self._segments.append(segment)

    def recent(self, n: int | None = None) -> list[TranscriptSegment]:
        segments = list(self._segments)
        return segments if n is None else segments[-n:]

    def as_text(self) -> str:
        return "\n".join(
            f"[{seg.speaker}] {seg.text}" for seg in self._segments
        )
