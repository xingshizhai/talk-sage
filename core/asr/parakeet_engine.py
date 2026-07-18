from __future__ import annotations

import tempfile
from pathlib import Path
import numpy as np
from core.asr.base import ASREngine
from core.asr.openai_cloud_engine import audio_to_wav_bytes
from core.models import TranscriptSegment


class ParakeetEngine(ASREngine):
    """English (multilingual) ASR via NVIDIA Parakeet ONNX (onnx-asr).

    Optional dependency: pip install "onnx-asr[cpu,hub]"
    Model downloads from Hugging Face on first use (~0.6B).
    """

    def __init__(
        self,
        model_id: str = "nemo-parakeet-tdt-0.6b-v3",
        device: str = "cpu",
        language: str = "en",
    ):
        self._model_id = model_id
        self._device = device
        self._language = language
        self._model = None

    def warmup(self) -> None:
        self._load_model()

    def _load_model(self) -> None:
        try:
            import onnx_asr
        except ImportError as e:
            raise RuntimeError(
                "Parakeet requires onnx-asr. Install: pip install \"onnx-asr[cpu,hub]\""
            ) from e
        # providers: cpu / cuda if supported by installed onnxruntime
        kwargs: dict = {}
        if self._device == "cuda":
            kwargs["providers"] = ["CUDAExecutionProvider", "CPUExecutionProvider"]
        try:
            self._model = onnx_asr.load_model(self._model_id, **kwargs)
        except TypeError:
            # Older onnx-asr may not accept providers=
            self._model = onnx_asr.load_model(self._model_id)

    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None:
        if self._model is None:
            self._load_model()

        wav = audio_to_wav_bytes(np.asarray(audio, dtype=np.float32), sample_rate=16000)
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            tmp.write(wav)
            tmp_path = Path(tmp.name)
        try:
            text = self._model.recognize(str(tmp_path))
        finally:
            try:
                tmp_path.unlink(missing_ok=True)
            except OSError:
                pass

        if text is None:
            return None
        if isinstance(text, (list, tuple)):
            text = " ".join(str(t) for t in text)
        text = str(text).strip()
        if not text:
            return None
        return TranscriptSegment(speaker=speaker, text=text, language=self._language)
