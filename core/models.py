from __future__ import annotations

import time
from dataclasses import dataclass, field
from collections import deque
from typing import TYPE_CHECKING, Literal

if TYPE_CHECKING:
    from core.knowledge_base import KnowledgeBase


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
    result_id: str = ""  # stable id so UI can update skeleton → final
    status: Literal["skeleton", "final"] = "final"


@dataclass
class ConversationState:
    topic: str = ""
    summary: str = ""
    open_questions: list[str] = field(default_factory=list)
    recent_decisions: list[str] = field(default_factory=list)

    def as_brief(self) -> str:
        lines = []
        if self.topic:
            lines.append(f"话题: {self.topic}")
        if self.summary:
            lines.append(f"摘要: {self.summary}")
        if self.open_questions:
            lines.append("未决问题: " + "；".join(self.open_questions[:5]))
        if self.recent_decisions:
            lines.append("近期决策: " + "；".join(self.recent_decisions[:5]))
        return "\n".join(lines) if lines else "（暂无上下文）"


class ConversationContext:
    def __init__(self, max_segments: int = 50):
        self._segments: deque[TranscriptSegment] = deque(maxlen=max_segments)
        self.state: ConversationState = ConversationState()
        self.knowledge_base: KnowledgeBase | None = None

    def add(self, segment: TranscriptSegment) -> None:
        self._segments.append(segment)

    def recent(self, n: int | None = None) -> list[TranscriptSegment]:
        segments = list(self._segments)
        return segments if n is None else segments[-n:]

    def as_text(self) -> str:
        return "\n".join(
            f"[{seg.speaker}] {seg.text}" for seg in self._segments
        )
