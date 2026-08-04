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
    ot = OfflineTranscriber(engine=engine, sample_rate=4, chunk_seconds=1)
    audio = np.zeros(10, dtype=np.float32)
    text = ot.transcribe(audio, speaker="client")
    assert "hello" in text
    assert "world" in text
    assert engine.transcribe.call_count >= 2


def test_offline_transcriber_whole_buffer():
    engine = MagicMock()
    engine.transcribe.return_value = TranscriptSegment(
        speaker="client", text="full pass", language="en"
    )
    ot = OfflineTranscriber(engine=engine, sample_rate=16000, chunk_seconds=0)
    audio = np.zeros(32000, dtype=np.float32)
    assert ot.transcribe(audio) == "full pass"
    assert engine.transcribe.call_count == 1
    assert len(engine.transcribe.call_args[0][0]) == 32000


def test_resolve_import_engine_prefers_bitnet(tmp_path):
    from core.asr.factory import resolve_import_engine
    from core.asr.bitnet_engine import BitNetEngine

    binary = tmp_path / "asr_infer.exe"
    vae = tmp_path / "vibeasr-vae-encoder-i8_s.gguf"
    lm = tmp_path / "vibeasr-lm-i2_s-embed-q6_k.gguf"
    for p in (binary, vae, lm):
        p.write_bytes(b"x")
    eng = resolve_import_engine({
        "mode": "local",
        "client": {"engine": "faster-whisper", "model": "small", "device": "cpu"},
        "user": {"model": "paraformer-zh", "device": "cpu"},
        "bitnet": {
            "binary": str(binary),
            "vae_model": str(vae),
            "lm_model": str(lm),
        },
        "import": {"prefer_bitnet": True},
    })
    assert isinstance(eng, BitNetEngine)
