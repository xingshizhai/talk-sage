import re
import time
from collections import deque
from core.models import TranscriptSegment

_TOKEN_RE = re.compile(r"[\w']+", re.UNICODE)


def text_similarity(a: str, b: str) -> float:
    """Jaccard similarity over lowercased word tokens."""
    ta = set(_TOKEN_RE.findall(a.lower()))
    tb = set(_TOKEN_RE.findall(b.lower()))
    if not ta or not tb:
        return 0.0
    return len(ta & tb) / len(ta | tb)


class CrosstalkFilter:
    """Drop mic segments that look like echo of recent loopback (client) speech.

    Dual-path capture often leaks system audio into the microphone. After ASR,
    if the user transcript is highly similar to a recent client transcript,
    treat it as crosstalk and discard.
    """

    def __init__(self, similarity_threshold: float = 0.6, window_seconds: float = 8.0):
        self._threshold = similarity_threshold
        self._window = window_seconds
        self._recent_client: deque[TranscriptSegment] = deque(maxlen=20)

    def observe(self, segment: TranscriptSegment) -> None:
        if segment.speaker == "client":
            self._recent_client.append(segment)

    def should_drop(self, segment: TranscriptSegment) -> bool:
        if segment.speaker != "user":
            return False
        now = segment.timestamp
        for client in self._recent_client:
            if now - client.timestamp > self._window:
                continue
            if text_similarity(segment.text, client.text) >= self._threshold:
                return True
        return False
