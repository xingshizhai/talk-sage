import numpy as np
import wave
from pathlib import Path
from unittest.mock import MagicMock
from core.import_audio import load_audio_file, OfflineTranscriber
from core.models import TranscriptSegment


def _write_wav(path: Path, samples: np.ndarray, sample_rate: int = 16000) -> None:
    pcm = (np.clip(samples, -1, 1) * 32767).astype(np.int16)
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(pcm.tobytes())


def test_load_audio_file_wav(tmp_path):
    path = tmp_path / "a.wav"
    _write_wav(path, np.zeros(8000, dtype=np.float32))
    audio, sr = load_audio_file(path)
    assert sr == 16000
    assert audio.dtype == np.float32
    assert len(audio) == 8000


def test_offline_transcriber_chunks_and_joins():
    engine = MagicMock()
    engine.transcribe.side_effect = [
        TranscriptSegment(speaker="client", text="hello", language="en"),
        TranscriptSegment(speaker="client", text="world", language="en"),
        None,
    ]
    # 2.5 chunks of 1s at 4 samples/sec for tiny test → use real sizes via chunk_seconds
    ot = OfflineTranscriber(engine=engine, sample_rate=4, chunk_seconds=1)
    audio = np.zeros(10, dtype=np.float32)  # 2 full chunks + 2 leftover → 3 calls if we pad leftover
    text = ot.transcribe(audio, speaker="client")
    assert "hello" in text
    assert "world" in text
    assert engine.transcribe.call_count >= 2
