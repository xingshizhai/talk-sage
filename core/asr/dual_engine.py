import numpy as np
from core.models import TranscriptSegment
from core.asr.base import ASREngine


class DualASREngine(ASREngine):
    """Routes audio to the appropriate engine based on speaker.

    client → FasterWhisperEngine (English)
    user   → FunASREngine (Chinese)
    """

    def __init__(self, client_engine: ASREngine, user_engine: ASREngine):
        self._client_engine = client_engine
        self._user_engine = user_engine

    def warmup(self) -> None:
        self._client_engine.warmup()
        self._user_engine.warmup()

    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None:
        if speaker == "client":
            return self._client_engine.transcribe(audio, speaker)
        return self._user_engine.transcribe(audio, speaker)
