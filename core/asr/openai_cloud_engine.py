import io
import wave
import numpy as np
from openai import OpenAI
from core.models import TranscriptSegment
from core.asr.base import ASREngine


def audio_to_wav_bytes(audio: np.ndarray, sample_rate: int = 16000) -> bytes:
    """Encode float32 mono PCM [-1, 1] as 16-bit WAV bytes."""
    clipped = np.clip(audio, -1.0, 1.0)
    pcm = (clipped * 32767.0).astype(np.int16)
    buf = io.BytesIO()
    with wave.open(buf, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(pcm.tobytes())
    return buf.getvalue()


class OpenAICloudEngine(ASREngine):
    """Cloud ASR via OpenAI-compatible /audio/transcriptions (batch chunks)."""

    def __init__(
        self,
        api_key: str,
        model: str = "whisper-1",
        base_url: str | None = None,
        language: str | None = None,
        sample_rate: int = 16000,
    ):
        self._api_key = api_key
        self._model = model
        self._base_url = base_url
        self._language = language
        self._sample_rate = sample_rate
        self._client: OpenAI | None = None

    def warmup(self) -> None:
        self._ensure_client()

    def _ensure_client(self) -> OpenAI:
        if self._client is None:
            kwargs: dict = {"api_key": self._api_key}
            if self._base_url:
                kwargs["base_url"] = self._base_url
            self._client = OpenAI(**kwargs)
        return self._client

    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None:
        client = self._ensure_client()
        wav = audio_to_wav_bytes(audio, sample_rate=self._sample_rate)
        file_obj = io.BytesIO(wav)
        file_obj.name = "chunk.wav"

        kwargs: dict = {
            "model": self._model,
            "file": file_obj,
        }
        if self._language:
            kwargs["language"] = self._language

        response = client.audio.transcriptions.create(**kwargs)
        text = (getattr(response, "text", None) or str(response) or "").strip()
        if not text:
            return None
        language = self._language or "en"
        return TranscriptSegment(speaker=speaker, text=text, language=language)
