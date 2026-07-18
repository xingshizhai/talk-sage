from __future__ import annotations

import wave
from pathlib import Path
import numpy as np
from core.asr.base import ASREngine


def load_audio_file(path: Path | str, target_sr: int = 16000) -> tuple[np.ndarray, int]:
    """Load mono float32 audio. WAV via stdlib; other formats via soundfile if available."""
    path = Path(path)
    suffix = path.suffix.lower()
    if suffix == ".wav":
        return _load_wav(path, target_sr)
    try:
        import soundfile as sf
    except ImportError as e:
        raise RuntimeError(
            "Non-WAV import requires soundfile. Install: pip install soundfile"
        ) from e
    data, sr = sf.read(str(path), dtype="float32", always_2d=False)
    if data.ndim > 1:
        data = data.mean(axis=1)
    if sr != target_sr:
        data = _resample(data, sr, target_sr)
        sr = target_sr
    return data.astype(np.float32), sr


def _load_wav(path: Path, target_sr: int) -> tuple[np.ndarray, int]:
    with wave.open(str(path), "rb") as wf:
        channels = wf.getnchannels()
        width = wf.getsampwidth()
        sr = wf.getframerate()
        frames = wf.readframes(wf.getnframes())
    if width == 2:
        pcm = np.frombuffer(frames, dtype=np.int16).astype(np.float32) / 32768.0
    elif width == 4:
        pcm = np.frombuffer(frames, dtype=np.int32).astype(np.float32) / 2147483648.0
    else:
        pcm = np.frombuffer(frames, dtype=np.uint8).astype(np.float32)
        pcm = (pcm - 128.0) / 128.0
    if channels > 1:
        pcm = pcm.reshape(-1, channels).mean(axis=1)
    if sr != target_sr:
        pcm = _resample(pcm, sr, target_sr)
        sr = target_sr
    return pcm.astype(np.float32), sr


def _resample(audio: np.ndarray, src_sr: int, dst_sr: int) -> np.ndarray:
    if src_sr == dst_sr or audio.size == 0:
        return audio
    duration = len(audio) / src_sr
    new_len = max(1, int(duration * dst_sr))
    x_old = np.linspace(0.0, 1.0, num=len(audio), endpoint=False)
    x_new = np.linspace(0.0, 1.0, num=new_len, endpoint=False)
    return np.interp(x_new, x_old, audio).astype(np.float32)


class OfflineTranscriber:
    """Chunk an audio buffer and run ASREngine, joining non-empty texts."""

    def __init__(
        self,
        engine: ASREngine,
        sample_rate: int = 16000,
        chunk_seconds: int = 3,
    ):
        self._engine = engine
        self._sample_rate = sample_rate
        self._chunk_size = sample_rate * chunk_seconds

    def transcribe(self, audio: np.ndarray, speaker: str = "client") -> str:
        parts: list[str] = []
        if audio.size == 0:
            return ""
        # Pad short leftover so final speech is not dropped
        total = len(audio)
        offset = 0
        while offset < total:
            end = min(offset + self._chunk_size, total)
            chunk = audio[offset:end]
            if len(chunk) < self._chunk_size:
                padded = np.zeros(self._chunk_size, dtype=np.float32)
                padded[: len(chunk)] = chunk
                chunk = padded
            segment = self._engine.transcribe(chunk, speaker=speaker)
            if segment and segment.text.strip():
                parts.append(segment.text.strip())
            offset += self._chunk_size
        return " ".join(parts)
