import numpy as np
from core.models import TranscriptSegment


class TranscribeEngine:
    def __init__(self, model_size: str = "base"):
        self._model_size = model_size
        self._model = None  # lazy load on first use

    def _load_model(self):
        from faster_whisper import WhisperModel
        self._model = WhisperModel(self._model_size, device="cpu", compute_type="int8")

    def transcribe(self, audio: "np.ndarray", speaker: str) -> TranscriptSegment | None:
        if self._model is None:
            self._load_model()
        segments, info = self._model.transcribe(audio, beam_size=5)
        text_parts = [seg.text for seg in segments]
        if not text_parts:
            return None
        text = "".join(text_parts).strip()
        if not text:
            return None
        return TranscriptSegment(
            speaker=speaker,
            text=text,
            language=info.language,
        )
