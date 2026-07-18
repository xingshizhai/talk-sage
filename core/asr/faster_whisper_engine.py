import numpy as np
from core.models import TranscriptSegment
from core.asr.base import ASREngine


class FasterWhisperEngine(ASREngine):
    """English ASR using faster-whisper with VAD filtering."""

    def __init__(
        self,
        model_size: str = "small",
        device: str = "cpu",
        compute_type: str = "int8",
        vad_filter: bool = True,
        language: str | None = "en",
    ):
        self._model_size = model_size
        self._device = device
        self._compute_type = compute_type
        self._vad_filter = vad_filter
        self._language = language
        self._model = None

    def warmup(self) -> None:
        self._load_model()

    def _load_model(self) -> None:
        from faster_whisper import WhisperModel
        self._model = WhisperModel(
            self._model_size,
            device=self._device,
            compute_type=self._compute_type,
        )

    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None:
        if self._model is None:
            self._load_model()

        kwargs: dict = {"beam_size": 5}
        if self._language:
            kwargs["language"] = self._language
        if self._vad_filter:
            kwargs["vad_filter"] = True
            kwargs["vad_parameters"] = {"min_silence_duration_ms": 500}

        segments, info = self._model.transcribe(audio, **kwargs)
        text_parts = [seg.text for seg in segments]
        if not text_parts:
            return None
        text = "".join(text_parts).strip()
        if not text:
            return None
        return TranscriptSegment(speaker=speaker, text=text, language=info.language)
