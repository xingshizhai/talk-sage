from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock, patch
import numpy as np
import pytest
from core.asr.bitnet_engine import (
    BitNetEngine,
    bitnet_available,
    parse_asr_infer_stdout,
    resample_audio,
    resolve_bitnet_paths,
)
from core.models import TranscriptSegment


def test_parse_asr_infer_stdout_plain():
    assert parse_asr_infer_stdout("hello world\n") == "hello world"


def test_parse_asr_infer_stdout_speaker_lines():
    out = "[0.00 - 1.20] Speaker 0: ask not\n[1.20 - 2.00] Speaker 0: what you can do\n"
    assert parse_asr_infer_stdout(out) == "ask not what you can do"


def test_resample_changes_length():
    audio = np.zeros(16000, dtype=np.float32)
    out = resample_audio(audio, 16000, 24000)
    assert len(out) == 24000


def test_resolve_paths_from_explicit(tmp_path: Path):
    binary = tmp_path / "asr_infer.exe"
    vae = tmp_path / "vibeasr-vae-encoder-i8_s.gguf"
    lm = tmp_path / "vibeasr-lm-i2_s-embed-q6_k.gguf"
    binary.write_bytes(b"x")
    vae.write_bytes(b"x")
    lm.write_bytes(b"x")
    b, v, l = resolve_bitnet_paths(str(binary), str(vae), str(lm))
    assert b == binary
    assert v == vae
    assert l == lm
    assert bitnet_available(str(binary), str(vae), str(lm))


def test_resolve_missing_raises():
    with pytest.raises(FileNotFoundError, match="asr_infer"):
        resolve_bitnet_paths(binary="/no/such/asr_infer", vae_model=None, lm_model=None)


def test_bitnet_transcribe_mocks_subprocess(tmp_path: Path):
    binary = tmp_path / "asr_infer.exe"
    vae = tmp_path / "vibeasr-vae-encoder-i8_s.gguf"
    lm = tmp_path / "vibeasr-lm-i2_s-embed-q6_k.gguf"
    for p in (binary, vae, lm):
        p.write_bytes(b"x")

    engine = BitNetEngine(
        binary=str(binary),
        vae_model=str(vae),
        lm_model=str(lm),
        threads=2,
    )
    mock_proc = MagicMock()
    mock_proc.returncode = 0
    mock_proc.stdout = "And so my fellow Americans\n"
    mock_proc.stderr = "timing...\n"

    with patch("core.asr.bitnet_engine.subprocess.run", return_value=mock_proc) as run:
        seg = engine.transcribe(np.zeros(16000, dtype=np.float32), speaker="client")

    assert isinstance(seg, TranscriptSegment)
    assert seg.text == "And so my fellow Americans"
    assert seg.language == "en"
    run.assert_called_once()
    cmd = run.call_args[0][0]
    assert str(binary) in cmd
    assert "--greedy" in cmd


def test_bitnet_raises_on_nonzero_exit(tmp_path: Path):
    binary = tmp_path / "asr_infer.exe"
    vae = tmp_path / "vae.gguf"
    lm = tmp_path / "lm.gguf"
    # Use resolve via explicit names that match defaults when passed as paths
    vae = tmp_path / "vibeasr-vae-encoder-i8_s.gguf"
    lm = tmp_path / "vibeasr-lm-i2_s-embed-q6_k.gguf"
    for p in (binary, vae, lm):
        p.write_bytes(b"x")
    engine = BitNetEngine(binary=str(binary), vae_model=str(vae), lm_model=str(lm))
    mock_proc = MagicMock(returncode=1, stdout="", stderr="boom")
    with patch("core.asr.bitnet_engine.subprocess.run", return_value=mock_proc):
        with pytest.raises(RuntimeError, match="exit 1"):
            engine.transcribe(np.zeros(8000, dtype=np.float32), speaker="client")


def test_factory_builds_bitnet():
    from core.asr.factory import build_asr_engine
    from core.asr.bitnet_engine import BitNetEngine as BE

    with patch("core.device_probe.detect_compute_device", return_value="cpu"):
        engine = build_asr_engine({
            "mode": "local",
            "client": {"engine": "bitnet", "device": "cpu"},
            "user": {"model": "paraformer-zh", "device": "cpu"},
            "bitnet": {
                "binary": "C:/fake/asr_infer.exe",
                "vae_model": "C:/fake/vae.gguf",
                "lm_model": "C:/fake/lm.gguf",
                "threads": 4,
            },
        })
    assert isinstance(engine._client_engine, BE)
