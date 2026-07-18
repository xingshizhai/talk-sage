import numpy as np
from core.models import TranscriptSegment
from core.asr.base import ASREngine


class FunASREngine(ASREngine):
    """Chinese ASR using FunASR Paraformer with VAD and punctuation."""

    def __init__(
        self,
        model: str = "paraformer-zh",
        vad_model: str = "fsmn-vad",
        punc_model: str = "ct-punc",
        device: str = "cpu",
    ):
        self._model_name = model
        self._vad_model = vad_model
        self._punc_model = punc_model
        self._device = device
        self._model = None

    def warmup(self) -> None:
        self._load_model()

    def _load_model(self) -> None:
        try:
            from funasr import AutoModel
        except ImportError as e:
            raise RuntimeError(
                "FunASR not installed. Run: pip install funasr modelscope"
            ) from e
        self._model = AutoModel(
            model=self._model_name,
            vad_model=self._vad_model,
            punc_model=self._punc_model,
            device=self._device,
        )

    def transcribe(self, audio: np.ndarray, speaker: str) -> TranscriptSegment | None:
        if self._model is None:
            self._load_model()

        # FunASR expects float32 ndarray at 16 kHz
        result = self._model.generate(input=audio, batch_size_s=300)
        if not result or not result[0].get("text"):
            return None
        text = result[0]["text"].strip()
        if not text:
            return None
        return TranscriptSegment(speaker=speaker, text=text, language="zh")
