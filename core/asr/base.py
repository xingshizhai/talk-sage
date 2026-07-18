from abc import ABC, abstractmethod
import numpy as np
from core.models import TranscriptSegment


class ASREngine(ABC):
    def warmup(self) -> None:
        """Pre-load model to avoid first-use latency. Optional."""

    @abstractmethod
    def transcribe(self, audio: "np.ndarray", speaker: str) -> TranscriptSegment | None:
        ...
