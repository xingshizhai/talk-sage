from __future__ import annotations

import re
from core.models import ConversationState, TranscriptSegment

_QUESTION_START = re.compile(
    r"^(what|how|why|when|where|who|which|should|could|would|can|do|does|is|are)\b",
    re.I,
)
_DECISION_RE = re.compile(
    r"\b(we agreed|let'?s go with|we(?:'ll| will) go with|decided to|proceed with|settled on)\b",
    re.I,
)
_WORD_RE = re.compile(r"[A-Za-z]{3,}|\b[A-Z]{2,}\b")


class StateTracker:
    """Heuristic conversation-state updater (no LLM required)."""

    def __init__(self, max_questions: int = 8, max_decisions: int = 6):
        self.state = ConversationState()
        self._max_questions = max_questions
        self._max_decisions = max_decisions

    def update(self, segment: TranscriptSegment) -> ConversationState:
        if segment.speaker != "client":
            return self.state

        text = segment.text.strip()
        if not text:
            return self.state

        self._update_topic(text)
        self._maybe_add_question(text)
        self._maybe_add_decision(text)
        return self.state

    def _update_topic(self, text: str) -> None:
        words = _WORD_RE.findall(text)
        if not words:
            self.state.topic = text[:80]
            return
        acronyms = [w for w in words if w.isupper() and len(w) >= 2]
        if acronyms:
            self.state.topic = " ".join(
                acronyms[:4] + [w for w in words if w not in acronyms][:4]
            )[:80]
        else:
            self.state.topic = text[:80]

    def _maybe_add_question(self, text: str) -> None:
        is_q = "?" in text or bool(_QUESTION_START.search(text))
        if not is_q:
            return
        if text in self.state.open_questions:
            return
        self.state.open_questions.append(text)
        self.state.open_questions = self.state.open_questions[-self._max_questions :]

    def _maybe_add_decision(self, text: str) -> None:
        if not _DECISION_RE.search(text):
            return
        if text in self.state.recent_decisions:
            return
        self.state.recent_decisions.append(text)
        self.state.recent_decisions = self.state.recent_decisions[-self._max_decisions :]
